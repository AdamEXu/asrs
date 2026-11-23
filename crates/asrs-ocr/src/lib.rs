#![allow(clippy::disallowed_methods)]
//! `VideoProcessor.readTimerText`: tesseract.js as upstream runs it.

mod engine;

pub use engine::{OcrError, Worker};
