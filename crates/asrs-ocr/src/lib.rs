//! The OCR of `src/main/electron/VideoProcessor.ts`: `readTimerText` and the tesseract.js 6.0.1
//! workers under it. The workers run the WebAssembly core and the model the upstream app ships, so
//! the recognized text is the upstream app's on every target.

mod engine;
mod timer;

pub use engine::{OcrError, Worker};
pub use timer::{TIMER_RECTS, TimerError, TimerReader, TimerRect};
