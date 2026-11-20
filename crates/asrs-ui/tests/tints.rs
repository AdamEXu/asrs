#![cfg(target_os = "macos")]

use std::collections::BTreeSet;

use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};
use asrs_ui::filter::FilterChain;
use asrs_ui::theme::{self, chains};

fn records() -> Vec<Record> {
    vectors::load("ui/tints-macos").expect("parity/vectors/ui/tints-macos is committed")
}

fn rgba(r: &Record) -> [u8; 4] {
    let a = r.field("rgba").and_then(Json::as_array).expect("rgba");
    [0, 1, 2, 3].map(|i| a[i].as_f64().expect("channel") as u8)
}

fn hex(s: &str) -> [u8; 4] {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap();
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
}

#[test]
fn every_opaque_probe_matches_chromium_exactly() {
    let mut checked = 0;
    for r in records() {
        let Some(chain) = r.string("chain") else {
            continue;
        };
        if r.field("opacity").and_then(Json::as_f64) != Some(1.0) {
            continue;
        }
        let input = hex(&r.string("input").unwrap());
        let got = FilterChain::parse(&chain).unwrap().apply(input);
        assert_eq!(got, rgba(&r), "{chain} on {input:?} (line {})", r.line);
        checked += 1;
    }
    assert_eq!(checked, 24 * 5 * 2 * 2);
}

#[test]
fn theme_lists_exactly_the_upstream_chains() {
    let upstream: BTreeSet<String> = records().iter().filter_map(|r| r.string("chain")).collect();
    let ours: BTreeSet<String> = chains::ALL.iter().map(|s| s.to_string()).collect();
    assert_eq!(ours, upstream);
}

#[test]
fn hover_and_active_backgrounds() {
    for r in records() {
        let scene = r.string("scene").unwrap();
        if scene.starts_with("ui-icons") {
            continue;
        }
        let Some(button) = r.field("button").and_then(Json::as_f64) else {
            continue;
        };
        let t = if scene.ends_with("-dark") {
            theme::DARK
        } else {
            theme::LIGHT
        };
        let expected = match (scene.split('-').nth(1).unwrap(), button as u8) {
            ("hover", 0) => t.button_hover,
            ("active", 0) => t.button_active,
            _ => t.background,
        };
        assert_eq!(
            rgba(&r),
            hex(&format!("{expected:06x}")),
            "{scene} button {button}"
        );
    }
}
