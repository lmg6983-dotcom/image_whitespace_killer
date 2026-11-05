//! Application state and UI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Key, Layout, Modifiers, Rect, RichText,
    Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2, text::LayoutJob,
    vec2,
};
use serde::{Deserialize, Serialize};
use trim_core::batch::{self, Options, Report};
use trim_core::{Background, FileReport, Status, codec, naming};

use crate::text;
use crate::theme;
use crate::worker::{self, Msg};

const PREFS_KEY: &str = "prefs";
/// Wait this long after the last change before re-analysing.
const DEBOUNCE: Duration = Duration::from_millis(300);
const ROW_H: f32 = 54.0;

// ---------------------------------------------------------------------------
// Preferences (remembered between runs)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
enum Naming {
    Numbered,
    KeepNames,
    Custom,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
enum Bg {
    White,
    Black,
    Auto,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
struct Prefs {
    save_to_other: bool,
    out_dir: Option<PathBuf>,
    keep_originals: bool,
    naming: Naming,
    custom: String,
    start: u64,
    fuzz: f64,
    margin: u32,
    bg: Bg,
    quality: u8,
    overwrite: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            save_to_other: false,
            out_dir: None,
            keep_originals: false,
            naming: Naming::Numbered,
            custom: "도 {번호}".into(),
            start: 1,
            fuzz: 5.0,
            margin: 0,
            bg: Bg::White,
            quality: 95,
            overwrite: false,
        }
    }
}

impl Prefs {
    fn template(&self) -> String {
        match self.naming {
            Naming::Numbered => batch::DEFAULT_TEMPLATE.into(),
            Naming::KeepNames => "{stem}".into(),
            Naming::Custom => self
                .custom
                .replace("{번호", "{num")
                .replace("{이름", "{stem"),
        }
    }

    /// Originals are deleted when the batch succeeds.
    fn deletes_originals(&self) -> bool {
        !self.save_to_other && !self.keep_originals
    }

    fn reset_advanced(&mut self) {
        let d = Prefs::default();
        self.fuzz = d.fuzz;
        self.margin = d.margin;
        self.bg = d.bg;
        self.quality = d.quality;
        self.overwrite = d.overwrite;
    }
}

// ---------------------------------------------------------------------------

enum Phase {
    Editing,
    Confirming,
    Running { done: usize, total: usize },
    Done(Report),
}

enum PreviewState {
    Loading,
    Ready {
        texture: TextureHandle,
        full_size: [u32; 2],
    },
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Info,
    Warn,
    Error,
}

