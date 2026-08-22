// Launch as a GUI app on Windows so no console/cmd window pops up.
#![cfg_attr(windows, windows_subsystem = "windows")]

use android_ui_viewer::app;
use android_ui_viewer::log::{self, LevelFilter};

use eframe::egui;

fn main() -> eframe::Result<()> {
    // Start file logging before anything else so startup issues are captured.
    log::init(LevelFilter::Debug);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Android UI Viewer",
        options,
        Box::new(|cc| {
            app::UiViewerApp::setup_fonts(&cc.egui_ctx);
            Box::new(app::UiViewerApp::new())
        }),
    )
}
