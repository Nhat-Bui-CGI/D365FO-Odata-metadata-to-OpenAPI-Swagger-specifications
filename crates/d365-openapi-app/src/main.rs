// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod jobs;
mod model;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("D365FO → OpenAPI")
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([900.0, 560.0]),
        // Set D365_OPENAPI_RENDERER=glow on machines where the default (wgpu) can't start,
        // e.g. some remote desktops or VMs without GPU drivers.
        renderer: match std::env::var("D365_OPENAPI_RENDERER").as_deref() {
            Ok("glow") => eframe::Renderer::Glow,
            _ => eframe::Renderer::default(),
        },
        ..Default::default()
    };
    eframe::run_native(
        "D365FO OpenAPI",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
