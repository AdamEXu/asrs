//! `asrs --self-test [<image> <left> <top> <width> <height>]`: checks, without a window, what a
//! package can get wrong, for `cargo xtask smoke`. Prints one `name: value` line per check and
//! ends with exit code 1 when one fails. With an image it reads the text in the rectangle, which
//! runs the OCR's compiled code under the program's own code signature.

use std::path::Path;

use asrs_field3d::managers::{april_tag_texture_path, decode_png_texture};
use asrs_js::string::JsString;

use crate::{licenses, main_process};

const FLAG: &str = "--self-test";

pub fn run_from_args(argv: &[String]) -> Option<i32> {
    let (_, args) = argv.split_first()?;
    let (flag, image) = args.split_first()?;
    if flag != FLAG {
        return None;
    }
    let mut code = 0;
    for (name, result) in checks(image) {
        match result {
            Ok(value) => println!("{name}: {value}"),
            Err(error) => {
                println!("{name}: failed: {error}");
                code = 1;
            }
        }
    }
    Some(code)
}

type Check = (&'static str, Result<String, String>);

fn checks(image: &[String]) -> Vec<Check> {
    let mut checks = vec![
        ("library", library()),
        ("assets", bundled_assets()),
        ("textures", tag_textures()),
        ("licenses", license_list()),
        ("documentation", Ok(documentation())),
    ];
    if !image.is_empty() {
        checks.push(("text", read_text(image)));
    }
    checks
}

fn library() -> Result<String, String> {
    let path = asrs_js::math::libm::load().map_err(str::to_owned)?;
    let sin = asrs_js::math::sin(1.0);
    Ok(format!("{} sin(1)={:016x}", path.display(), sin.to_bits()))
}

fn bundled_assets() -> Result<String, String> {
    let dir = main_process::bundled_assets_dir();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("config.json").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    if names.is_empty() {
        return Err(format!("no assets in {}", dir.display()));
    }
    Ok(names.join(" "))
}

fn tag_textures() -> Result<String, String> {
    let root = main_process::app_root();
    for name in ["16h5/smile", "16h5/029", "36h11/smile", "36h11/586"] {
        let url = JsString::from(format!("../www/textures/apriltag-{name}.png"));
        let path = april_tag_texture_path(&root, &url).ok_or("not a texture's URL")?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        decode_png_texture(&bytes).ok_or_else(|| format!("{}: not a PNG", path.display()))?;
    }
    Ok(root.display().to_string())
}

fn license_list() -> Result<String, String> {
    let entries = licenses::packaged_count()?;
    Ok(entries.to_string())
}

fn documentation() -> String {
    let dir = main_process::documentation_dir();
    if dir.join("index.html").is_file() {
        dir.display().to_string()
    } else {
        "none".to_owned()
    }
}

fn read_text(args: &[String]) -> Result<String, String> {
    let [image, rectangle @ ..] = args else {
        return Err("no image".to_owned());
    };
    let rectangle: Vec<f64> = rectangle.iter().filter_map(|n| n.parse().ok()).collect();
    let rectangle = <[f64; 4]>::try_from(rectangle)
        .map_err(|_| "the image needs <left> <top> <width> <height>".to_owned())?;
    let image = std::fs::read(Path::new(image)).map_err(|e| format!("{image}: {e}"))?;
    let mut worker = asrs_ocr::Worker::new().map_err(|e| e.to_string())?;
    worker
        .recognize(&image, rectangle)
        .map(|text| format!("{text:?}"))
        .map_err(|e| e.to_string())
}
