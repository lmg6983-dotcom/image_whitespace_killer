# Using `trim-image` from an agent

`trim-image` removes white margins from patent drawing images and (by default)
renames them `도 1.<ext>`, `도 2.<ext>`, … while deleting the originals. It is
non-interactive and has no external dependencies.

## Recommended workflow

1. **Preview**: `trim-image <dir-or-files> --dry-run --json`
   Check `summary`, each file's `status` and `output`, and look for `failed` entries.
2. **Apply**: run the same command without `--dry-run`.
3. **Verify**: exit code `0` and `"committed": true`.

Prefer `-o <out-dir>` when the originals must be preserved. Without `-o`, the
originals of successful files are **deleted** (same as the legacy `.bat`).

## Exit codes

- `0`: all files succeeded and changes were committed (or dry-run found no failures).
- `1`: at least one file failed. Without `--allow-partial` **nothing on disk changed**.
  Fix the cause (see `files[].error`) and rerun.
- `2`: bad arguments or fatal error. Nothing changed. With `--json`, stdout is
  `{"ok": false, "fatal": true, "committed": false, "error": "..."}`.

## JSON report (`--json`, stdout)

```jsonc
{
  "ok": true,                 // no file failed
  "dry_run": false,
  "committed": true,          // changes applied to disk
  "aborted": null,            // reason string if the batch was rolled back
  "settings": {
    "fuzz_percent": 5.0, "background": "#FFFFFF", "name_template": "도 {num}",
    "start": 1, "out_dir": null, "keep_originals": false, "overwrite": false,
    "allow_partial": false, "margin": 0, "jpeg_quality": 95
  },
  "summary": { "total": 3, "trimmed": 2, "unchanged": 1, "blank": 0, "failed": 0 },
  "files": [
    {
      "index": 1,                              // the {num} value
      "input": "/abs/path/fig1.png",
      "output": "/abs/path/도 1.png",
      "status": "trimmed",                     // trimmed | unchanged | blank | failed
      "format": "png",
      "original_size": [2480, 3508],           // [width, height]
      "output_size": [1802, 2406],
      "crop": { "x": 339, "y": 551, "width": 1802, "height": 2406 },
      "background": "#FFFFFF",
      "dpi": [300.0, 300.0],                   // null if the source declares none
      "written": true,
      "original_removed": true,                // original deleted or replaced
      "warnings": [],
      "error": null
    }
  ]
}
```

Statuses:

- `trimmed`: margins removed and the result re-encoded.
- `unchanged`: nothing to trim. The file was copied byte-for-byte (renamed if the template says so).
- `blank`: the whole image is background. It is left uncropped and gets a warning; check it manually.
- `failed`: see `error`. The original is untouched.

All paths in the report are absolute. Human-readable output (without `--json`)
is for people only; parse JSON instead.

## Useful flags

| Flag | Purpose |
|---|---|
| `-o DIR` | write results to `DIR`; originals are never touched |
| `--keep-names` | keep file names (trim in place) |
| `--name 'FIG{num:02}'` | custom names; `{num}`, `{num:03}`, `{stem}` |
| `--start N` | first number |
| `-k` | keep originals next to outputs |
| `--fuzz 10` | treat near-white (scanner noise, JPEG artefacts) as background |
| `--bg auto` | background colour taken from the image corners |
| `--margin 20` | keep 20 px of white around the drawing |
| `--overwrite` | allow replacing files that are not part of the batch |
| `--allow-partial` | commit successful files even if others failed |

## Notes

- Directories are scanned non-recursively in natural order (`fig2` before `fig10`).
  Hidden files (`.*`) are skipped. Explicit file arguments keep their order.
  Numbering runs across all inputs in that order.
- Re-running on an already processed folder is safe and keeps the numbering.
- If a target name such as `도 1.png` exists and is not one of the inputs, the
  file fails with an "already exists" error rather than overwriting it.
