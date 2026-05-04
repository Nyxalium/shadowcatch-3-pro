use crate::capture::{CaptureController, CaptureEvent, CaptureSettings};
use crate::devices;
use crate::formats::{self, CaptureFormat};
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SavedSettings {
    video_device: Option<String>,
    audio_device: Option<String>,
    pixel_format: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u32>,
}

pub fn build(app: &gtk::Application) {
    let saved = load_settings();
    let video_devices = devices::list_video_devices();
    let audio_devices = devices::list_audio_devices();
    let initial_video = preferred_video_device(&video_devices, &saved);
    let initial_formats = formats::list_formats(&initial_video);

    let controller = Rc::new(RefCell::new(CaptureController::new()));
    let available_formats = Rc::new(RefCell::new(initial_formats));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("ShadowCatch 3 Pro")
        .default_width(1280)
        .default_height(720)
        .build();

    let picture = gtk::Picture::builder()
        .hexpand(true)
        .vexpand(true)
        .can_shrink(true)
        .build();
    picture.set_content_fit(gtk::ContentFit::Contain);

    let status_label = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .label("")
        .build();
    status_label.add_css_class("dim-label");
    status_label.set_visible(false);
    let diagnostic_label = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .label("Starting capture...")
        .build();
    diagnostic_label.add_css_class("dim-label");
    diagnostic_label.set_margin_top(24);
    diagnostic_label.set_margin_bottom(24);
    diagnostic_label.set_margin_start(24);
    diagnostic_label.set_margin_end(24);

    let video_combo = gtk::ComboBoxText::new();
    let audio_combo = gtk::ComboBoxText::new();
    let format_combo = gtk::ComboBoxText::new();
    let resolution_combo = gtk::ComboBoxText::new();
    let fps_combo = gtk::ComboBoxText::new();
    let debug_switch = gtk::Switch::builder()
        .active(false)
        .valign(gtk::Align::Center)
        .build();
    let restart_button = gtk::Button::with_label("Apply");
    let settings_button = gtk::ToggleButton::with_label("Settings");

    for combo in [
        &video_combo,
        &audio_combo,
        &format_combo,
        &resolution_combo,
        &fps_combo,
    ] {
        combo.set_size_request(260, -1);
    }

    populate_video_devices(&video_combo, &video_devices, &initial_video);
    populate_audio_devices(&audio_combo, &audio_devices, saved.audio_device.as_deref());
    populate_format_controls(
        &format_combo,
        &resolution_combo,
        &fps_combo,
        &available_formats.borrow(),
        &saved,
    );

    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    settings_box.set_margin_top(12);
    settings_box.set_margin_bottom(12);
    settings_box.set_margin_start(12);
    settings_box.set_margin_end(12);
    settings_box.set_size_request(420, -1);

    let auto_label = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .label(auto_summary(
            &initial_video,
            active_id(&audio_combo).as_deref(),
        ))
        .build();
    settings_box.append(&auto_label);
    settings_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    settings_box.append(&setting_row("Video Device", &video_combo));
    settings_box.append(&setting_row("Audio Source", &audio_combo));
    settings_box.append(&setting_row("Input Format", &format_combo));
    settings_box.append(&setting_row("Resolution", &resolution_combo));
    settings_box.append(&setting_row("Frame Rate", &fps_combo));
    settings_box.append(&setting_row("Log Debug Info", &debug_switch));
    settings_box.append(&restart_button);

    let settings_frame = gtk::Frame::builder().child(&settings_box).build();
    settings_frame.add_css_class("view");

    let drawer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .transition_duration(180)
        .child(&settings_frame)
        .build();
    drawer.set_halign(gtk::Align::End);
    drawer.set_valign(gtk::Align::Fill);
    drawer.set_reveal_child(false);

    let header = gtk::HeaderBar::new();
    header.pack_end(&settings_button);
    window.set_titlebar(Some(&header));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&picture);
    content.append(&status_label);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&content));
    overlay.add_overlay(&diagnostic_label);
    overlay.add_overlay(&drawer);
    window.set_child(Some(&overlay));

    connect_format_refresh(
        &format_combo,
        &resolution_combo,
        &fps_combo,
        Rc::clone(&available_formats),
    );
    connect_resolution_refresh(
        &format_combo,
        &resolution_combo,
        &fps_combo,
        Rc::clone(&available_formats),
    );
    connect_video_refresh(
        &video_combo,
        &format_combo,
        &resolution_combo,
        &fps_combo,
        Rc::clone(&available_formats),
    );

    {
        let drawer = drawer.clone();
        settings_button.connect_toggled(move |button| {
            drawer.set_reveal_child(button.is_active());
        });
    }

    {
        let auto_label = auto_label.clone();
        let video_combo = video_combo.clone();
        let audio_combo = audio_combo.clone();
        video_combo.connect_changed(move |video_combo| {
            auto_label.set_label(&auto_summary(
                &active_id(video_combo).unwrap_or_else(|| "/dev/video0".to_string()),
                active_id(&audio_combo).as_deref(),
            ));
        });
    }

    {
        let auto_label = auto_label.clone();
        let video_combo = video_combo.clone();
        audio_combo.connect_changed(move |audio_combo| {
            auto_label.set_label(&auto_summary(
                &active_id(&video_combo).unwrap_or_else(|| "/dev/video0".to_string()),
                active_id(audio_combo).as_deref(),
            ));
        });
    }

    let start_capture = Rc::new({
        let controller = Rc::clone(&controller);
        let picture = picture.clone();
        let status_label = status_label.clone();
        let diagnostic_label = diagnostic_label.clone();
        let video_combo = video_combo.clone();
        let audio_combo = audio_combo.clone();
        let format_combo = format_combo.clone();
        let resolution_combo = resolution_combo.clone();
        let fps_combo = fps_combo.clone();
        let debug_switch = debug_switch.clone();

        move || {
            let Some(settings) = capture_settings_from_widgets(
                &video_combo,
                &audio_combo,
                &format_combo,
                &resolution_combo,
                &fps_combo,
                &debug_switch,
            ) else {
                status_label.set_label("Choose a video device before starting capture.");
                return;
            };

            status_label.set_visible(settings.debug_stats);
            if settings.debug_stats {
                status_label.set_label("Waiting for debug stats...");
            } else {
                status_label.set_label("");
            }

            let pipeline_status = status_label.clone();
            let pipeline_diagnostics = diagnostic_label.clone();
            match controller.borrow_mut().start(&settings, move |event| {
                update_capture_status(&pipeline_status, &pipeline_diagnostics, event);
            }) {
                Ok(paintable) => {
                    picture.set_paintable(Some(&paintable));
                    diagnostic_label.set_visible(true);
                    diagnostic_label.set_label("Waiting for the first video frame...");
                    save_settings(&settings);
                }
                Err(err) => {
                    status_label.set_visible(false);
                    status_label.set_label("");
                    diagnostic_label.set_visible(true);
                    diagnostic_label.set_label(&format!(
                        "Could not start capture. Check that HDMI is connected and no other app is using the device.\n{err:#}"
                    ));
                }
            }
        }
    });

    {
        let start_capture = Rc::clone(&start_capture);
        let settings_button = settings_button.clone();
        restart_button.connect_clicked(move |_| {
            start_capture();
            settings_button.set_active(false);
        });
    }

    window.connect_close_request(move |_| {
        controller.borrow_mut().stop();
        gtk::glib::Propagation::Proceed
    });

    window.present();
    start_capture();
}

