//! trim-image — command-line tool: remove white margins around patent
//! drawings and number them "도 1, 도 2, ...". Non-interactive, with
//! `--dry-run`, `--json` reports and meaningful exit codes (see AGENTS.md).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use trim_core::{Background, Options, Report, Status, codec, trim};

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

fn run(cli: &Cli) -> anyhow::Result<Report> {
    let opts = Options {
        paths: cli.paths.clone(),
        fuzz: cli.fuzz,
        background: cli.bg,
        name_template: if cli.keep_names {
            "{stem}".into()
        } else {
            cli.name.clone()
        },
        start: cli.start,
        out_dir: cli.out_dir.clone(),
        keep_originals: cli.keep_originals,
        overwrite: cli.overwrite,
        allow_partial: cli.allow_partial,
        margin: cli.margin,
        jpeg_quality: cli.quality,
        dry_run: cli.dry_run,
    };
    trim_core::run(&opts)
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
