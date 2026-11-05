//! Background work: analysing (dry run), processing, and preview decoding.
//! Results come back over a channel; the UI never blocks.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use eframe::egui::{self, ColorImage};
use trim_core::batch::{self, Options, Report};
use trim_core::codec;

/// Longest side of preview images, in pixels.
const PREVIEW_MAX: u32 = 1400;

pub enum Msg {
    AnalysisProgress {
        generation: u64,
        done: usize,
        total: usize,
    },
    Analysis {
        generation: u64,
        result: Result<Report, String>,
    },
    RunProgress {
        done: usize,
        total: usize,
    },
    RunDone(Result<Report, String>),
    Preview {
        path: PathBuf,
        result: Result<Preview, String>,
    },
}

pub struct Preview {
    pub image: ColorImage,
    /// Size of the full (EXIF-oriented) image, which crop rectangles refer to.
    pub full_size: [u32; 2],
}

pub fn analyse(ctx: &egui::Context, tx: &Sender<Msg>, generation: u64, mut opts: Options) {
    opts.dry_run = true;
    let (ctx, tx) = (ctx.clone(), tx.clone());
    thread::spawn(move || {
        let result = batch::run_with_progress(&opts, |p| {
            let _ = tx.send(Msg::AnalysisProgress {
                generation,
                done: p.done,
                total: p.total,
            });
            ctx.request_repaint();
        })
        .map_err(|e| format!("{e:#}"));
        let _ = tx.send(Msg::Analysis { generation, result });
        ctx.request_repaint();
    });
}

pub fn process(ctx: &egui::Context, tx: &Sender<Msg>, mut opts: Options) {
    opts.dry_run = false;
    let (ctx, tx) = (ctx.clone(), tx.clone());
    thread::spawn(move || {
        let result = batch::run_with_progress(&opts, |p| {
            let _ = tx.send(Msg::RunProgress {
                done: p.done,
                total: p.total,
            });
            ctx.request_repaint();
        })
        .map_err(|e| format!("{e:#}"));
        let _ = tx.send(Msg::RunDone(result));
        ctx.request_repaint();
    });
}

pub fn load_preview(ctx: &egui::Context, tx: &Sender<Msg>, path: PathBuf) {
    let (ctx, tx) = (ctx.clone(), tx.clone());
    thread::spawn(move || {
        let result = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| codec::decode(&bytes).map_err(|e| format!("{e:#}")))
            .map(|decoded| {
                let full_size = [decoded.image.width(), decoded.image.height()];
                let small = if full_size[0].max(full_size[1]) > PREVIEW_MAX {
                    decoded.image.thumbnail(PREVIEW_MAX, PREVIEW_MAX)
                } else {
                    decoded.image
                };
                let rgba = small.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                Preview {
                    image: ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    full_size,
                }
            });
        let _ = tx.send(Msg::Preview { path, result });
        ctx.request_repaint();
    });
}

/// Open a folder in the system file manager.
pub fn open_folder(path: &std::path::Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}