fn setting_row(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_hexpand(true);

    let text = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();

    row.append(&text);
    row.append(widget);
    row
}

fn update_capture_status(
    status_label: &gtk::Label,
    diagnostic_label: &gtk::Label,
    event: CaptureEvent,
) {
    match event {
        CaptureEvent::PipelineStarted => {
            diagnostic_label.set_visible(true);
            diagnostic_label.set_label("Pipeline started. Waiting for video/audio buffers...");
        }
        CaptureEvent::Retrying(message) => {
            diagnostic_label.set_visible(true);
            diagnostic_label.set_label(&message);
        }
        CaptureEvent::VideoFrame => {
            diagnostic_label.set_visible(false);
        }
        CaptureEvent::AudioFrame => {}
        CaptureEvent::DebugStats(stats) => {
            status_label.set_visible(true);
            status_label.set_label(&stats);
        }
        CaptureEvent::Warning(message) => {
            diagnostic_label.set_visible(true);
            diagnostic_label.set_label(&format!("Capture warning:\n{message}"));
        }
        CaptureEvent::Error(message) => {
            diagnostic_label.set_visible(true);
            diagnostic_label.set_label(&format!("Capture error:\n{message}"));
        }
        CaptureEvent::NoFrames {
            waiting_for_video,
            waiting_for_audio,
        } => {
            let mut waiting_for = Vec::new();
            if waiting_for_video {
                waiting_for.push("video");
            }
            if waiting_for_audio {
                waiting_for.push("audio");
            }

            diagnostic_label.set_visible(true);
            diagnostic_label.set_label(&format!(
                "The pipeline opened, but no {} buffers arrived after 2 seconds.\n\
                 This usually means the capture device accepted the selected mode but did not start streaming. \
                 ShadowCatch will retry the stream automatically.",
                waiting_for.join(" or ")
            ));
        }
    }
}