pub struct App {
    prefs: Prefs,
    last_prefs: Prefs,
    files: Vec<PathBuf>,
    selected: Option<usize>,
    phase: Phase,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Bumped on every change to files or settings.
    generation: u64,
    dirty_since: Option<Instant>,
    analysing: Option<(usize, usize)>,
    analysis: Option<Report>,
    analysis_generation: u64,
    fatal: Option<String>,
    previews: HashMap<PathBuf, PreviewState>,
    notice: Option<(Tone, String)>,
    scroll_to_selected: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::apply_style(&cc.egui_ctx);
        let prefs: Prefs = cc
            .storage
            .and_then(|s| eframe::get_value(s, PREFS_KEY))
            .unwrap_or_default();
        let (tx, rx) = channel();
        let mut app = Self {
            last_prefs: prefs.clone(),
            prefs,
            files: Vec::new(),
            selected: None,
            phase: Phase::Editing,
            tx,
            rx,
            generation: 0,
            dirty_since: None,
            analysing: None,
            analysis: None,
            analysis_generation: 0,
            fatal: None,
            previews: HashMap::new(),
            notice: None,
            scroll_to_selected: false,
        };
        // Files or folders passed on the command line (e.g. "Open with…").
        let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !args.is_empty() {
            app.add_paths(args);
        }
        app
    }

    // ---- state changes ----------------------------------------------------

    fn mark_dirty(&mut self) {
        self.generation += 1;
        self.dirty_since = Some(Instant::now());
        self.fatal = None;
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let mut added = 0;
        let mut skipped = 0;
        for p in paths {
            let found = if p.is_dir() {
                batch::collect_inputs(std::slice::from_ref(&p)).unwrap_or_default()
            } else if batch::is_supported_image(&p) {
                batch::absolute(&p).map(|a| vec![a]).unwrap_or_default()
            } else {
                skipped += 1;
                continue;
            };
            for f in found {
                let key = batch::path_key(&f);
                if !self.files.iter().any(|e| batch::path_key(e) == key) {
                    self.files.push(f);
                    added += 1;
                }
            }
        }
        if skipped > 0 {
            self.notice = Some((
                Tone::Warn,
                format!(
                    "지원하지 않는 파일 {skipped}개는 건너뛰었습니다. ({})",
                    codec::EXTENSIONS.join(", ")
                ),
            ));
        } else if added == 0 {
            self.notice = Some((Tone::Info, "새로 추가된 이미지가 없습니다.".into()));
        } else {
            self.notice = None;
        }
        if added > 0 {
            if self.selected.is_none() {
                self.selected = Some(self.files.len() - added);
            }
            self.mark_dirty();
        }
    }

    fn remove_selected(&mut self) {
        if let Some(i) = self.selected
            && i < self.files.len()
        {
            self.files.remove(i);
            self.selected = if self.files.is_empty() {
                None
            } else {
                Some(i.min(self.files.len() - 1))
            };
            self.mark_dirty();
        }
    }

    fn move_selected(&mut self, delta: isize) {
        let Some(i) = self.selected else { return };
        let j = i as isize + delta;
        if j < 0 || j as usize >= self.files.len() {
            return;
        }
        self.files.swap(i, j as usize);
        self.selected = Some(j as usize);
        self.scroll_to_selected = true;
        self.mark_dirty();
    }

    fn sort_by_name(&mut self) {
        let sel = self.selected.map(|i| self.files[i].clone());
        self.files.sort_by(|a, b| {
            naming::natural_cmp(
                &a.file_name().unwrap_or_default().to_string_lossy(),
                &b.file_name().unwrap_or_default().to_string_lossy(),
            )
        });
        self.selected = sel.and_then(|s| self.files.iter().position(|f| *f == s));
        self.mark_dirty();
    }

    fn remove_failed(&mut self) {
        let Some(report) = &self.analysis else { return };
        let failed: Vec<String> = report
            .files
            .iter()
            .filter(|f| f.status == Status::Failed)
            .map(|f| f.input.clone())
            .collect();
        let sel = self.selected.map(|i| self.files[i].clone());
        self.files
            .retain(|p| !failed.contains(&p.display().to_string()));
        self.selected = sel
            .and_then(|s| self.files.iter().position(|f| *f == s))
            .or((!self.files.is_empty()).then_some(0));
        self.mark_dirty();
    }

    fn clear(&mut self) {
        self.files.clear();
        self.selected = None;
        self.analysis = None;
        self.previews.clear();
        self.phase = Phase::Editing;
        self.notice = None;
        self.mark_dirty();
    }

    fn options(&self) -> Options {
        let p = &self.prefs;
        Options {
            paths: self.files.clone(),
            fuzz: p.fuzz,
            background: match p.bg {
                Bg::White => Background::Color([255, 255, 255]),
                Bg::Black => Background::Color([0, 0, 0]),
                Bg::Auto => Background::Auto,
            },
            name_template: p.template(),
            start: p.start,
            out_dir: if p.save_to_other {
                p.out_dir.clone()
            } else {
                None
            },
            keep_originals: p.keep_originals,
            overwrite: p.overwrite,
            allow_partial: false,
            margin: p.margin,
            jpeg_quality: p.quality,
            dry_run: true,
        }
    }

    /// Analysis matches the current files and settings.
    fn analysis_fresh(&self) -> bool {
        self.dirty_since.is_none()
            && self.analysing.is_none()
            && self.analysis_generation == self.generation
    }

    /// Report row for file `i`, if it belongs to that file.
    fn row_report(&self, i: usize) -> Option<&FileReport> {
        let report = match &self.phase {
            Phase::Done(r) => r,
            _ => self.analysis.as_ref()?,
        };
        let f = report.files.get(i)?;
        (f.input == self.files.get(i)?.display().to_string()).then_some(f)
    }

    fn start_run(&mut self, ctx: &egui::Context) {
        self.phase = Phase::Running {
            done: 0,
            total: self.files.len(),
        };
        worker::process(ctx, &self.tx, self.options());
    }

    fn request_run(&mut self, ctx: &egui::Context) {
        if self.run_blocker().is_some() {
            return;
        }
        if self.prefs.deletes_originals() {
            self.phase = Phase::Confirming;
        } else {
            self.start_run(ctx);
        }
    }

    /// Why the run button is disabled, if it is.
    fn run_blocker(&self) -> Option<String> {
        if !matches!(self.phase, Phase::Editing) {
            return Some(String::new());
        }
        if self.files.is_empty() {
            return Some("먼저 이미지를 추가하세요.".into());
        }
        if self.prefs.save_to_other && self.prefs.out_dir.is_none() {
            return Some("저장할 폴더를 선택하세요.".into());
        }
        if let Some(f) = &self.fatal {
            return Some(f.clone());
        }
        if !self.analysis_fresh() {
            return Some(String::new());
        }
        let failed = self.analysis.as_ref().map_or(0, |r| r.summary.failed);
        if failed > 0 {
            return Some(format!(
                "오류가 있는 파일 {failed}개를 해결하거나 목록에서 빼야 실행할 수 있습니다."
            ));
        }
        None
    }

    fn result_folder(&self, report: &Report) -> Option<PathBuf> {
        if let Some(d) = &report.settings.out_dir {
            return Some(PathBuf::from(d));
        }
        report
            .files
            .iter()
            .find(|f| f.written)
            .or(report.files.first())
            .and_then(|f| Path::new(&f.output).parent().map(Path::to_path_buf))
    }

    // ---- background messages ------------------------------------------------

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::AnalysisProgress {
                    generation,
                    done,
                    total,
                } if generation == self.generation => self.analysing = Some((done, total)),
                Msg::AnalysisProgress { .. } => {}
                Msg::Analysis { generation, result } => {
                    if generation != self.generation {
                        continue;
                    }
                    self.analysing = None;
                    self.analysis_generation = generation;
                    match result {
                        Ok(r) => {
                            self.analysis = Some(r);
                            self.fatal = None;
                        }
                        Err(e) => self.fatal = Some(text::fatal(&e)),
                    }
                }
                Msg::RunProgress { done, total } => {
                    if let Phase::Running { .. } = self.phase {
                        self.phase = Phase::Running { done, total };
                    }
                }
                Msg::RunDone(Ok(report)) => {
                    self.previews.clear();
                    self.phase = Phase::Done(report);
                }
                Msg::RunDone(Err(e)) => {
                    self.phase = Phase::Editing;
                    self.notice = Some((Tone::Error, text::fatal(&e)));
                    self.mark_dirty();
                }
                Msg::Preview { path, result } => {
                    let state = match result {
                        Ok(p) => PreviewState::Ready {
                            texture: ctx.load_texture(
                                path.display().to_string(),
                                p.image,
                                TextureOptions::LINEAR,
                            ),
                            full_size: p.full_size,
                        },
                        Err(_) => PreviewState::Failed,
                    };
                    self.previews.insert(path, state);
                }
            }
        }

        // Debounced re-analysis.
        if let Some(t) = self.dirty_since {
            let wait = DEBOUNCE.saturating_sub(t.elapsed());
            if wait.is_zero() {
                self.dirty_since = None;
                if self.files.is_empty() {
                    self.analysis = None;
                    self.analysing = None;
                    self.analysis_generation = self.generation;
                } else {
                    self.analysing = Some((0, self.files.len()));
                    worker::analyse(ctx, &self.tx, self.generation, self.options());
                }
            } else {
                ctx.request_repaint_after(wait);
            }
        }
    }

    // ---- input ----------------------------------------------------------------

    fn handle_input(&mut self, ctx: &egui::Context) {
        let editing = matches!(self.phase, Phase::Editing);
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            if matches!(self.phase, Phase::Done(_)) {
                self.clear();
            }
            if matches!(self.phase, Phase::Editing) {
                self.add_paths(dropped);
            }
        }
        if !editing {
            return;
        }

        let cmd = |key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, key));
        if cmd(Key::O) {
            self.pick_files();
        }
        if cmd(Key::Enter) {
            self.request_run(ctx);
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let key = |m, k| ctx.input_mut(|i| i.consume_key(m, k));
        if key(Modifiers::ALT, Key::ArrowUp) {
            self.move_selected(-1);
        }
        if key(Modifiers::ALT, Key::ArrowDown) {
            self.move_selected(1);
        }
        if !self.files.is_empty() {
            if key(Modifiers::NONE, Key::ArrowUp) {
                self.selected = Some(self.selected.map_or(0, |i| i.saturating_sub(1)));
                self.scroll_to_selected = true;
            }
            if key(Modifiers::NONE, Key::ArrowDown) {
                let last = self.files.len() - 1;
                self.selected = Some(self.selected.map_or(0, |i| (i + 1).min(last)));
                self.scroll_to_selected = true;
            }
        }
        if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
            self.remove_selected();
        }
    }

    fn pick_files(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .set_title("도면 이미지 선택")
            .add_filter("이미지", codec::EXTENSIONS)
            .pick_files()
        {
            self.add_paths(files);
        }
    }

    fn pick_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("도면 이미지가 있는 폴더 선택")
            .pick_folder()
        {
            self.add_paths(vec![dir]);
        }
    }

    // ---- UI -------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut Ui) {
        egui::Panel::top("top")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("도면 여백 자르기").strong());
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "이미지의 흰 여백을 잘라내고 ‘도 1’, ‘도 2’… 순서로 이름을 붙입니다.",
                        )
                        .color(theme::muted(ui.visuals())),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                                .small()
                                .color(theme::muted(ui.visuals())),
                        );
                    });
                });
            });

        let Some((tone, msg)) = self.notice.clone() else {
            return;
        };
        let dark = ui.visuals().dark_mode;
        let color = match tone {
            Tone::Info => theme::accent(dark),
            Tone::Warn => theme::warning(dark),
            Tone::Error => theme::danger(dark),
        };
        egui::Panel::top("notice")
            .frame(
                egui::Frame::new()
                    .fill(theme::tint(color, dark))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(msg).color(color));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("닫기").clicked() {
                            self.notice = None;
                        }
                    });
                });
            });
    }

    fn settings_panel(&mut self, ui: &mut Ui) {
        let editing = matches!(self.phase, Phase::Editing);
        egui::Panel::right("settings")
            .exact_size(330.0)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(16))
            .show(ui, |ui| {
                egui::Panel::bottom("run")
                    .frame(egui::Frame::new().inner_margin(egui::Margin {
                        top: 12,
                        ..Default::default()
                    }))
                    .show(ui, |ui| self.run_box(ui));
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_enabled_ui(editing, |ui| self.settings_form(ui));
                });
            });
    }

    fn settings_form(&mut self, ui: &mut Ui) {
        let dark = ui.visuals().dark_mode;
        let muted = theme::muted(ui.visuals());
        let p = &mut self.prefs;

        section(ui, "저장 위치");
        ui.radio_value(&mut p.save_to_other, false, "원본과 같은 폴더");
        if !p.save_to_other {
            ui.indent("keep", |ui| {
                ui.checkbox(&mut p.keep_originals, "원본 파일도 남겨두기");
                if !p.keep_originals {
                    ui.label(
                        RichText::new("원본은 삭제되고 잘린 이미지로 바뀝니다.")
                            .small()
                            .color(theme::warning(dark)),
                    );
                }
            });
        }
        ui.radio_value(&mut p.save_to_other, true, "다른 폴더 (원본 유지)");
        if p.save_to_other {
            ui.indent("out", |ui| {
                match &p.out_dir {
                    Some(d) => {
                        ui.add(
                            egui::Label::new(RichText::new(d.display().to_string()).small())
                                .truncate(),
                        )
                        .on_hover_text(d.display().to_string());
                    }
                    None => {
                        ui.label(
                            RichText::new("아직 폴더를 고르지 않았습니다.")
                                .small()
                                .color(theme::warning(dark)),
                        );
                    }
                }
                if ui.button("폴더 선택…").clicked()
                    && let Some(d) = rfd::FileDialog::new()
                        .set_title("결과를 저장할 폴더")
                        .pick_folder()
                {
                    p.out_dir = Some(d);
                }
            });
        }

        section(ui, "파일 이름");
        ui.radio_value(&mut p.naming, Naming::Numbered, "도 1, 도 2, 도 3 …");
        ui.radio_value(&mut p.naming, Naming::KeepNames, "원래 이름 그대로");
        ui.radio_value(&mut p.naming, Naming::Custom, "직접 입력");
        if p.naming == Naming::Custom {
            ui.indent("custom", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut p.custom)
                        .hint_text("예: FIG {번호:02}")
                        .desired_width(f32::INFINITY),
                );
                ui.label(
                    RichText::new("{번호}  {번호:02}(01, 02 …)  {이름}(원래 이름)")
                        .small()
                        .color(muted),
                );
            });
        }
        if p.naming != Naming::KeepNames {
            ui.horizontal(|ui| {
                ui.label("시작 번호");
                ui.add(egui::DragValue::new(&mut p.start).range(0..=99_999));
            });
        }
        let example_stem = self
            .files
            .first()
            .and_then(|f| f.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "그림".into());
        let ext = self
            .files
            .first()
            .and_then(|f| f.extension())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "png".into());
        let example = naming::render(&p.template(), p.start, &example_stem)
            .map(|n| format!("예: {n}.{ext}"))
            .unwrap_or_else(|_| "이름 형식을 확인하세요.".into());
        ui.label(RichText::new(example).small().color(muted));

        ui.add_space(6.0);
        egui::CollapsingHeader::new(RichText::new("고급 설정").strong())
            .id_salt("advanced")
            .show(ui, |ui| {
                ui.label("흰색 허용 범위");
                ui.add(
                    egui::Slider::new(&mut p.fuzz, 0.0..=30.0)
                        .suffix(" %")
                        .step_by(0.5),
                );
                ui.label(
                    RichText::new("스캔한 도면에 얼룩이나 회색 테두리가 남으면 값을 올리세요.")
                        .small()
                        .color(muted),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("남길 여백");
                    ui.add(
                        egui::DragValue::new(&mut p.margin)
                            .range(0..=1000)
                            .suffix(" px"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("배경색");
                    egui::ComboBox::from_id_salt("bg")
                        .selected_text(match p.bg {
                            Bg::White => "흰색",
                            Bg::Black => "검은색",
                            Bg::Auto => "자동 (모서리 색)",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut p.bg, Bg::White, "흰색");
                            ui.selectable_value(&mut p.bg, Bg::Black, "검은색");
                            ui.selectable_value(&mut p.bg, Bg::Auto, "자동 (모서리 색)");
                        });
                });
                ui.label("JPEG 저장 품질");
                ui.add(egui::Slider::new(&mut p.quality, 50..=100));
                ui.checkbox(&mut p.overwrite, "같은 이름의 기존 파일 덮어쓰기");
                ui.add_space(4.0);
                if ui.button("기본값으로 되돌리기").clicked() {
                    p.reset_advanced();
                }
            });
    }

    fn run_box(&mut self, ui: &mut Ui) {
        let dark = ui.visuals().dark_mode;
        let muted = theme::muted(ui.visuals());

        if let Phase::Editing = self.phase {
            // What will happen.
            if let Some((done, total)) = self.analysing {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("이미지 확인 중… {done}/{total}"));
                });
            } else if self.analysis_fresh()
                && let Some(r) = &self.analysis
                && !self.files.is_empty()
            {
                let s = &r.summary;
                let mut parts = vec![format!("자를 이미지 {}개", s.trimmed)];
                if s.unchanged > 0 {
                    parts.push(format!("여백 없음 {}개", s.unchanged));
                }
                if s.blank > 0 {
                    parts.push(format!("빈 이미지 {}개", s.blank));
                }
                ui.label(parts.join(" · "));
                if s.failed > 0 {
                    ui.label(
                        RichText::new(format!("오류 {}개", s.failed)).color(theme::danger(dark)),
                    );
                }
            }
            let dest = if self.prefs.save_to_other {
                RichText::new("저장: 다른 폴더 · 원본 유지").color(muted)
            } else if self.prefs.keep_originals {
                RichText::new("저장: 같은 폴더 · 원본 유지").color(muted)
            } else {
                RichText::new("저장: 같은 폴더 · 원본 삭제").color(theme::warning(dark))
            };
            ui.label(dest.small());
        }

        if matches!(self.phase, Phase::Done(_)) {
            ui.label(
                RichText::new("새 이미지를 넣으려면 ‘새 작업’을 누르세요.")
                    .small()
                    .color(muted),
            );
            return;
        }
        let blocker = self.run_blocker();
        if let Some(msg) = &blocker
            && !msg.is_empty()
        {
            ui.label(RichText::new(msg).small().color(if self.fatal.is_some() {
                theme::danger(dark)
            } else {
                muted
            }));
            let failed = self.analysis_fresh()
                && self.analysis.as_ref().is_some_and(|r| r.summary.failed > 0);
            if failed
                && matches!(self.phase, Phase::Editing)
                && ui.button("오류 파일 목록에서 빼기").clicked()
            {
                self.remove_failed();
            }
        }

        let n = self.files.len();
        let label = if n == 0 {
            "여백 자르기".to_string()
        } else {
            format!("여백 자르기 ({n}개)")
        };
        let enabled = blocker.is_none();
        let button =
            egui::Button::new(RichText::new(label).strong().size(15.0).color(if enabled {
                Color32::WHITE
            } else {
                muted
            }))
            .fill(if enabled {
                theme::PRIMARY
            } else {
                ui.visuals().widgets.inactive.bg_fill
            })
            .min_size(vec2(ui.available_width(), 42.0));
        let resp = ui.add_enabled(enabled, button).on_hover_text("Ctrl+Enter");
        if resp.clicked() {
            self.request_run(ui.ctx());
        }
    }

    fn file_panel(&mut self, ui: &mut Ui) {
        egui::Panel::left("files")
            .default_size(420.0)
            .size_range(320.0..=700.0)
            .resizable(true)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 12)))
            .show(ui, |ui| {
                let editing = matches!(self.phase, Phase::Editing);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("이미지 {}개", self.files.len())).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_enabled_ui(editing, |ui| {
                            if ui.button("폴더 추가…").clicked() {
                                self.pick_folder();
                            }
                            if ui.button("파일 추가…").on_hover_text("Ctrl+O").clicked() {
                                self.pick_files();
                            }
                        });
                    });
                });
                ui.add_enabled_ui(editing, |ui| {
                    ui.horizontal(|ui| {
                        let has_sel = self.selected.is_some();
                        let n = self.files.len();
                        let sel = self.selected.unwrap_or(0);
                        if ui
                            .add_enabled(has_sel && sel > 0, egui::Button::new("위로"))
                            .on_hover_text("선택한 이미지를 앞으로 (Alt+↑)")
                            .clicked()
                        {
                            self.move_selected(-1);
                        }
                        if ui
                            .add_enabled(has_sel && sel + 1 < n, egui::Button::new("아래로"))
                            .on_hover_text("선택한 이미지를 뒤로 (Alt+↓)")
                            .clicked()
                        {
                            self.move_selected(1);
                        }
                        if ui
                            .button("이름순")
                            .on_hover_text("파일 이름순으로 정렬 (fig2 → fig10)")
                            .clicked()
                        {
                            self.sort_by_name();
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("모두 비우기").clicked() {
                                self.clear();
                            }
                            if ui
                                .add_enabled(has_sel, egui::Button::new("빼기"))
                                .on_hover_text("선택한 이미지를 목록에서 빼기 (Delete). 파일은 지워지지 않습니다.")
                                .clicked()
                            {
                                self.remove_selected();
                            }
                        });
                    });
                });
                ui.label(
                    RichText::new("이 순서대로 번호가 매겨집니다.")
                        .small()
                        .color(theme::muted(ui.visuals())),
                );
                ui.separator();
                self.file_list(ui);
            });
    }

    fn file_list(&mut self, ui: &mut Ui) {
        let n = self.files.len();
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show_rows(ui, ROW_H, n, |ui, range| {
                for i in range {
                    let resp = self.file_row(ui, i);
                    if self.scroll_to_selected && self.selected == Some(i) {
                        resp.scroll_to_me(None);
                        self.scroll_to_selected = false;
                    }
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                }
            });
        if let Some(i) = clicked {
            self.selected = Some(i);
        }
    }

    fn file_row(&self, ui: &mut Ui, i: usize) -> egui::Response {
        let dark = ui.visuals().dark_mode;
        let v = ui.visuals().clone();
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let selected = self.selected == Some(i);
        let report = self.row_report(i);
        let done = matches!(self.phase, Phase::Done(_));
        let path = &self.files[i];
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let status_text = report.map(|r| text::status_label(r.status, done));
        resp.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::ListBoxOption,
                true,
                selected,
                format!("{} {}", name, status_text.unwrap_or("")),
            )
        });

        let painter = ui.painter_at(rect);
        if selected {
            painter.rect_filled(rect.shrink(1.0), 6, theme::tint(theme::accent(dark), dark));
        } else if resp.hovered() {
            painter.rect_filled(rect.shrink(1.0), 6, v.faint_bg_color);
        }

        // Number badge.
        let index = report.map_or(self.prefs.start + i as u64, |r| r.index);
        let badge =
            Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(30.0, 22.0));
        painter.rect_filled(badge, 11, v.widgets.inactive.bg_fill);
        painter.text(
            badge.center(),
            Align2::CENTER_CENTER,
            index.to_string(),
            FontId::proportional(12.0),
            v.text_color(),
        );

        // Status chip.
        let mut right = rect.right() - 8.0;
        if let (Some(r), Some(label)) = (report, status_text) {
            let color = match r.status {
                Status::Trimmed => theme::success(dark),
                Status::Unchanged => theme::muted(&v),
                Status::Blank => theme::warning(dark),
                Status::Failed => theme::danger(dark),
            };
            let galley = painter.layout_no_wrap(label.into(), FontId::proportional(12.0), color);
            let chip = Rect::from_min_size(
                pos2(right - galley.size().x - 16.0, rect.top() + 9.0),
                vec2(galley.size().x + 16.0, 22.0),
            );
            painter.rect_filled(chip, 11, theme::tint(color, dark));
            painter.galley(chip.center() - galley.size() / 2.0, galley, color);
            right = chip.left() - 8.0;
        } else if self.analysing.is_some() {
            let galley = painter.layout_no_wrap(
                "확인 중…".into(),
                FontId::proportional(12.0),
                theme::muted(&v),
            );
            painter.galley(
                pos2(right - galley.size().x, rect.top() + 12.0),
                galley,
                theme::muted(&v),
            );
            right -= 60.0;
        }

        // Name and details.
        let left = rect.left() + 46.0;
        let line1 = one_line(ui, &name, 14.0, v.strong_text_color(), right - left);
        painter.galley(pos2(left, rect.top() + 9.0), line1, v.strong_text_color());
        let (detail, color) = match report {
            Some(r) if r.status == Status::Failed => (text::file_error(r), theme::danger(dark)),
            Some(r) => {
                let mut s = format!("→ {}", text::file_name(&r.output));
                if let (Some(a), Some(b)) = (r.original_size, r.output_size)
                    && a != b
                {
                    s.push_str(&format!("   {}×{} → {}×{}", a[0], a[1], b[0], b[1]));
                }
                (s, theme::muted(&v))
            }
            None => (String::new(), theme::muted(&v)),
        };
        let line2 = one_line(ui, &detail, 12.0, color, rect.right() - 8.0 - left);
        painter.galley(pos2(left, rect.top() + 31.0), line2, color);

        if let Some(r) = report.filter(|r| r.status == Status::Failed) {
            resp.on_hover_text(text::file_error(r))
        } else {
            resp.on_hover_text(path.display().to_string())
        }
    }

    fn empty_state(&mut self, ui: &mut Ui) {
        let dark = ui.visuals().dark_mode;
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        let rect = ui.available_rect_before_wrap().shrink(8.0);
        let stroke_color = if hovering {
            theme::accent(dark)
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke.color
        };
        if hovering {
            ui.painter()
                .rect_filled(rect, 12, theme::tint(theme::accent(dark), dark));
        }
        ui.painter()
            .rect_stroke(rect, 12, Stroke::new(1.5, stroke_color), StrokeKind::Inside);

        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_center_size(rect.center(), vec2(420.0, 260.0)))
                .layout(Layout::top_down(Align::Center)),
        );
        let ui = &mut child;
        page_icon(ui, theme::accent(dark));
        ui.add_space(10.0);
        ui.label(
            RichText::new("도면 이미지를 여기에 끌어다 놓으세요")
                .size(18.0)
                .strong(),
        );
        ui.label(
            RichText::new("파일이나 폴더를 통째로 놓아도 됩니다.")
                .color(theme::muted(ui.visuals())),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let w = 2.0 * 120.0 + ui.spacing().item_spacing.x;
            ui.add_space((ui.available_width() - w) / 2.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("파일 선택…").color(Color32::WHITE))
                        .fill(theme::PRIMARY)
                        .min_size(vec2(120.0, 34.0)),
                )
                .on_hover_text("Ctrl+O")
                .clicked()
            {
                self.pick_files();
            }
            if ui
                .add(egui::Button::new("폴더 선택…").min_size(vec2(120.0, 34.0)))
                .clicked()
            {
                self.pick_folder();
            }
        });
        ui.add_space(12.0);
        ui.label(
            RichText::new(format!(
                "지원 형식: {}",
                codec::EXTENSIONS.join(", ").to_uppercase()
            ))
            .small()
            .color(theme::muted(ui.visuals())),
        );
    }

    fn preview(&mut self, ui: &mut Ui) {
        let Some(i) = self.selected.filter(|&i| i < self.files.len()) else {
            centered_hint(ui, "왼쪽 목록에서 이미지를 고르면 미리보기가 나옵니다.");
            return;
        };
        let path = self.files[i].clone();
        if !self.previews.contains_key(&path) {
            if self.previews.len() > 16 {
                self.previews.clear();
            }
            self.previews.insert(path.clone(), PreviewState::Loading);
            worker::load_preview(ui.ctx(), &self.tx, path.clone());
        }
        let dark = ui.visuals().dark_mode;
        let muted = theme::muted(ui.visuals());
        let report = self.row_report(i).cloned();

        // Header.
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(path.file_name().unwrap_or_default().to_string_lossy()).strong(),
                )
                .truncate(),
            );
        });
        let caption = match &report {
            Some(r) => match (r.status, r.original_size, r.output_size) {
                (Status::Failed, ..) => {
                    RichText::new(text::file_error(r)).color(theme::danger(dark))
                }
                (Status::Blank, ..) => {
                    RichText::new("이미지 전체가 배경색입니다. 자르지 않고 그대로 저장합니다.")
                        .color(theme::warning(dark))
                }
                (Status::Unchanged, Some(a), _) => {
                    RichText::new(format!("{}×{} px · 잘라낼 여백이 없습니다.", a[0], a[1]))
                        .color(muted)
                }
                (_, Some(a), Some(b)) => {
                    RichText::new(format!("{}×{} px → {}×{} px", a[0], a[1], b[0], b[1]))
                        .color(muted)
                }
                _ => RichText::new(""),
            },
            None => RichText::new("확인 중…").color(muted),
        };
        ui.label(caption);
        if report.as_ref().is_some_and(|r| r.status == Status::Trimmed) {
            ui.label(
                RichText::new("어둡게 보이는 부분을 잘라냅니다.")
                    .small()
                    .color(muted),
            );
        }
        for w in report.iter().flat_map(|r| &r.warnings) {
            if !w.contains("entirely background") {
                ui.label(
                    RichText::new(text::warning(w))
                        .small()
                        .color(theme::warning(dark)),
                );
            }
        }
        ui.add_space(4.0);

        // Canvas.
        let canvas = ui.available_rect_before_wrap();
        let canvas_bg = if dark {
            Color32::from_gray(38)
        } else {
            Color32::from_gray(226)
        };
        ui.painter().rect_filled(canvas, 8, canvas_bg);
        ui.allocate_rect(canvas, Sense::hover());
        match self.previews.get(&path) {
            Some(PreviewState::Ready { texture, full_size }) => {
                let tex_size = texture.size_vec2();
                let area = canvas.shrink(16.0);
                let scale = (area.width() / tex_size.x)
                    .min(area.height() / tex_size.y)
                    .min(4.0);
                let img_rect = Rect::from_center_size(area.center(), tex_size * scale);
                let painter = ui.painter_at(canvas);
                painter.image(
                    texture.id(),
                    img_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.rect_stroke(
                    img_rect,
                    0,
                    Stroke::new(1.0, Color32::from_black_alpha(40)),
                    StrokeKind::Outside,
                );
                if let Some(crop) = report.as_ref().and_then(|r| r.crop) {
                    let s = img_rect.width() / full_size[0] as f32;
                    let keep = Rect::from_min_size(
                        img_rect.min + vec2(crop.x as f32, crop.y as f32) * s,
                        vec2(crop.width as f32, crop.height as f32) * s,
                    );
                    let shade = Color32::from_black_alpha(if dark { 150 } else { 110 });
                    for r in outside(img_rect, keep) {
                        painter.rect_filled(r, CornerRadius::ZERO, shade);
                    }
                    painter.rect_stroke(
                        keep,
                        0,
                        Stroke::new(2.0, theme::accent(dark)),
                        StrokeKind::Outside,
                    );
                }
            }
            Some(PreviewState::Failed) => {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(canvas));
                centered_hint(&mut child, "미리보기를 표시할 수 없습니다.");
            }
            _ => {
                ui.put(
                    Rect::from_center_size(canvas.center(), Vec2::splat(32.0)),
                    egui::Spinner::new().size(28.0),
                );
            }
        }
    }

    fn result_view(&mut self, ui: &mut Ui, report: &Report) {
        let dark = ui.visuals().dark_mode;
        let muted = theme::muted(ui.visuals());
        let s = &report.summary;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_center_size(
                    ui.available_rect_before_wrap().center(),
                    vec2(460.0, 320.0),
                ))
                .layout(Layout::top_down(Align::Center)),
        );
        let ui = &mut child;
        let (title, color, body) = if !report.committed {
            (
                "아무 파일도 바뀌지 않았습니다",
                theme::danger(dark),
                format!(
                    "{}개 파일에서 오류가 나서 작업을 취소했습니다. 목록에서 원인을 확인하세요.",
                    s.failed
                ),
            )
        } else if s.failed > 0 {
            (
                "일부 파일을 처리하지 못했습니다",
                theme::warning(dark),
                format!(
                    "{}개 성공, {}개 실패. 목록에서 원인을 확인하세요.",
                    s.total - s.failed,
                    s.failed
                ),
            )
        } else {
            (
                "완료했습니다",
                theme::success(dark),
                format!(
                    "{}개 중 {}개의 여백을 잘랐습니다.{}",
                    s.total,
                    s.trimmed,
                    if s.unchanged > 0 {
                        format!(" ({}개는 여백이 없어 그대로 두었습니다.)", s.unchanged)
                    } else {
                        String::new()
                    }
                ),
            )
        };
        status_mark(ui, color, report.committed && s.failed == 0);
        ui.add_space(8.0);
        ui.label(RichText::new(title).size(20.0).strong().color(color));
        ui.add_space(4.0);
        ui.label(body);
        if s.blank > 0 {
            ui.label(
                RichText::new(format!(
                    "빈 이미지 {}개는 내용이 맞는지 직접 확인하세요.",
                    s.blank
                ))
                .color(theme::warning(dark)),
            );
        }
        if report.committed {
            let deleted = report.files.iter().filter(|f| f.original_removed).count();
            if deleted > 0 {
                ui.label(
                    RichText::new(format!("원본 {deleted}개는 결과 파일로 바뀌었습니다."))
                        .color(muted),
                );
            }
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let w = 2.0 * 150.0 + ui.spacing().item_spacing.x;
            ui.add_space((ui.available_width() - w) / 2.0);
            if report.committed {
                if ui
                    .add(
                        egui::Button::new(RichText::new("결과 폴더 열기").color(Color32::WHITE))
                            .fill(theme::PRIMARY)
                            .min_size(vec2(150.0, 36.0)),
                    )
                    .clicked()
                    && let Some(d) = self.result_folder(report)
                {
                    worker::open_folder(&d);
                }
                if ui
                    .add(egui::Button::new("새 작업").min_size(vec2(150.0, 36.0)))
                    .clicked()
                {
                    self.clear();
                }
            } else {
                if ui
                    .add(
                        egui::Button::new(RichText::new("목록으로 돌아가기").color(Color32::WHITE))
                            .fill(theme::PRIMARY)
                            .min_size(vec2(150.0, 36.0)),
                    )
                    .clicked()
                {
                    self.phase = Phase::Editing;
                    self.mark_dirty();
                }
                if ui
                    .add(egui::Button::new("새 작업").min_size(vec2(150.0, 36.0)))
                    .clicked()
                {
                    self.clear();
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        match self.phase {
            Phase::Confirming => {
                let n = self.files.len();
                let mut action = None;
                let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
                    ui.set_width(400.0);
                    ui.label(RichText::new(format!("원본 {n}개를 바꿉니다")).size(18.0).strong());
                    ui.add_space(4.0);
                    ui.label("여백을 자른 이미지를 같은 폴더에 저장하고, 원본 파일은 삭제합니다. 이 작업은 되돌릴 수 없습니다.");
                    ui.label(
                        RichText::new("원본을 남기려면 ‘원본 파일도 남겨두기’를 켜거나 다른 폴더에 저장하세요.")
                            .small()
                            .color(theme::muted(ui.visuals())),
                    );
                    ui.add_space(10.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("자르고 원본 삭제").color(Color32::WHITE),
                                )
                                .fill(theme::DANGER_FILL),
                            )
                            .clicked()
                        {
                            action = Some(true);
                        }
                        if ui.button("취소").clicked() {
                            action = Some(false);
                        }
                    });
                });
                if modal.should_close() && action.is_none() {
                    action = Some(false);
                }
                match action {
                    Some(true) => self.start_run(ctx),
                    Some(false) => self.phase = Phase::Editing,
                    None => {}
                }
            }
            Phase::Running { done, total } => {
                egui::Modal::new(egui::Id::unique("running")).show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.label(RichText::new("여백을 자르는 중…").size(17.0).strong());
                    ui.add_space(6.0);
                    let frac = if total == 0 {
                        0.0
                    } else {
                        done as f32 / total as f32
                    };
                    ui.add(egui::ProgressBar::new(frac).text(format!("{done} / {total}")));
                    ui.label(
                        RichText::new("끝날 때까지 창을 닫지 마세요.")
                            .small()
                            .color(theme::muted(ui.visuals())),
                    );
                });
            }
            _ => {}
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.handle_input(&ctx);

        self.top_bar(ui);
        self.settings_panel(ui);
        if !self.files.is_empty() {
            self.file_panel(ui);
        }
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if let Phase::Done(report) = &self.phase {
                let report = report.clone();
                self.result_view(ui, &report);
            } else if self.files.is_empty() {
                self.empty_state(ui);
            } else {
                self.preview(ui);
            }
        });
        self.dialogs(&ctx);

        // Closing mid-run could leave temp files; wait for the batch to finish.
        if matches!(self.phase, Phase::Running { .. })
            && ctx.input(|i| i.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        if self.prefs != self.last_prefs {
            self.last_prefs = self.prefs.clone();
            self.mark_dirty();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFS_KEY, &self.prefs);
    }
}

