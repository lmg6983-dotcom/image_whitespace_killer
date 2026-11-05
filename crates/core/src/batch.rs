//! Batch processing: plan output names, trim every image into a temp folder,
//! then commit everything at once (or nothing, if a file failed).

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::ImageFormat;
use serde::Serialize;

use crate::codec;
use crate::naming;
use crate::trim::{self, Background, Rect};

/// Default output name template.
pub const DEFAULT_TEMPLATE: &str = "도 {num}";

/// Everything that controls a batch run.
#[derive(Clone, Debug)]
pub struct Options {
    /// Image files and/or directories. Empty means the current directory.
    pub paths: Vec<PathBuf>,
    /// Colour tolerance in percent (ImageMagick `-fuzz`), 0–100.
    pub fuzz: f64,
    pub background: Background,
    /// Output file name without extension (`{num}`, `{num:03}`, `{stem}`).
    pub name_template: String,
    pub start: u64,
    /// Write results here instead of next to the inputs (originals are kept).
    pub out_dir: Option<PathBuf>,
    pub keep_originals: bool,
    pub overwrite: bool,
    pub allow_partial: bool,
    /// Pixels of background to keep around the content.
    pub margin: u32,
    pub jpeg_quality: u8,
    pub dry_run: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            fuzz: 5.0,
            background: Background::Color([255, 255, 255]),
            name_template: DEFAULT_TEMPLATE.into(),
            start: 1,
            out_dir: None,
            keep_originals: false,
            overwrite: false,
            allow_partial: false,
            margin: 0,
            jpeg_quality: 95,
            dry_run: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Report types (JSON contract, see AGENTS.md)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Margins were removed.
    Trimmed,
    /// Nothing to trim; file copied unchanged (possibly renamed).
    Unchanged,
    /// Whole image is background; copied unchanged (possibly renamed).
    Blank,
    /// Processing failed; the original was not touched.
    Failed,
}

/// Machine-readable reason for a failed file.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The file could not be read from disk.
    Read,
    /// The file is not a readable image (corrupt or unsupported).
    Decode,
    /// The result could not be encoded.
    Encode,
    /// The result could not be written or moved into place.
    Write,
    /// The target name is taken by a file that is not part of the batch.
    Exists,
}