fn auto_summary(video_device: &str, audio_device: Option<&str>) -> String {
    let audio = match audio_device {
        Some("none") | None => "no audio source".to_string(),
        Some(device) if device.starts_with("pulse:") => "PipeWire/Pulse audio".to_string(),
        Some(device) if device.starts_with("alsa:") => "ALSA audio".to_string(),
        Some(device) => device.to_string(),
    };

    format!("Auto-selected {video_device} with {audio}. Open these controls only if the defaults need changing.")
}

fn populate_video_devices(
    combo: &gtk::ComboBoxText,
    devices: &[devices::VideoDevice],
    selected: &str,
) {
    combo.remove_all();

    if devices.is_empty() {
        combo.append(Some("/dev/video0"), "/dev/video0 (fallback)");
    } else {
        for device in devices {
            let marker = if device.is_shadowcast {
                "ShadowCast"
            } else {
                "V4L2"
            };
            combo.append(
                Some(&device.path),
                &format!("{} - {} ({marker})", device.path, device.name),
            );
        }
    }

    if !combo.set_active_id(Some(selected)) {
        combo.set_active(Some(0));
    }
}

fn populate_audio_devices(
    combo: &gtk::ComboBoxText,
    devices: &[devices::AudioDevice],
    selected: Option<&str>,
) {
    combo.remove_all();
    combo.append(Some("none"), "No audio");

    for device in devices {
        combo.append(Some(&device.id), &device.name);
    }

    let preferred_shadowcast_pipewire = devices
        .iter()
        .find(|device| device.is_shadowcast && device.backend == devices::AudioBackend::Pulse);

    if let Some(device) = preferred_shadowcast_pipewire {
        combo.set_active_id(Some(&device.id));
        return;
    }

    if selected
        .map(|selected| combo.set_active_id(Some(selected)))
        .unwrap_or(false)
    {
        return;
    }

    if let Some(device) = devices.iter().find(|device| device.is_shadowcast) {
        combo.set_active_id(Some(&device.id));
    } else {
        combo.set_active(Some(0));
    }
}

fn populate_format_controls(
    format_combo: &gtk::ComboBoxText,
    resolution_combo: &gtk::ComboBoxText,
    fps_combo: &gtk::ComboBoxText,
    available_formats: &[CaptureFormat],
    saved: &SavedSettings,
) {
    format_combo.remove_all();
    for pixel_format in formats::unique_pixel_formats(available_formats) {
        format_combo.append(Some(&pixel_format), &pixel_format);
    }

    let preferred_format = saved.pixel_format.as_deref().unwrap_or("MJPG");
    if !format_combo.set_active_id(Some(preferred_format)) {
        format_combo.set_active(Some(0));
    }

    refresh_resolutions(
        format_combo,
        resolution_combo,
        fps_combo,
        available_formats,
        saved,
    );
}

fn refresh_resolutions(
    format_combo: &gtk::ComboBoxText,
    resolution_combo: &gtk::ComboBoxText,
    fps_combo: &gtk::ComboBoxText,
    available_formats: &[CaptureFormat],
    saved: &SavedSettings,
) {
    let pixel_format = active_id(format_combo).unwrap_or_else(|| "MJPG".to_string());
    resolution_combo.remove_all();

    for (width, height) in formats::unique_resolutions(available_formats, &pixel_format) {
        let id = format!("{width}x{height}");
        resolution_combo.append(Some(&id), &id);
    }

    let preferred_resolution = match (saved.width, saved.height) {
        (Some(width), Some(height)) => format!("{width}x{height}"),
        _ => "1920x1080".to_string(),
    };

    if !resolution_combo.set_active_id(Some(&preferred_resolution)) {
        resolution_combo.set_active(Some(0));
    }

    refresh_frame_rates(
        format_combo,
        resolution_combo,
        fps_combo,
        available_formats,
        saved,
    );
}

fn refresh_frame_rates(
    format_combo: &gtk::ComboBoxText,
    resolution_combo: &gtk::ComboBoxText,
    fps_combo: &gtk::ComboBoxText,
    available_formats: &[CaptureFormat],
    saved: &SavedSettings,
) {
    let pixel_format = active_id(format_combo).unwrap_or_else(|| "MJPG".to_string());
    let (width, height) = selected_resolution(resolution_combo).unwrap_or((1920, 1080));
    fps_combo.remove_all();

    for fps in formats::frame_rates_for(available_formats, &pixel_format, width, height) {
        let id = fps.to_string();
        fps_combo.append(Some(&id), &format!("{fps} fps"));
    }

    let preferred_fps = saved.fps.unwrap_or(60).to_string();
    if !fps_combo.set_active_id(Some(&preferred_fps)) {
        fps_combo.set_active(Some(0));
    }
}

