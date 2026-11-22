#![allow(clippy::disallowed_methods)]

//! The tab bar's layout against Blink: in the `dom/tabs-model` vectors (Blink, real geometry)
//! the pointer handlers read the titles' rects in turn. Each title's width, measured from its
//! `innerText` in the `tabs:view` read before or after it (a rename can land between reads of
//! one step), must equal Blink's border box; laid out by `TabBarGeometry` from those widths,
//! every title must land where Blink put it: contiguous from the bar's left edge (the sidebar's
//! 300 px plus 10), 10 px below the 38 px title bar, 32 px tall.

#![cfg(target_os = "macos")]

use asrs_app::views::tab_bar::{TabBarGeometry, tab_style};
use asrs_parity::Value;
use asrs_parity::geometry::GeometryRecord;
use asrs_parity::vectors;
use asrs_ui::text::Measurer;

const VECTORS: [&str; 2] = ["dom/tabs-model/pointer", "dom/tabs-model/drag-shift"];
const BAR_LEFT: f64 = 310.0;
const BAR_TOP: f64 = 38.0;
const TAB_INSETS: f64 = 22.0;

/// The `innerText` of each `div.tab` in a `tabs:view` read.
fn titles(value: &Value) -> Option<Vec<String>> {
    let Value::Array(bar) = value.get("bar")? else {
        return None;
    };
    bar.iter()
        .map(|tab| match tab {
            Value::Array(pair) => match pair.get(1)? {
                Value::String(s) => Some(s.to_string_lossy()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn title_index(g: &GeometryRecord) -> Option<usize> {
    let mut segments = g.el.rsplit('>');
    let last = segments.next()?;
    let parent = segments.next()?;
    let classes = |s: &str| {
        s.rsplit_once(':')
            .map(|(n, _)| n.split('.').skip(1).map(str::to_owned).collect::<Vec<_>>())
    };
    let is_bar = classes(parent)?.iter().any(|c| c == "tab-bar");
    let is_tab = classes(last)?.iter().any(|c| c == "tab");
    (is_bar && is_tab && g.name == "getBoundingClientRect")
        .then(|| last.rsplit_once(':')?.1.parse().ok())
        .flatten()
}

fn rect(value: &Value) -> [f64; 4] {
    let Value::Array(items) = value else {
        panic!("not a rect: {value:?}");
    };
    let v: Vec<f64> = items.iter().filter_map(Value::as_f64).collect();
    [v[0], v[1], v[2], v[3]]
}

#[test]
fn tab_bar_positions_match_blink() {
    let mut compared = 0;
    let mut measured = 0;
    let mut measurer = Measurer::default();
    for id in VECTORS {
        let records = vectors::load(id).unwrap();
        let mut groups: Vec<Vec<(usize, [f64; 4])>> = Vec::new();
        let view = |record: &vectors::Record| {
            record
                .string("eval")
                .is_some_and(|e| e.starts_with("/*tabs:view*/"))
                .then(|| titles(&record.value("value").unwrap()).expect("tabs:view titles"))
        };
        let mut after = vec![None; records.len()];
        let mut next = None;
        for (i, record) in records.iter().enumerate().rev() {
            next = view(record).or(next);
            after[i] = next.clone();
        }
        let mut before = None;
        for (record, after) in records.iter().zip(&after) {
            before = view(record).or(before);
            let Ok(Some(g)) = GeometryRecord::parse(record) else {
                continue;
            };
            let Some(index) = title_index(&g) else {
                continue;
            };
            let r = rect(&g.value);
            let candidates: Vec<&String> = [&before, after]
                .into_iter()
                .filter_map(|t| t.as_ref()?.get(index))
                .collect();
            assert!(!candidates.is_empty(), "{id}: no title for tab {index}");
            let widths: Vec<f64> = candidates
                .iter()
                .map(|t| measurer.measure(&tab_style(), t, 1.0).0 + TAB_INSETS)
                .collect();
            assert!(
                widths.contains(&r[2]),
                "{id}: tab {index} {candidates:?} widths {widths:?}, Blink {}",
                r[2]
            );
            measured += 1;
            match groups.last_mut() {
                Some(group) if group.last().is_some_and(|(last, _)| index == last + 1) => {
                    group.push((index, r))
                }
                _ => groups.push(vec![(index, r)]),
            }
        }
        for group in groups {
            let (first, anchor) = group[0];
            let mut geometry = TabBarGeometry {
                bar: [anchor[0], anchor[1] - 10.0, 780.0, 50.0],
                widths: group.iter().map(|(_, r)| r[2]).collect(),
                device_pixel_ratio: 1.0,
                ..TabBarGeometry::default()
            };
            geometry.assign_scroll_left(0.0);
            for (index, blink) in group {
                let port = geometry.title_rect(index - first);
                assert_eq!(
                    [port.x, port.y, port.width, port.height],
                    blink,
                    "{id}: tab {index}"
                );
                compared += 1;
            }
            if first == 0 {
                assert_eq!(
                    anchor[..2],
                    [BAR_LEFT, BAR_TOP + 10.0],
                    "{id}: the bar's origin"
                );
            }
        }
    }
    assert!(compared > 0 && measured > 0);
    eprintln!("tab bar: {measured} title widths and {compared} title rects, 0 mismatches");
}
