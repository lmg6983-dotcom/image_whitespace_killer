//! 도면 여백 자르기 — desktop app for trimming white margins from patent
//! drawings. Uses the same engine as the `trim-image` command-line tool.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod text;
mod theme;
mod worker;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("도면 여백 자르기")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(theme::app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "trim-image-gui",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
