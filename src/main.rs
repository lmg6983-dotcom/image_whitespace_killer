//! trim-image — remove white margins around patent drawings.
//!
//! Rust port of `trim_image.bat` (ImageMagick `-fuzz 5% -trim`, rename to
//! "도 1..n", delete originals), redesigned for unattended use by agents:
//! no prompts, explicit arguments, `--dry-run`, `--json` reports and
//! meaningful exit codes. No external tools (ImageMagick) required.

mod codec;
mod naming;
mod trim;

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use image::ImageFormat;
use serde::Serialize;

use crate::trim::{Background, Rect};

const AFTER_HELP: &str = "\
EXAMPLES:
  trim-image                                  # same as trim_image.bat in the current folder
  trim-image ./drawings --dry-run --json      # preview: what would happen, machine-readable
  trim-image ./drawings --json                # trim, rename to '도 1, 도 2, ...', delete originals
  trim-image a.png b.tif -o out --name 'FIG{num:02}'  # write FIG01.png, FIG02.tif into ./out
  trim-image ./drawings --keep-names          # trim in place, keep file names

NAME TEMPLATE (--name):
  {num}      sequence number (1, 2, ... starting at --start)
  {num:03}   zero-padded sequence number (001, 002, ...)
  {stem}     original file name without extension
  The original extension is always kept: '도 {num}' -> '도 1.png'.

BEHAVIOUR:
  * Directories are scanned (non-recursive) for png jpg jpeg tif tiff bmp gif webp,
    in natural order (fig2 before fig10). Files given explicitly keep argument order.
  * Without --out-dir, results go next to the inputs and originals are removed
    (like the .bat). With --out-dir, originals are never touched.
  * All outputs are fully written to a temp folder first. If any file fails,
    nothing is changed at all (unless --allow-partial). Existing files that are
    not part of the batch are never overwritten unless --overwrite is given.
  * Images with nothing to trim are copied byte-for-byte (no re-compression).
    Blank images (all background) are left uncropped and reported as 'blank'.
  * DPI is preserved for PNG, JPEG, TIFF and BMP.

EXIT CODES:
  0  every file succeeded (trimmed / unchanged / blank) and was committed
  1  at least one file failed (see report; without --allow-partial nothing changed)
  2  invalid arguments or fatal error (nothing was changed)";

#[derive(Parser, Debug)]
#[command(
    name = "trim-image",
    version,
    about = "Trim white margins from (patent drawing) images and number them '도 1, 도 2, ...'.",
    after_help = AFTER_HELP
)]
struct Cli {
    /// Image files and/or directories to process [default: current directory]
    paths: Vec<PathBuf>,

    /// Colour tolerance in percent (ImageMagick -fuzz equivalent), 0-100
    #[arg(short, long, default_value_t = 5.0, value_name = "PERCENT", value_parser = parse_fuzz)]
    fuzz: f64,

    /// Background colour to trim: white, black, #RRGGBB, or auto (from corners)
    #[arg(short, long = "bg", default_value = "white", value_name = "COLOR", value_parser = trim::parse_background)]
    bg: Background,

    /// Output file name without extension (see NAME TEMPLATE below)
    #[arg(short, long, default_value = "도 {num}", value_name = "TEMPLATE")]
    name: String,

    /// Keep original file names (same as --name '{stem}')
    #[arg(long, conflicts_with = "name")]
    keep_names: bool,

    /// First sequence number
    #[arg(long, default_value_t = 1, value_name = "N")]
    start: u64,

    /// Write results into this directory instead of next to the inputs (originals are kept)
    #[arg(short, long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// Do not delete the original files
    #[arg(short, long)]
    keep_originals: bool,

    /// Allow replacing existing files that are not part of this batch
    #[arg(long)]
    overwrite: bool,

    /// Commit the files that succeeded even if others failed
    /// (default: if any file fails, nothing is changed)
    #[arg(long)]
    allow_partial: bool,

    /// Keep this many pixels of background around the content
    #[arg(short, long, default_value_t = 0, value_name = "PX")]
    margin: u32,

    /// JPEG quality for re-encoded JPEG files, 1-100
    #[arg(short, long, default_value_t = 95, value_name = "Q",
          value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,

    /// Analyse and report only; do not write, rename or delete anything
    #[arg(long)]
    dry_run: bool,

    /// Print a JSON report to stdout (human-readable text otherwise)
    #[arg(long)]
    json: bool,

    /// Suppress human-readable per-file lines (summary and errors still shown)
    #[arg(short = 'Q', long)]
    quiet: bool,
}

fn parse_fuzz(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .trim_end_matches('%')
        .parse()
        .map_err(|_| format!("'{s}' is not a number"))?;
    if !(0.0..=100.0).contains(&v) {
        return Err("fuzz must be between 0 and 100".into());
    }
    Ok(v)
}