#[derive(Serialize, Clone, Debug)]
pub struct FileReport {
    /// Sequence number used for {num}.
    pub index: u64,
    pub input: String,
    pub output: String,
    pub status: Status,
    pub format: Option<String>,
    pub original_size: Option<[u32; 2]>,
    pub output_size: Option<[u32; 2]>,
    /// Kept region in the (EXIF-oriented) source image.
    pub crop: Option<Rect>,
    pub background: Option<String>,
    pub dpi: Option<[f64; 2]>,
    /// Output file was written (always false in dry-run).
    pub written: bool,
    /// Original no longer exists under its old name (deleted or replaced).
    pub original_removed: bool,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub error_code: Option<ErrorCode>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Settings {
    pub fuzz_percent: f64,
    pub background: String,
    pub name_template: String,
    pub start: u64,
    pub out_dir: Option<String>,
    pub keep_originals: bool,
    pub overwrite: bool,
    pub allow_partial: bool,
    pub margin: u32,
    pub jpeg_quality: u8,
}

#[derive(Serialize, Default, Clone, Debug)]
pub struct Summary {
    pub total: usize,
    pub trimmed: usize,
    pub unchanged: usize,
    pub blank: usize,
    pub failed: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct Report {
    pub ok: bool,
    pub dry_run: bool,
    /// Changes were applied to disk (false for dry-run or an aborted batch).
    pub committed: bool,
    /// Why nothing was changed, if the batch was aborted.
    pub aborted: Option<String>,
    pub settings: Settings,
    pub summary: Summary,
    pub files: Vec<FileReport>,
}

/// Progress notification, sent before each file is analysed.
#[derive(Clone, Copy, Debug)]
pub struct Progress<'a> {
    /// Files finished so far.
    pub done: usize,
    pub total: usize,
    pub current: &'a Path,
}

// ---------------------------------------------------------------------------

struct Job {
    input: PathBuf,
    output: PathBuf,
    out_dir: PathBuf,
    temp: Option<PathBuf>,
    /// Output == input and content unchanged: nothing to do on disk.
    noop: bool,
    report: FileReport,
}

impl Job {
    fn fail(&mut self, code: ErrorCode, msg: String) {
        self.report.status = Status::Failed;
        self.report.error = Some(msg);
        self.report.error_code = Some(code);
    }
}

enum Payload {
    Encoded(Vec<u8>),
    CopyOriginal,
}

struct Failure(ErrorCode, anyhow::Error);

trait Code<T> {
    fn code(self, c: ErrorCode) -> std::result::Result<T, Failure>;
}

impl<T> Code<T> for Result<T> {
    fn code(self, c: ErrorCode) -> std::result::Result<T, Failure> {
        self.map_err(|e| Failure(c, e))
    }
}

/// Run a batch without progress reporting.
pub fn run(opts: &Options) -> Result<Report> {
    run_with_progress(opts, |_| {})
}

/// Run a batch, calling `progress` before each file is analysed.
///
/// Returns `Err` only for fatal problems found before anything was written
/// (bad template, missing path, name collisions inside the batch, ...).
pub fn run_with_progress(opts: &Options, mut progress: impl FnMut(Progress)) -> Result<Report> {
    let template = opts.name_template.clone();
    naming::validate_template(&template).map_err(anyhow::Error::msg)?;

    let keep_originals = opts.keep_originals || opts.out_dir.is_some();
    let out_dir = opts.out_dir.as_deref().map(absolute).transpose()?;

    let inputs = collect_inputs(&opts.paths)?;

    // ---- plan ---------------------------------------------------------
    let mut jobs = Vec::with_capacity(inputs.len());
    let mut seen_outputs = HashSet::new();
    for (i, input) in inputs.into_iter().enumerate() {
        let index = opts.start + i as u64;
        let stem = input.file_stem().unwrap_or_default().to_string_lossy();
        let ext = input.extension().unwrap_or_default().to_string_lossy();
        let base = naming::render(&template, index, &stem).map_err(anyhow::Error::msg)?;
        let dir = match &out_dir {
            Some(d) => d.clone(),
            None => input.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        let output = dir.join(format!("{base}.{ext}"));
        if !seen_outputs.insert(path_key(&output)) {
            bail!(
                "several inputs would be written to '{}'; use {{num}} in --name",
                output.display()
            );
        }
        if keep_originals && path_key(&output) == path_key(&input) {
            bail!(
                "output '{}' is the input file itself, but originals must be kept \
                 (--keep-originals/--out-dir); choose another --name or --out-dir",
                output.display()
            );
        }
        jobs.push(Job {
            report: FileReport {
                index,
                input: input.display().to_string(),
                output: output.display().to_string(),
                status: Status::Failed,
                format: None,
                original_size: None,
                output_size: None,
                crop: None,
                background: None,
                dpi: None,
                written: false,
                original_removed: false,
                warnings: Vec::new(),
                error: None,
                error_code: None,
            },
            input,
            output,
            out_dir: dir,
            temp: None,
            noop: false,
        });
    }

    if let Some(d) = &out_dir {
        if !opts.dry_run {
            fs::create_dir_all(d)
                .with_context(|| format!("cannot create output directory '{}'", d.display()))?;
        } else if d.exists() && !d.is_dir() {
            bail!("'{}' exists and is not a directory", d.display());
        }
    }

    // ---- phase 1: decode, trim, encode to temp files ------------------
    let temp_name = format!(".trim-image-tmp-{}", std::process::id());
    let mut temp_dirs: Vec<PathBuf> = Vec::new();
    let total = jobs.len();
    for (done, job) in jobs.iter_mut().enumerate() {
        progress(Progress {
            done,
            total,
            current: &job.input,
        });
        let payload = match process(job, opts) {
            Ok(p) => p,
            Err(Failure(code, e)) => {
                job.fail(code, format!("{e:#}"));
                continue;
            }
        };
        job.noop = matches!(payload, Payload::CopyOriginal)
            && path_key(&job.output) == path_key(&job.input);
        if opts.dry_run || job.noop {
            continue;
        }
        let tdir = job.out_dir.join(&temp_name);
        let tpath = tdir.join(format!(
            "{}.{}",
            job.report.index,
            job.output.extension().unwrap_or_default().to_string_lossy()
        ));
        let res = fs::create_dir_all(&tdir)
            .with_context(|| format!("cannot create temp folder '{}'", tdir.display()))
            .and_then(|_| {
                if !temp_dirs.contains(&tdir) {
                    temp_dirs.push(tdir.clone());
                }
                match &payload {
                    Payload::Encoded(bytes) => fs::write(&tpath, bytes).map(|_| ()),
                    Payload::CopyOriginal => fs::copy(&job.input, &tpath).map(|_| ()),
                }
                .with_context(|| format!("cannot write '{}'", tpath.display()))
            });
        match res {
            Ok(()) => job.temp = Some(tpath),
            Err(e) => {
                job.fail(ErrorCode::Write, format!("{e:#}"));
                let _ = fs::remove_file(&tpath);
            }
        }
    }

    // ---- phase 2: refuse to clobber unrelated existing files ----------
    let ok = |j: &Job| j.report.status != Status::Failed;
    let mut freed: HashSet<String> = if keep_originals {
        HashSet::new()
    } else {
        jobs.iter()
            .filter(|j| ok(j))
            .map(|j| path_key(&j.input))
            .collect()
    };
    loop {
        let mut changed = false;
        for job in jobs.iter_mut().filter(|j| ok(j) && !j.noop) {
            if opts.overwrite || freed.contains(&path_key(&job.output)) || !job.output.exists() {
                continue;
            }
            let msg = format!(
                "output '{}' already exists and is not part of this batch (use --overwrite or another --name/--out-dir)",
                job.output.display()
            );
            job.fail(ErrorCode::Exists, msg);
            if let Some(t) = job.temp.take() {
                let _ = fs::remove_file(t);
            }
            freed.remove(&path_key(&job.input));
            changed = true;
        }
        if !changed {
            break;
        }
    }

    // ---- phase 3: commit (all-or-nothing unless allow_partial) ---------
    let failed = jobs.iter().filter(|j| !ok(j)).count();
    let mut aborted = None;
    if !opts.dry_run && failed > 0 && !opts.allow_partial {
        aborted = Some(format!(
            "{failed} file(s) failed; nothing was changed (fix them, or rerun with --allow-partial)"
        ));
        for job in &mut jobs {
            if let Some(t) = job.temp.take() {
                let _ = fs::remove_file(t);
            }
        }
        for d in &temp_dirs {
            let _ = fs::remove_dir(d);
        }
    }
    let committed = !opts.dry_run && aborted.is_none();
    if committed {
        let replaced: HashSet<String> = jobs
            .iter()
            .filter(|j| ok(j) && !j.noop)
            .map(|j| path_key(&j.output))
            .collect();
        if !keep_originals {
            for job in jobs.iter_mut().filter(|j| ok(j) && !j.noop) {
                if replaced.contains(&path_key(&job.input)) {
                    continue; // will be overwritten by an output below
                }
                match fs::remove_file(&job.input) {
                    Ok(()) => job.report.original_removed = true,
                    Err(e) => job
                        .report
                        .warnings
                        .push(format!("could not delete original: {e}")),
                }
            }
        }
        for job in jobs.iter_mut().filter(|j| ok(j) && !j.noop) {
            let temp = job.temp.clone().expect("temp file for successful job");
            match fs::rename(&temp, &job.output) {
                Ok(()) => {
                    job.report.written = true;
                    job.temp = None;
                }
                Err(e) => {
                    let msg = format!(
                        "could not move result into place: {e}; result kept at '{}'",
                        temp.display()
                    );
                    job.fail(ErrorCode::Write, msg);
                }
            }
        }
        if !keep_originals {
            for job in jobs.iter_mut().filter(|j| !j.noop && j.report.written) {
                if replaced.contains(&path_key(&job.input)) {
                    job.report.original_removed = true;
                }
            }
        }
        for d in &temp_dirs {
            let _ = fs::remove_dir(d); // only succeeds when empty
        }
    }

    // ---- report ---------------------------------------------------------
    let mut summary = Summary {
        total: jobs.len(),
        ..Default::default()
    };
    for j in &jobs {
        match j.report.status {
            Status::Trimmed => summary.trimmed += 1,
            Status::Unchanged => summary.unchanged += 1,
            Status::Blank => summary.blank += 1,
            Status::Failed => summary.failed += 1,
        }
    }
    Ok(Report {
        ok: summary.failed == 0,
        dry_run: opts.dry_run,
        committed,
        aborted,
        settings: Settings {
            fuzz_percent: opts.fuzz,
            background: match opts.background {
                Background::Auto => "auto".into(),
                Background::Color(c) => trim::hex_color(c),
            },
            name_template: template,
            start: opts.start,
            out_dir: out_dir.map(|d| d.display().to_string()),
            keep_originals,
            overwrite: opts.overwrite,
            allow_partial: opts.allow_partial,
            margin: opts.margin,
            jpeg_quality: opts.jpeg_quality,
        },
        summary,
        files: jobs.into_iter().map(|j| j.report).collect(),
    })
}

/// Decode, find the content box, and produce the bytes for the output file.
fn process(job: &mut Job, opts: &Options) -> std::result::Result<Payload, Failure> {
    let bytes = fs::read(&job.input)
        .with_context(|| format!("cannot read '{}'", job.input.display()))
        .code(ErrorCode::Read)?;
    let decoded = codec::decode(&bytes).code(ErrorCode::Decode)?;
    let img = &decoded.image;
    let (w, h) = (img.width(), img.height());
    let r = &mut job.report;
    r.format = Some(format!("{:?}", decoded.format).to_lowercase());
    r.original_size = Some([w, h]);
    r.dpi = decoded.dpi.map(|(x, y)| [x, y]);

    let rgba = img.to_rgba8();
    let bg = trim::resolve_background(&rgba, opts.background);
    r.background = Some(trim::hex_color(bg));
    let bbox = trim::content_bbox(&rgba, bg, opts.fuzz);
    drop(rgba);

    let out_ext = job
        .output
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let out_format = codec::format_for_ext(&out_ext)
        .with_context(|| format!("unsupported output extension '.{out_ext}'"))
        .code(ErrorCode::Encode)?;

    let Some(bbox) = bbox else {
        r.status = Status::Blank;
        r.output_size = Some([w, h]);
        r.warnings
            .push("image is entirely background; left uncropped".into());
        return Ok(Payload::CopyOriginal);
    };
    let crop = trim::expand(bbox, opts.margin, w, h);
    r.crop = Some(crop);
    r.output_size = Some([crop.width, crop.height]);

    if crop
        == (Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        })
        && out_format == decoded.format
    {
        r.status = Status::Unchanged;
        return Ok(Payload::CopyOriginal);
    }

    if matches!(decoded.format, ImageFormat::Gif) {
        r.warnings.push("GIF: only the first frame is kept".into());
    }
    if decoded.dpi.is_none()
        && matches!(
            out_format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Tiff | ImageFormat::Bmp
        )
    {
        r.warnings
            .push("source has no DPI information; none written".into());
    }
    let cropped = img.crop_imm(crop.x, crop.y, crop.width, crop.height);
    let encoded = codec::encode(&cropped, out_format, decoded.dpi, opts.jpeg_quality)
        .with_context(|| format!("cannot encode {out_format:?}"))
        .code(ErrorCode::Encode)?;
    r.status = if crop.width == w && crop.height == h {
        Status::Unchanged
    } else {
        Status::Trimmed
    };
    Ok(Payload::Encoded(encoded))
}

/// Expand directories (non-recursive, natural order, hidden files skipped)
/// and validate files. Empty `paths` means the current directory.
pub fn collect_inputs(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let default = [PathBuf::from(".")];
    let paths = if paths.is_empty() {
        &default[..]
    } else {
        paths
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for p in paths {
        let abs = absolute(p)?;
        let meta =
            fs::metadata(&abs).with_context(|| format!("cannot access '{}'", p.display()))?;
        let mut found = Vec::new();
        if meta.is_dir() {
            for entry in
                fs::read_dir(&abs).with_context(|| format!("cannot list '{}'", p.display()))?
            {
                let entry = entry?;
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || !entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                {
                    continue;
                }
                if is_supported_image(&path) {
                    found.push((name, path));
                }
            }
            found.sort_by(|a, b| naming::natural_cmp(&a.0, &b.0));
        } else {
            if !is_supported_image(&abs) {
                bail!(
                    "'{}' is not a supported image (extensions: {})",
                    p.display(),
                    codec::EXTENSIONS.join(", ")
                );
            }
            found.push((String::new(), abs));
        }
        for (_, path) in found {
            if seen.insert(path_key(&path)) {
                out.push(path);
            }
        }
    }
    Ok(out)
}

/// The file has one of the supported image extensions.
pub fn is_supported_image(p: &Path) -> bool {
    p.extension()
        .map(|e| codec::is_supported_ext(&e.to_string_lossy()))
        .unwrap_or(false)
}

/// Absolute, lexically normalised path (`a/./b/../c` -> `a/c`).
pub fn absolute(p: &Path) -> Result<PathBuf> {
    let abs = std::path::absolute(p).with_context(|| format!("invalid path '{}'", p.display()))?;
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Comparison key for paths (case-insensitive on Windows/macOS file systems).
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if cfg!(any(windows, target_os = "macos")) {
        s.to_lowercase()
    } else {
        s
    }
}
