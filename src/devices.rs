use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoDevice {
    pub path: String,
    pub name: String,
    pub is_shadowcast: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioBackend {
    Pulse,
    Alsa,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub backend: AudioBackend,
    pub is_shadowcast: bool,
}

pub fn list_video_devices() -> Vec<VideoDevice> {
    let mut devices = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/video4linux") else {
        return devices;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.starts_with("video") {
            continue;
        }

        let sys_path = entry.path();
        let name = read_trimmed(sys_path.join("name")).unwrap_or_else(|| file_name.clone());
        let path = format!("/dev/{file_name}");
        if !is_video_capture_node(&path) {
            continue;
        }

        let is_shadowcast = looks_like_shadowcast(&name);

        devices.push(VideoDevice {
            path,
            name,
            is_shadowcast,
        });
    }

    devices.sort_by(|left, right| {
        right
            .is_shadowcast
            .cmp(&left.is_shadowcast)
            .then_with(|| left.path.cmp(&right.path))
    });
    devices
}

pub fn list_audio_devices() -> Vec<AudioDevice> {
    let mut devices = list_pulse_sources();
    devices.extend(list_alsa_capture_devices());
    devices.sort_by(|left, right| {
        right
            .is_shadowcast
            .cmp(&left.is_shadowcast)
            .then_with(|| {
                audio_backend_rank(&left.backend).cmp(&audio_backend_rank(&right.backend))
            })
            .then_with(|| left.name.cmp(&right.name))
    });
    devices
}

fn is_video_capture_node(path: &str) -> bool {
    let Ok(output) = Command::new("v4l2-ctl")
        .args(["--all", "-d", path])
        .output()
    else {
        return true;
    };

    if !output.status.success() {
        return false;
    }

    let output = String::from_utf8_lossy(&output.stdout);
    let Some(device_caps) = output.split("Device Caps").nth(1) else {
        return output.contains("Video Capture");
    };

    device_caps.contains("Video Capture")
}

fn audio_backend_rank(backend: &AudioBackend) -> u8 {
    match backend {
        AudioBackend::Pulse => 0,
        AudioBackend::Alsa => 1,
    }
}

fn list_pulse_sources() -> Vec<AudioDevice> {
    let Ok(output) = Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
    else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut columns = line.split('\t');
            let _index = columns.next()?;
            let source_name = columns.next()?.to_string();
            let driver = columns.next().unwrap_or_default();
            let label = format!("{source_name} ({driver})");

            Some(AudioDevice {
                id: format!("pulse:{source_name}"),
                name: label,
                backend: AudioBackend::Pulse,
                is_shadowcast: looks_like_shadowcast(&source_name),
            })
        })
        .collect()
}

fn list_alsa_capture_devices() -> Vec<AudioDevice> {
    let Ok(output) = Command::new("arecord").arg("-l").output() else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_arecord_line)
        .collect()
}

fn parse_arecord_line(line: &str) -> Option<AudioDevice> {
    let line = line.trim();
    if !line.starts_with("card ") {
        return None;
    }

    let (card_part, rest) = line.split_once(':')?;
    let card = card_part.strip_prefix("card ")?.split_whitespace().next()?;
    let device = rest.split("device ").nth(1)?.split(':').next()?.trim();
    let name = rest.trim().to_string();

    Some(AudioDevice {
        id: format!("alsa:hw:{card},{device}"),
        name: format!("{name} [hw:{card},{device}]"),
        backend: AudioBackend::Alsa,
        is_shadowcast: looks_like_shadowcast(rest),
    })
}

fn looks_like_shadowcast(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("shadowcast") || value.contains("shadow cast") || value.contains("s3")
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