// ---------------------------------------------------------------------------
// Small drawing helpers
// ---------------------------------------------------------------------------

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).size(15.0).strong());
}

fn centered_hint(ui: &mut Ui, msg: &str) {
    let rect = ui.available_rect_before_wrap();
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        msg,
        FontId::proportional(14.0),
        theme::muted(ui.visuals()),
    );
}

/// Single-line text truncated with "…" to `width`.
fn one_line(
    ui: &Ui,
    s: &str,
    size: f32,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(s.to_string(), FontId::proportional(size), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The parts of `outer` not covered by `inner`.
fn outside(outer: Rect, inner: Rect) -> [Rect; 4] {
    [
        Rect::from_min_max(outer.min, pos2(outer.max.x, inner.min.y)),
        Rect::from_min_max(pos2(outer.min.x, inner.max.y), outer.max),
        Rect::from_min_max(
            pos2(outer.min.x, inner.min.y),
            pos2(inner.min.x, inner.max.y),
        ),
        Rect::from_min_max(
            pos2(inner.max.x, inner.min.y),
            pos2(outer.max.x, inner.max.y),
        ),
    ]
}

/// A page with crop marks.
fn page_icon(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(64.0, 72.0), Sense::hover());
    let p = ui.painter();
    let page = rect.shrink2(vec2(12.0, 10.0));
    p.rect_stroke(page, 3, Stroke::new(2.0, color), StrokeKind::Middle);
    for k in 0..3 {
        let y = page.top() + 14.0 + k as f32 * 10.0;
        p.line_segment(
            [pos2(page.left() + 8.0, y), pos2(page.right() - 8.0, y)],
            Stroke::new(1.5, color.gamma_multiply(0.6)),
        );
    }
    let l = 10.0;
    let st = Stroke::new(2.0, color);
    let (a, b) = (rect.left_top(), rect.right_bottom());
    p.line_segment([a, a + vec2(l, 0.0)], st);
    p.line_segment([a, a + vec2(0.0, l)], st);
    p.line_segment([b, b - vec2(l, 0.0)], st);
    p.line_segment([b, b - vec2(0.0, l)], st);
}

/// Check mark (success) or exclamation mark in a circle.
fn status_mark(ui: &mut Ui, color: Color32, ok: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    p.circle_filled(c, 26.0, color);
    let st = Stroke::new(4.0, Color32::WHITE);
    if ok {
        p.line_segment([c + vec2(-11.0, 1.0), c + vec2(-3.0, 9.0)], st);
        p.line_segment([c + vec2(-3.0, 9.0), c + vec2(12.0, -8.0)], st);
    } else {
        p.line_segment([c + vec2(0.0, -12.0), c + vec2(0.0, 4.0)], st);
        p.circle_filled(c + vec2(0.0, 11.0), 2.5, Color32::WHITE);
    }
}
