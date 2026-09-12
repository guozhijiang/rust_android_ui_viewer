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
            // 竖屏/窄窗可用:560pt 在 150% 缩放下也放得进 1080px 宽的竖屏
            // 显示器;面板宽度会随窗口自适应(见 app.rs 的响应式面板)。
            .with_min_inner_size([560.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Android UI Viewer",
        options,
        Box::new(|cc| {
            let scale = app::UiViewerApp::setup_fonts(&cc.egui_ctx);
            let mut viewer = app::UiViewerApp::new();
            viewer.set_ui_scale(scale);
            Ok(Box::new(viewer))
        }),
    )
}
