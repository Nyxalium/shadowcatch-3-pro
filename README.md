# ShadowCatch 3 Pro Viewer

ShadowCatch 3 Pro Viewer is a small Linux desktop app for viewing and listening to a ShadowCast 3 Pro capture device without assembling an `mpv` command by hand.

The first target is Arch/CachyOS with GTK4, GStreamer, V4L2, and PipeWire/PulseAudio.

## Features

- Auto-selects likely ShadowCast video and audio devices.
- Opens a normal resizable GTK window.
- Defaults to MJPEG 1080p at 60 FPS.
- Lets you choose video device, audio source, resolution, frame rate, and input format.
- Persists the last selected settings under the XDG config directory.

## Runtime Dependencies

Install the Rust toolchain and the GTK/GStreamer runtime packages for your distro.

On Arch-like systems:

```sh
sudo pacman -S rust gtk4 gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugin-gtk4 v4l-utils
```

`gst-plugin-gtk4` provides `gtk4paintablesink`, which is used to render the capture feed inside the GTK window.

## Run

```sh
cargo run
```

Enable `Log Debug Info` in the settings panel to print lightweight once-per-second capture stats in the console.

If the app opens but the video stays black, verify the console HDMI cable is plugged into the orange ShadowCast core and that no other program is using the capture device.
