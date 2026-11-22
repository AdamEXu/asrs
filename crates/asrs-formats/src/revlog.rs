//! `@rev-robotics/revlog-converter` 2.0.0 (`dist/lib/revlogParser.js`): converts a REVLib
//! StatusLogger `.revlog` file (format v1 or v2) into WPILOG bytes. Upstream's main process
//! calls `parseREVLOG(logPath, writable)` on `historical-start` and hands the collected chunks
//! to the WPILOG worker (`main/electron/main.ts`).

pub mod dbc;

use asrs_js::string::JsError;

/// The DBC files the package ships in `dist/resources`, read with `encoding: "ascii"`.
pub const SPARK_DBC: &str = include_str!("revlog/spark.public.dbc");
pub const SERVO_HUB_DBC: &str = include_str!("revlog/servo_hub.public.dbc");
pub const ENCODER_DBC: &str = include_str!("revlog/encoder.public.dbc");
pub const A301_DBC: &str = include_str!("revlog/a301.public.dbc");

/// `parseREVLOG(input, writable)` with the whole file as input: `Ok` is every byte written
/// to the writable when the promise resolves, `Err` is the rejection (`name` and `message`).
pub fn convert(input: &[u8]) -> Result<Vec<u8>, JsError> {
    let _ = input;
    todo!("F14: port parseREVLOG")
}
