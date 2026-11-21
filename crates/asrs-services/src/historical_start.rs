//! The file reads of main's `historical-start` for a plain path (F13 ports the rest): a `.dslog`
//! also sends its sibling `.dsevents`, and a failed read is a `null` slot.

use std::fs;

pub fn read_files(path: &str) -> Vec<Option<Vec<u8>>> {
    match path.strip_suffix(".dslog") {
        Some(stem) => vec![
            fs::read(path).ok(),
            fs::read(format!("{stem}.dsevents")).ok(),
        ],
        None => vec![fs::read(path).ok()],
    }
}
