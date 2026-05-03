mod capture;
mod devices;
mod formats;
mod ui;

use gtk::prelude::*;

fn main() {
    // GTK 4 can default to Vulkan on some systems. The GTK/GStreamer paintable
    // path is more reliable on NVIDIA/Wayland with the GL renderer.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "gl");
    }

    if let Err(err) = gst::init() {
        eprintln!("Failed to initialize GStreamer: {err}");
        return;
    }

    let app = gtk::Application::builder()
        .application_id("dev.nyxalium.ShadowCatch3Pro")
        .build();

    app.connect_activate(ui::build);
    app.run();
}
