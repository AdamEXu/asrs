//! The `.dslog` worker entry for `asrs-cli decode`: `dsLogWorker` on the real port.

use asrs_formats::worker;

pub fn run(files: &[Option<Vec<u8>>]) -> worker::Outcome {
    let refs: Vec<Option<&[u8]>> = files.iter().map(|f| f.as_deref()).collect();
    asrs_formats::dslog::run_worker(&refs)
}
