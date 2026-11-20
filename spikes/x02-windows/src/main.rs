#[cfg(windows)]
mod dom;
#[cfg(windows)]
mod dwrite;
#[cfg(windows)]
mod scenes;
mod shape;

const USAGE: &str = "usage: x02 <command>

  lut <reference dir> <out.bin>                 fit the mask-gamma table (asrs-cli canvas lut)
  render <scenes dir> <out dir> <lut.bin> [<scene>...]
                                                rasterize scenes with the DirectWrite backend
  measure <measure.jsonl>                       widths against Chromium's measureText
  dom <dom dir> <out dir>                       DOM text scenes with ClearType masks

Environment: X02_GRIDFIT=0|1 (default 1), X02_CLEARTYPE=0|1 (default 1).";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    let result = scenes::run(&args);
    #[cfg(not(windows))]
    let result: Result<(), String> = Err(format!("x02 runs on Windows only ({args:?})"));
    if let Err(e) = result {
        eprintln!("{e}\n\n{USAGE}");
        std::process::exit(1);
    }
}
