//! Core of `trim-image`: remove white margins around (patent) drawings and
//! number the results `도 1`, `도 2`, …
//!
//! Shared by the command-line tool (`trim-image`) and the desktop app
//! (`trim-image-gui`). The entry point is [`batch::run`].

pub mod batch;
pub mod codec;
pub mod naming;
pub mod trim;

pub use batch::{ErrorCode, FileReport, Options, Progress, Report, Status, Summary, run};
pub use trim::{Background, Rect};