fn connect_format_refresh(
    format_combo: &gtk::ComboBoxText,
    resolution_combo: &gtk::ComboBoxText,
    fps_combo: &gtk::ComboBoxText,
    available_formats: Rc<RefCell<Vec<CaptureFormat>>>,
) {
    let resolution_combo = resolution_combo.clone();
    let fps_combo = fps_combo.clone();
    format_combo.connect_changed(move |format_combo| {
        refresh_resolutions(
            format_combo,
            &resolution_combo,
            &fps_combo,
            &available_formats.borrow(),
            &SavedSettings::default(),
        );
    });
}

fn connect_resolution_refresh(
    format_combo: &gtk::ComboBoxText,
    resolution_combo: &gtk::ComboBoxText,
    fps_combo: &gtk::ComboBoxText,
    available_formats: Rc<RefCell<Vec<CaptureFormat>>>,
) {
    let format_combo = format_combo.clone();
    let fps_combo = fps_combo.clone();
    resolution_combo.connect_changed(move |resolution_combo| {
        refresh_frame_rates(
            &format_combo,
            resolution_combo,
            &fps_combo,
            &available_formats.borrow(),
            &SavedSettings::default(),
        );
    });
}

fn connect_video_refresh(
    video_combo: &gtk::ComboBoxText,
    format_combo: &gtk::ComboBoxText,
    resolution_combo: &gtk::ComboBoxText,
    fps_combo: &gtk::ComboBoxText,
    available_formats: Rc<RefCell<Vec<CaptureFormat>>>,
) {
    let format_combo = format_combo.clone();
    let resolution_combo = resolution_combo.clone();
    let fps_combo = fps_combo.clone();

    video_combo.connect_changed(move |video_combo| {
        let Some(video_device) = active_id(video_combo) else {
            return;
        };

        *available_formats.borrow_mut() = formats::list_formats(&video_device);
        populate_format_controls(
            &format_combo,
            &resolution_combo,
            &fps_combo,
            &available_formats.borrow(),
            &SavedSettings::default(),
        );
    });
}

fn capture_settings_from_widgets(
    video_combo: &gtk::ComboBoxText,
    audio_combo: &gtk::ComboBoxText,
    format_combo: &gtk::ComboBoxText,
    resolution_combo: &gtk::ComboBoxText,
    fps_combo: &gtk::ComboBoxText,
    debug_switch: &gtk::Switch,
) -> Option<CaptureSettings> {
    let video_device = active_id(video_combo)?;
    let audio_device = active_id(audio_combo).filter(|id| id != "none");
    let pixel_format = active_id(format_combo).unwrap_or_else(|| "MJPG".to_string());
    let (width, height) = selected_resolution(resolution_combo).unwrap_or((1920, 1080));
    let fps = active_id(fps_combo)
        .and_then(|fps| fps.parse::<u32>().ok())
        .unwrap_or(60);

    Some(CaptureSettings {
        video_device,
        audio_device,
        pixel_format,
        width,
        height,
        fps,
        debug_stats: debug_switch.is_active(),
    })
}

fn active_id(combo: &gtk::ComboBoxText) -> Option<String> {
    combo.active_id().map(|value| value.to_string())
}

fn selected_resolution(combo: &gtk::ComboBoxText) -> Option<(u32, u32)> {
    let active = active_id(combo)?;
    let (width, height) = active.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn preferred_video_device(devices: &[devices::VideoDevice], saved: &SavedSettings) -> String {
    if let Some(saved_device) = saved.video_device.as_deref() {
        if devices.iter().any(|device| device.path == saved_device) {
            return saved_device.to_string();
        }
    }

    devices
        .iter()
        .find(|device| device.is_shadowcast)
        .or_else(|| devices.first())
        .map(|device| device.path.clone())
        .unwrap_or_else(|| "/dev/video0".to_string())
}

fn load_settings() -> SavedSettings {
    let Some(path) = settings_path() else {
        return SavedSettings::default();
    };

    fs::read_to_string(path)
        .ok()
        .and_then(|content| toml::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_settings(settings: &CaptureSettings) {
    let Some(path) = settings_path() else {
        return;
    };

    let saved = SavedSettings {
        video_device: Some(settings.video_device.clone()),
        audio_device: settings.audio_device.clone(),
        pixel_format: Some(settings.pixel_format.clone()),
        width: Some(settings.width),
        height: Some(settings.height),
        fps: Some(settings.fps),
    };

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(content) = toml::to_string_pretty(&saved) {
        let _ = fs::write(path, content);
    }
}

fn settings_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("shadowcatch-3-pro")
            .join("settings.toml"),
    )
}
