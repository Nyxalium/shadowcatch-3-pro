use std::collections::BTreeSet;
use std::process::Command;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CaptureFormat {
    pub pixel_format: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl CaptureFormat {
    pub fn default_shadowcast() -> Self {
        Self {
            pixel_format: "MJPG".to_string(),
            width: 1920,
            height: 1080,
            fps: 60,
        }
    }
}

pub fn list_formats(video_device: &str) -> Vec<CaptureFormat> {
    let Ok(output) = Command::new("v4l2-ctl")
        .args(["--list-formats-ext", "-d", video_device])
        .output()
    else {
        return vec![CaptureFormat::default_shadowcast()];
    };

    if !output.status.success() {
        return vec![CaptureFormat::default_shadowcast()];
    }

    let parsed = parse_v4l2_formats(&String::from_utf8_lossy(&output.stdout));
    if parsed.is_empty() {
        vec![CaptureFormat::default_shadowcast()]
    } else {
        parsed
    }
}

pub fn unique_pixel_formats(formats: &[CaptureFormat]) -> Vec<String> {
    formats
        .iter()
        .map(|format| format.pixel_format.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn unique_resolutions(formats: &[CaptureFormat], pixel_format: &str) -> Vec<(u32, u32)> {
    formats
        .iter()
        .filter(|format| format.pixel_format == pixel_format)
        .map(|format| (format.width, format.height))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn frame_rates_for(
    formats: &[CaptureFormat],
    pixel_format: &str,
    width: u32,
    height: u32,
) -> Vec<u32> {
    formats
        .iter()
        .filter(|format| {
            format.pixel_format == pixel_format && format.width == width && format.height == height
        })
        .map(|format| format.fps)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn parse_v4l2_formats(output: &str) -> Vec<CaptureFormat> {
    let mut formats = BTreeSet::new();
    let mut current_pixel_format: Option<String> = None;
    let mut current_size: Option<(u32, u32)> = None;

    for line in output.lines() {
        let trimmed = line.trim();

        if let Some(pixel_format) = parse_pixel_format(trimmed) {
            current_pixel_format = Some(pixel_format);
            current_size = None;
            continue;
        }

        if let Some(size) = parse_size(trimmed) {
            current_size = Some(size);
            continue;
        }

        if let (Some(pixel_format), Some((width, height)), Some(fps)) = (
            current_pixel_format.as_ref(),
            current_size,
            parse_fps(trimmed),
        ) {
            formats.insert(CaptureFormat {
                pixel_format: pixel_format.clone(),
                width,
                height,
                fps,
            });
        }
    }

    formats.into_iter().collect()
}

fn parse_pixel_format(line: &str) -> Option<String> {
    let start = line.find('\'')? + 1;
    let end = line[start..].find('\'')? + start;
    Some(line[start..end].to_string())
}

fn parse_size(line: &str) -> Option<(u32, u32)> {
    let size = line.strip_prefix("Size: Discrete ")?;
    let (width, height) = size.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn parse_fps(line: &str) -> Option<u32> {
    let fps_text = line.split('(').nth(1)?.split_whitespace().next()?;
    let fps = fps_text.parse::<f32>().ok()?;
    Some(fps.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v4l2_ctl_output() {
        let output = r#"
            [0]: 'MJPG' (Motion-JPEG, compressed)
                Size: Discrete 1920x1080
                    Interval: Discrete 0.017s (60.000 fps)
                    Interval: Discrete 0.033s (30.000 fps)
            [1]: 'YUYV' (YUYV 4:2:2)
                Size: Discrete 1280x720
                    Interval: Discrete 0.033s (30.000 fps)
        "#;

        let formats = parse_v4l2_formats(output);
        assert!(formats.contains(&CaptureFormat {
            pixel_format: "MJPG".to_string(),
            width: 1920,
            height: 1080,
            fps: 60,
        }));
        assert!(formats.contains(&CaptureFormat {
            pixel_format: "YUYV".to_string(),
            width: 1280,
            height: 720,
            fps: 30,
        }));
    }
}
