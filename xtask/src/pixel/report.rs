use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use asrs_parity::pixel;

use super::CaptureResult;
use crate::Result;

const STYLE: &str = r"
:root { --bg: #fafafa; --fg: #1d1d1f; --muted: #6e6e73; --line: #d2d2d7; --pass: #1a7f37; --fail: #c62828; --card: #ffffff; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #161618; --fg: #f2f2f7; --muted: #a1a1a6; --line: #3a3a3c; --pass: #4cc76a; --fail: #ff6b6b; --card: #1f1f22; }
}
body { margin: 0; padding: 16px; background: var(--bg); color: var(--fg); font: 14px/1.45 ui-sans-serif, system-ui, sans-serif; }
h1 { font-size: 20px; margin: 0 0 4px; }
p.summary { color: var(--muted); margin: 0 0 16px; }
section { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 12px; margin-bottom: 16px; }
h2 { font-size: 15px; margin: 0 0 8px; }
.pass { color: var(--pass); }
.fail { color: var(--fail); }
table { border-collapse: collapse; width: 100%; margin-bottom: 8px; }
td, th { text-align: left; border-top: 1px solid var(--line); padding: 4px 8px; vertical-align: top; }
td.detail { color: var(--muted); word-break: break-word; }
.images { display: flex; gap: 8px; flex-wrap: wrap; }
figure { margin: 0; max-width: 100%; }
figure img { max-width: 100%; height: auto; border: 1px solid var(--line); image-rendering: pixelated; }
figcaption { color: var(--muted); font-size: 12px; }
";

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn file_stem(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn verdict(pass: bool) -> &'static str {
    if pass {
        r#"<span class="pass">pass</span>"#
    } else {
        r#"<span class="fail">FAIL</span>"#
    }
}

/// Writes report.html and the images it shows into `dir`, replacing an earlier report.
pub fn write(dir: &Path, results: &[CaptureResult]) -> Result<PathBuf> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let images = dir.join("images");
    fs::create_dir_all(&images).map_err(|e| format!("{}: {e}", images.display()))?;
    let passed = results.iter().filter(|r| r.pass()).count();
    let mut html = String::new();
    let _ = write!(
        html,
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Pixel gates</title><style>{STYLE}</style></head><body><h1>Pixel gates</h1><p class="summary">{passed} of {} captures pass. Diff colours: red fails gate 2, orange differs inside a text region, blue is masked or compared through its dump.</p>"#,
        results.len()
    );
    for result in results {
        let stem = file_stem(&result.label);
        let _ = write!(
            html,
            "<section><h2>{} {}</h2>",
            verdict(result.pass()),
            escape(&result.label)
        );
        if let Some(e) = &result.error {
            let _ = write!(html, r#"<p class="fail">{}</p>"#, escape(e));
        }
        if !result.outcome.checks.is_empty() {
            html.push_str("<table><tr><th>Gate</th><th>What</th><th></th><th>Detail</th></tr>");
            for c in &result.outcome.checks {
                let _ = write!(
                    html,
                    r#"<tr><td>{}</td><td>{}</td><td>{}</td><td class="detail">{}</td></tr>"#,
                    escape(c.gate),
                    escape(&c.what),
                    verdict(c.pass),
                    escape(&c.detail)
                );
            }
            html.push_str("</table>");
        }
        let mut figures = Vec::new();
        for (tag, image) in [
            ("upstream (sRGB)", result.reference.as_ref()),
            ("port", result.port.as_ref()),
            ("window diff", result.outcome.window_diff.as_ref()),
        ] {
            if let Some(image) = image {
                figures.push((tag.to_owned(), image));
            }
        }
        for (file, image) in &result.outcome.dump_diffs {
            figures.push((format!("{file} diff"), image));
        }
        if !figures.is_empty() {
            html.push_str(r#"<div class="images">"#);
            for (i, (tag, image)) in figures.iter().enumerate() {
                let name = format!("{stem}.{i}.png");
                pixel::write_png(&images.join(&name), image)?;
                let _ = write!(
                    html,
                    r#"<figure><img src="images/{name}" alt="{}" width="{}"><figcaption>{} ({}x{})</figcaption></figure>"#,
                    escape(tag),
                    image.width,
                    escape(tag),
                    image.width,
                    image.height
                );
            }
            html.push_str("</div>");
        }
        html.push_str("</section>");
    }
    html.push_str("</body></html>\n");
    let path = dir.join("report.html");
    fs::write(&path, html).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}