// ---------------------------------------------------------------------------
// Report types (JSON contract)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum Status {
    /// Margins were removed.
    Trimmed,
    /// Nothing to trim; file copied unchanged (possibly renamed).
    Unchanged,
    /// Whole image is background; copied unchanged (possibly renamed).
    Blank,
    /// Processing failed; the original was not touched.
    Failed,
}

#[derive(Serialize, Debug)]
struct FileReport {
    /// Sequence number used for {num}.
    index: u64,
    input: String,
    output: String,
    status: Status,
    format: Option<String>,
    original_size: Option<[u32; 2]>,
    output_size: Option<[u32; 2]>,
    /// Kept region in the (EXIF-oriented) source image.
    crop: Option<Rect>,
    background: Option<String>,
    dpi: Option<[f64; 2]>,
    /// Output file was written (always false in dry-run).
    written: bool,
    /// Original no longer exists under its old name (deleted or replaced).
    original_removed: bool,
    warnings: Vec<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct Settings {
    fuzz_percent: f64,
    background: String,
    name_template: String,
    start: u64,
    out_dir: Option<String>,
    keep_originals: bool,
    overwrite: bool,
    allow_partial: bool,
    margin: u32,
    jpeg_quality: u8,
}

#[derive(Serialize, Default)]
struct Summary {
    total: usize,
    trimmed: usize,
    unchanged: usize,
    blank: usize,
    failed: usize,
}

#[derive(Serialize)]
struct Report {
    ok: bool,
    dry_run: bool,
    /// Changes were applied to disk (false for dry-run or an aborted batch).
    committed: bool,
    /// Why nothing was changed, if the batch was aborted.
    aborted: Option<String>,
    settings: Settings,
    summary: Summary,
    files: Vec<FileReport>,
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

enum Payload {
    Encoded(Vec<u8>),
    CopyOriginal,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(report) => {
            print_report(&cli, &report);
            if report.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            if cli.json {
                let v = serde_json::json!({ "ok": false, "fatal": true, "committed": false, "error": format!("{e:#}") });
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<Report> {
    let template = if cli.keep_names {
        "{stem}".to_string()
    } else {
        cli.name.clone()
    };
    naming::validate_template(&template).map_err(anyhow::Error::msg)?;

    let keep_originals = cli.keep_originals || cli.out_dir.is_some();
    let out_dir = cli.out_dir.as_deref().map(absolute).transpose()?;

    let inputs = collect_inputs(&cli.paths)?;

    // ---- plan ---------------------------------------------------------
    let mut jobs = Vec::with_capacity(inputs.len());
    let mut seen_outputs = HashSet::new();
    for (i, input) in inputs.into_iter().enumerate() {
        let index = cli.start + i as u64;
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
            },
            input,
            output,
            out_dir: dir,
            temp: None,
            noop: false,
        });
    }

    if let Some(d) = &out_dir {
        if !cli.dry_run {
            fs::create_dir_all(d)
                .with_context(|| format!("cannot create output directory '{}'", d.display()))?;
        } else if d.exists() && !d.is_dir() {
            bail!("'{}' exists and is not a directory", d.display());
        }
    }

