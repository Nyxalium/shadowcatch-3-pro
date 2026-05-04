use anyhow::{anyhow, Context, Result};
use gst::prelude::*;
use gtk::glib;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const STARTUP_RETRY_LIMIT: usize = 6;
const STARTUP_RETRY_INTERVAL: Duration = Duration::from_millis(700);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureSettings {
    pub video_device: String,
    pub audio_device: Option<String>,
    pub pixel_format: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub debug_stats: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureEvent {
    PipelineStarted,
    Retrying(String),
    VideoFrame,
    AudioFrame,
    DebugStats(String),
    Warning(String),
    Error(String),
    NoFrames {
        waiting_for_video: bool,
        waiting_for_audio: bool,
    },
}

pub struct CaptureController {
    pipeline: Option<gst::Pipeline>,
    bus_watch: Option<gst::bus::BusWatchGuard>,
    health_check_cancel: Option<Arc<AtomicBool>>,
    stats_report_cancel: Option<Arc<AtomicBool>>,
}

impl CaptureController {
    pub fn new() -> Self {
        Self {
            pipeline: None,
            bus_watch: None,
            health_check_cancel: None,
            stats_report_cancel: None,
        }
    }

    pub fn start<F>(
        &mut self,
        settings: &CaptureSettings,
        on_event: F,
    ) -> Result<gtk::gdk::Paintable>
    where
        F: Fn(CaptureEvent) + 'static,
    {
        self.stop();

        let on_event: std::rc::Rc<dyn Fn(CaptureEvent)> = std::rc::Rc::new(on_event);
        let pipeline_description = build_pipeline_description(settings);
        let element = gst::parse::launch(&pipeline_description)
            .with_context(|| format!("Failed to build pipeline:\n{pipeline_description}"))?;
        let pipeline = element
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow!("GStreamer did not create a pipeline"))?;

        let video_sink = pipeline
            .by_name("video_sink")
            .ok_or_else(|| anyhow!("gtk4paintablesink is missing from the pipeline"))?;
        let paintable = video_sink.property::<gtk::gdk::Paintable>("paintable");

        let video_seen = attach_first_buffer_probe(&pipeline, "video_probe", "video-frame");
        let audio_seen = if settings.audio_device.is_some() {
            Some(attach_first_buffer_probe(
                &pipeline,
                "audio_probe",
                "audio-frame",
            ))
        } else {
            None
        };

        if settings.debug_stats {
            let stats_cancel = start_debug_stats(
                &pipeline,
                settings.audio_device.is_some(),
                std::rc::Rc::clone(&on_event),
            );
            self.stats_report_cancel = Some(stats_cancel);
        }

        if let Some(bus) = pipeline.bus() {
            let on_bus_event = std::rc::Rc::clone(&on_event);
            let source_id = bus
                .add_watch_local(move |_, message| {
                    use gst::MessageView;

                    match message.view() {
                        MessageView::Error(error) => {
                            let source = message
                                .src()
                                .map(|source| source.path_string().to_string())
                                .unwrap_or_else(|| "GStreamer".to_string());
                            on_bus_event(CaptureEvent::Error(format!(
                                "{source} error: {}{}",
                                error.error(),
                                error
                                    .debug()
                                    .map(|debug| format!("\n{debug}"))
                                    .unwrap_or_default()
                            )));
                        }
                        MessageView::Warning(warning) => {
                            let source = message
                                .src()
                                .map(|source| source.path_string().to_string())
                                .unwrap_or_else(|| "GStreamer".to_string());
                            on_bus_event(CaptureEvent::Warning(format!(
                                "{source} warning: {}{}",
                                warning.error(),
                                warning
                                    .debug()
                                    .map(|debug| format!("\n{debug}"))
                                    .unwrap_or_default()
                            )));
                        }
                        MessageView::Application(application) => {
                            if let Some(structure) = application.structure() {
                                if structure.name() == "shadowcatch-status" {
                                    match structure.get::<&str>("kind") {
                                        Ok("video-frame") => {
                                            on_bus_event(CaptureEvent::VideoFrame);
                                        }
                                        Ok("audio-frame") => {
                                            on_bus_event(CaptureEvent::AudioFrame);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        _ => {}
                    }

                    glib::ControlFlow::Continue
                })
                .context("Failed to attach GStreamer diagnostics")?;
            self.bus_watch = Some(source_id);
        }

        pipeline
            .set_state(gst::State::Playing)
            .context("Failed to start capture pipeline")?;

        on_event(CaptureEvent::PipelineStarted);

        let on_health_event = std::rc::Rc::clone(&on_event);
        let needs_audio = audio_seen.is_some();
        let retry_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let retry_pipeline = pipeline.clone();
        let health_check_cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_timer = Arc::clone(&health_check_cancel);
        glib::timeout_add_local(STARTUP_RETRY_INTERVAL, move || {
            if cancel_for_timer.load(Ordering::Relaxed) {
                return glib::ControlFlow::Break;
            }

            let waiting_for_video = !video_seen.load(Ordering::Relaxed);
            let waiting_for_audio = audio_seen
                .as_ref()
                .map(|seen| !seen.load(Ordering::Relaxed))
                .unwrap_or(false);

            if !waiting_for_video && (!needs_audio || !waiting_for_audio) {
                return glib::ControlFlow::Break;
            }

            if waiting_for_video {
                let attempt = retry_count.fetch_add(1, Ordering::Relaxed) + 1;
                if attempt > STARTUP_RETRY_LIMIT {
                    on_health_event(CaptureEvent::NoFrames {
                        waiting_for_video,
                        waiting_for_audio,
                    });
                    return glib::ControlFlow::Continue;
                }

                on_health_event(CaptureEvent::Retrying(format!(
                    "Warming up the ShadowCast stream (attempt {attempt}/{STARTUP_RETRY_LIMIT})."
                )));
                let _ = retry_pipeline.set_state(gst::State::Ready);
                let _ = retry_pipeline.set_state(gst::State::Playing);
            } else if needs_audio && waiting_for_audio {
                on_health_event(CaptureEvent::NoFrames {
                    waiting_for_video,
                    waiting_for_audio,
                });
            }

            glib::ControlFlow::Continue
        });
        self.health_check_cancel = Some(health_check_cancel);

        self.pipeline = Some(pipeline);
        Ok(paintable)
    }

    pub fn stop(&mut self) {
        if let Some(cancel) = self.health_check_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }

        if let Some(cancel) = self.stats_report_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }

        if let Some(bus_watch) = self.bus_watch.take() {
            drop(bus_watch);
        }

        if let Some(pipeline) = self.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
    }
}

impl Drop for CaptureController {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build_pipeline_description(settings: &CaptureSettings) -> String {
    let video_caps = video_caps(settings);
    let video = format!(
        "v4l2src name=video_source device={} do-timestamp=true ! {} ! queue max-size-buffers=2 leaky=downstream ! {} ! videoconvert ! identity name=video_probe silent=true ! gtk4paintablesink name=video_sink sync=false",
        gst_quote(&settings.video_device),
        video_caps,
        decode_chain_for(&settings.pixel_format),
    );

    match settings.audio_device.as_deref() {
        Some(audio_device) => format!("{video} {}", audio_pipeline(audio_device)),
        None => video,
    }
}

fn video_caps(settings: &CaptureSettings) -> String {
    let media_type = match settings.pixel_format.as_str() {
        "MJPG" | "MJPEG" => "image/jpeg".to_string(),
        "YUYV" => "video/x-raw,format=YUY2".to_string(),
        "BGR3" => "video/x-raw,format=BGR".to_string(),
        "NV12" => "video/x-raw,format=NV12".to_string(),
        format => format!("video/x-raw,format={format}"),
    };

    format!(
        "{media_type},width={},height={},framerate={}/1",
        settings.width, settings.height, settings.fps
    )
}

fn decode_chain_for(pixel_format: &str) -> &'static str {
    match pixel_format {
        "MJPG" | "MJPEG" => "jpegparse ! jpegdec",
        _ => "identity",
    }
}

fn audio_pipeline(audio_device: &str) -> String {
    if let Some(source_name) = audio_device.strip_prefix("pulse:") {
        return format!(
            "pulsesrc name=audio_source device={} do-timestamp=true ! queue max-size-buffers=8 leaky=downstream ! audioconvert ! audioresample ! identity name=audio_probe silent=true ! autoaudiosink sync=false",
            gst_quote(source_name)
        );
    }

    if let Some(alsa_device) = audio_device.strip_prefix("alsa:") {
        return format!(
            "alsasrc name=audio_source device={} do-timestamp=true ! queue max-size-buffers=8 leaky=downstream ! audioconvert ! audioresample ! identity name=audio_probe silent=true ! autoaudiosink sync=false",
            gst_quote(alsa_device)
        );
    }

    "autoaudiosrc name=audio_source ! queue max-size-buffers=8 leaky=downstream ! audioconvert ! audioresample ! identity name=audio_probe silent=true ! autoaudiosink sync=false".to_string()
}

fn gst_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn attach_first_buffer_probe(
    pipeline: &gst::Pipeline,
    element_name: &str,
    message_kind: &'static str,
) -> Arc<AtomicBool> {
    let seen = Arc::new(AtomicBool::new(false));
    let Some(element) = pipeline.by_name(element_name) else {
        return seen;
    };
    let Some(pad) = element.static_pad("src") else {
        return seen;
    };

    let seen_for_probe = Arc::clone(&seen);
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, _| {
        if !seen_for_probe.swap(true, Ordering::Relaxed) {
            if let Some(parent) = pad.parent_element() {
                let structure = gst::Structure::builder("shadowcatch-status")
                    .field("kind", message_kind)
                    .build();
                let message = gst::message::Application::builder(structure)
                    .src(&parent)
                    .build();
                let _ = parent.post_message(message);
            }
        }

        gst::PadProbeReturn::Ok
    });

    seen
}

#[derive(Default)]
struct DebugCounters {
    video_buffers: AtomicU64,
    video_bytes: AtomicU64,
    audio_buffers: AtomicU64,
    audio_bytes: AtomicU64,
}

fn start_debug_stats(
    pipeline: &gst::Pipeline,
    has_audio: bool,
    on_event: std::rc::Rc<dyn Fn(CaptureEvent)>,
) -> Arc<AtomicBool> {
    let counters = Arc::new(DebugCounters::default());
    attach_debug_stats_probe(
        pipeline,
        "video_source",
        StreamKind::Video,
        Arc::clone(&counters),
    );

    if has_audio {
        attach_debug_stats_probe(
            pipeline,
            "audio_source",
            StreamKind::Audio,
            Arc::clone(&counters),
        );
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_timer = Arc::clone(&cancel);
    let started_at = Instant::now();
    let mut last_video_buffers = 0;
    let mut last_video_bytes = 0;
    let mut last_audio_buffers = 0;
    let mut last_audio_bytes = 0;

    glib::timeout_add_seconds_local(1, move || {
        if cancel_for_timer.load(Ordering::Relaxed) {
            return glib::ControlFlow::Break;
        }

        let video_buffers = counters.video_buffers.load(Ordering::Relaxed);
        let video_bytes = counters.video_bytes.load(Ordering::Relaxed);
        let audio_buffers = counters.audio_buffers.load(Ordering::Relaxed);
        let audio_bytes = counters.audio_bytes.load(Ordering::Relaxed);

        let video_buffers_per_second = video_buffers.saturating_sub(last_video_buffers);
        let video_bytes_per_second = video_bytes.saturating_sub(last_video_bytes);
        let audio_buffers_per_second = audio_buffers.saturating_sub(last_audio_buffers);
        let audio_bytes_per_second = audio_bytes.saturating_sub(last_audio_bytes);

        last_video_buffers = video_buffers;
        last_video_bytes = video_bytes;
        last_audio_buffers = audio_buffers;
        last_audio_bytes = audio_bytes;

        let stats = if has_audio {
            format!(
                "up {} | video {} fps, {}, {} total | audio {} buf/s, {}, {} total",
                format_uptime(started_at.elapsed()),
                video_buffers_per_second,
                format_bitrate(video_bytes_per_second),
                format_bytes(video_bytes),
                audio_buffers_per_second,
                format_bitrate(audio_bytes_per_second),
                format_bytes(audio_bytes),
            )
        } else {
            format!(
                "up {} | video {} fps, {}, {} total",
                format_uptime(started_at.elapsed()),
                video_buffers_per_second,
                format_bitrate(video_bytes_per_second),
                format_bytes(video_bytes),
            )
        };

        on_event(CaptureEvent::DebugStats(stats));

        glib::ControlFlow::Continue
    });

    cancel
}

#[derive(Clone, Copy)]
enum StreamKind {
    Video,
    Audio,
}

fn attach_debug_stats_probe(
    pipeline: &gst::Pipeline,
    element_name: &str,
    stream_kind: StreamKind,
    counters: Arc<DebugCounters>,
) {
    let Some(element) = pipeline.by_name(element_name) else {
        return;
    };
    let Some(pad) = element.static_pad("src") else {
        return;
    };

    pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        let Some(buffer) = info.buffer() else {
            return gst::PadProbeReturn::Ok;
        };
        let bytes = buffer.size() as u64;

        match stream_kind {
            StreamKind::Video => {
                counters.video_buffers.fetch_add(1, Ordering::Relaxed);
                counters.video_bytes.fetch_add(bytes, Ordering::Relaxed);
            }
            StreamKind::Audio => {
                counters.audio_buffers.fetch_add(1, Ordering::Relaxed);
                counters.audio_bytes.fetch_add(bytes, Ordering::Relaxed);
            }
        }

        gst::PadProbeReturn::Ok
    });
}

fn format_uptime(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn format_bitrate(bytes_per_second: u64) -> String {
    let bits_per_second = bytes_per_second as f64 * 8.0;

    if bits_per_second >= 1_000_000.0 {
        format!("{:.2} Mb/s", bits_per_second / 1_000_000.0)
    } else if bits_per_second >= 1_000.0 {
        format!("{:.1} Kb/s", bits_per_second / 1_000.0)
    } else {
        format!("{bits_per_second:.0} b/s")
    }
}

fn format_bytes(bytes: u64) -> String {
    let bytes = bytes as f64;

    if bytes >= 1_000_000_000.0 {
        format!("{:.2} GB", bytes / 1_000_000_000.0)
    } else if bytes >= 1_000_000.0 {
        format!("{:.2} MB", bytes / 1_000_000.0)
    } else if bytes >= 1_000.0 {
        format!("{:.1} KB", bytes / 1_000.0)
    } else {
        format!("{bytes:.0} B")
    }
}
