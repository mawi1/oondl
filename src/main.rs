mod downloader;
mod gui;

use eframe::NativeOptions;
use egui::{Style, Visuals};
use gui::OondlApp;
use single_instance::SingleInstance;

fn main() -> eframe::Result<()> {
    const APP_NAME: &str = "oondl";
    env_logger::init();

    let s = SingleInstance::new(APP_NAME).unwrap();
    if !s.is_single() {
        log::warn!("another instance is already running");
        return Ok(());
    }

    let native_options = NativeOptions {
        persist_window: false,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0, 550.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(|cc| {
            let style = Style {
                visuals: Visuals::dark(),
                ..Style::default()
            };
            cc.egui_ctx.set_style(style);

            let client = downloader::run(cc.egui_ctx.clone());
            Box::new(OondlApp::new(cc, client))
        }),
    )
}