    // ---- phase 1: decode, trim, encode to temp files ------------------
    let temp_name = format!(".trim-image-tmp-{}", std::process::id());
    let mut temp_dirs: Vec<PathBuf> = Vec::new();
    for job in &mut jobs {
        let payload = match process(job, cli) {
            Ok(p) => p,
            Err(e) => {
                job.report.status = Status::Failed;
                job.report.error = Some(format!("{e:#}"));
                continue;
            }
        };
        job.noop = matches!(payload, Payload::CopyOriginal)
            && path_key(&job.output) == path_key(&job.input);
        if cli.dry_run || job.noop {
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
                job.report.status = Status::Failed;
                job.report.error = Some(format!("{e:#}"));
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
            if cli.overwrite || freed.contains(&path_key(&job.output)) || !job.output.exists() {
                continue;
            }
            job.report.status = Status::Failed;
            job.report.error = Some(format!(
                "output '{}' already exists and is not part of this batch (use --overwrite or another --name/--out-dir)",
                job.output.display()
            ));
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

    // ---- phase 3: commit (all-or-nothing unless --allow-partial) -------
    let failed = jobs.iter().filter(|j| !ok(j)).count();
    let mut aborted = None;
    if !cli.dry_run && failed > 0 && !cli.allow_partial {
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
    let committed = !cli.dry_run && aborted.is_none();
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
                    job.report.status = Status::Failed;
                    job.report.error = Some(format!(
                        "could not move result into place: {e}; result kept at '{}'",
                        temp.display()
                    ));
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
        dry_run: cli.dry_run,
        committed,
        aborted,
        settings: Settings {
            fuzz_percent: cli.fuzz,
            background: match cli.bg {
                Background::Auto => "auto".into(),
                Background::Color(c) => trim::hex_color(c),
            },
            name_template: template,
            start: cli.start,
            out_dir: out_dir.map(|d| d.display().to_string()),
            keep_originals,
            overwrite: cli.overwrite,
            allow_partial: cli.allow_partial,
            margin: cli.margin,
            jpeg_quality: cli.quality,
        },
        summary,
        files: jobs.into_iter().map(|j| j.report).collect(),
    })
}

/// Decode, find the content box, and produce the bytes for the output file.
fn process(job: &mut Job, cli: &Cli) -> Result<Payload> {
    let bytes =
        fs::read(&job.input).with_context(|| format!("cannot read '{}'", job.input.display()))?;
    let decoded = codec::decode(&bytes)?;
    let img = &decoded.image;
    let (w, h) = (img.width(), img.height());
    let r = &mut job.report;
    r.format = Some(format!("{:?}", decoded.format).to_lowercase());
    r.original_size = Some([w, h]);
    r.dpi = decoded.dpi.map(|(x, y)| [x, y]);

    let rgba = img.to_rgba8();
    let bg = trim::resolve_background(&rgba, cli.bg);
    r.background = Some(trim::hex_color(bg));
    let bbox = trim::content_bbox(&rgba, bg, cli.fuzz);
    drop(rgba);

    let out_ext = job
        .output
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let out_format = codec::format_for_ext(&out_ext)
        .with_context(|| format!("unsupported output extension '.{out_ext}'"))?;

    let Some(bbox) = bbox else {
        r.status = Status::Blank;
        r.output_size = Some([w, h]);
        r.warnings
            .push("image is entirely background; left uncropped".into());
        return Ok(Payload::CopyOriginal);
    };
    let crop = trim::expand(bbox, cli.margin, w, h);
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
    let encoded = codec::encode(&cropped, out_format, decoded.dpi, cli.quality)
        .with_context(|| format!("cannot encode {out_format:?}"))?;
    r.status = if crop.width == w && crop.height == h {
        Status::Unchanged
    } else {
        Status::Trimmed
    };
    Ok(Payload::Encoded(encoded))
}

fn collect_inputs(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
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
                if has_supported_ext(&path) {
                    found.push((name, path));
                }
            }
            found.sort_by(|a, b| naming::natural_cmp(&a.0, &b.0));
        } else {
            if !has_supported_ext(&abs) {
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

fn has_supported_ext(p: &Path) -> bool {
    p.extension()
        .map(|e| codec::is_supported_ext(&e.to_string_lossy()))
        .unwrap_or(false)
}

/// Absolute, lexically normalised path (`a/./b/../c` -> `a/c`).
fn absolute(p: &Path) -> Result<PathBuf> {
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
fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if cfg!(any(windows, target_os = "macos")) {
        s.to_lowercase()
    } else {
        s
    }
}

fn print_report(cli: &Cli, report: &Report) {
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(report).expect("serialisable report")
        );
        return;
    }
    let name = |s: &str| {
        Path::new(s)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| s.to_string())
    };
    if report.files.is_empty() {
        println!("No image files found ({}).", codec::EXTENSIONS.join(", "));
        return;
    }
    for f in &report.files {
        if f.status == Status::Failed {
            eprintln!(
                "[{}] FAILED    {}: {}",
                f.index,
                name(&f.input),
                f.error.as_deref().unwrap_or("unknown error")
            );
            continue;
        }
        if cli.quiet {
            continue;
        }
        let status = match f.status {
            Status::Trimmed => "trimmed  ",
            Status::Unchanged => "unchanged",
            Status::Blank => "blank    ",
            Status::Failed => unreachable!(),
        };
        let size = match (f.original_size, f.output_size) {
            (Some(a), Some(b)) if a != b => format!("  {}x{} -> {}x{}", a[0], a[1], b[0], b[1]),
            _ => String::new(),
        };
        println!(
            "[{}] {status} {} -> {}{size}",
            f.index,
            name(&f.input),
            name(&f.output)
        );
        for w in &f.warnings {
            println!("      warning: {w}");
        }
    }
    let s = &report.summary;
    let mut tail = String::new();
    if let Some(d) = &report.settings.out_dir {
        tail.push_str(&format!(" Output: {d}."));
    }
    if report.dry_run {
        tail.push_str(" Dry run: nothing was changed.");
    } else if let Some(why) = &report.aborted {
        tail.push_str(&format!(" ABORTED: {why}."));
    } else if !report.settings.keep_originals {
        tail.push_str(" Originals of successful files were replaced.");
    }
    println!(
        "Done: {} file(s) - {} trimmed, {} unchanged, {} blank, {} failed.{tail}",
        s.total, s.trimmed, s.unchanged, s.blank, s.failed
    );
}
