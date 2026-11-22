#![allow(clippy::disallowed_methods)]

//! The tab bar's layout against Blink: in the `dom/tabs-model` vectors (Blink, real geometry)
//! the pointer handlers read the titles' rects in turn. Laid out by `TabBarGeometry` from
//! those widths, every title must land where Blink put it: contiguous from the bar's left edge
//! (the sidebar's 300 px plus 10), 10 px below the 38 px title bar, 32 px tall. Widths are not compared yet: the DOM text stack's emoji advance is
//! 1 px off Blink's (#197).

#![cfg(target_os = "macos")]

use asrs_app::views::tab_bar::TabBarGeometry;
use asrs_parity::Value;
use asrs_parity::geometry::GeometryRecord;
use asrs_parity::vectors;

const VECTORS: [&str; 2] = ["dom/tabs-model/pointer", "dom/tabs-model/drag-shift"];
const BAR_LEFT: f64 = 310.0;
const BAR_TOP: f64 = 38.0;

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
    for id in VECTORS {
        let records = vectors::load(id).unwrap();
        let mut groups: Vec<Vec<(usize, [f64; 4])>> = Vec::new();
        for record in &records {
            let Ok(Some(g)) = GeometryRecord::parse(record) else {
                continue;
            };
            let Some(index) = title_index(&g) else {
                continue;
            };
            let r = rect(&g.value);
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
    assert!(compared > 0);
    eprintln!("tab bar positions: {compared} title rects, 0 mismatches");
}
