//! End-to-end tests of a batch run on a temporary folder.

use std::fs;
use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage};
use trim_core::{ErrorCode, Options, Status, run};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trim-core-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// White 200x100 image with a black 20x10 box at (50, 30).
fn drawing(path: &Path) {
    let mut img = RgbImage::from_pixel(200, 100, Rgb([255, 255, 255]));
    for y in 30..40 {
        for x in 50..70 {
            img.put_pixel(x, y, Rgb([0, 0, 0]));
        }
    }
    img.save(path).unwrap();
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn opts(dir: &Path) -> Options {
    Options {
        paths: vec![dir.to_path_buf()],
        ..Options::default()
    }
}

#[test]
fn dry_run_changes_nothing() {
    let d = temp_dir("dry");
    drawing(&d.join("fig2.png"));
    drawing(&d.join("fig10.png"));
    let report = run(&Options {
        dry_run: true,
        ..opts(&d)
    })
    .unwrap();
    assert!(report.ok && !report.committed);
    assert_eq!(report.summary.trimmed, 2);
    // natural order: fig2 before fig10
    assert!(report.files[0].input.ends_with("fig2.png"));
    assert!(report.files[1].output.ends_with("도 2.png"));
    assert_eq!(report.files[0].output_size, Some([20, 10]));
    assert_eq!(names(&d), ["fig10.png", "fig2.png"]);
}

#[test]
fn commit_renames_and_removes_originals() {
    let d = temp_dir("commit");
    drawing(&d.join("a.png"));
    drawing(&d.join("b.png"));
    let report = run(&opts(&d)).unwrap();
    assert!(report.ok && report.committed);
    assert_eq!(names(&d), ["도 1.png", "도 2.png"]);
    assert_eq!(image::open(d.join("도 1.png")).unwrap().width(), 20);

    // Running again is a no-op that keeps the numbering.
    let again = run(&opts(&d)).unwrap();
    assert!(again.files.iter().all(|f| f.status == Status::Unchanged));
    assert_eq!(names(&d), ["도 1.png", "도 2.png"]);
}

#[test]
fn one_failure_changes_nothing() {
    let d = temp_dir("abort");
    drawing(&d.join("a.png"));
    fs::write(d.join("b.png"), b"not an image").unwrap();
    let report = run(&opts(&d)).unwrap();
    assert!(!report.ok && !report.committed && report.aborted.is_some());
    assert_eq!(report.files[1].error_code, Some(ErrorCode::Decode));
    assert_eq!(names(&d), ["a.png", "b.png"]);
}

#[test]
fn out_dir_keeps_originals_and_refuses_to_clobber() {
    let d = temp_dir("out");
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("도 1.png"), b"someone else's file").unwrap();
    drawing(&d.join("a.png"));
    let report = run(&Options {
        out_dir: Some(out.clone()),
        ..opts(&d)
    })
    .unwrap();
    assert_eq!(report.files[0].error_code, Some(ErrorCode::Exists));
    assert!(d.join("a.png").exists());
    assert_eq!(
        fs::read(out.join("도 1.png")).unwrap(),
        b"someone else's file"
    );
}
