#![allow(clippy::disallowed_methods)]

//! Where Electron put windows in `parity/windows/*.json` (`cargo xtask app windows`), against
//! `asrs_app::windows::placement` and the X11 displays.

use asrs_app::windows::{WindowKind, WindowOptions, placement};
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use asrs_services::windows::Rect;
use asrs_ui::platform::displays::Displays;

fn records(os: &str) -> Vec<Json> {
    let path = vectors::workspace_root().join(format!("parity/windows/{os}.json"));
    let file = json::parse(&std::fs::read_to_string(path).expect("recording")).expect("JSON");
    file.get("records")
        .and_then(Json::as_array)
        .expect("records")
        .to_vec()
}

fn area(value: &Json) -> [f64; 4] {
    let items = value.as_array().expect("an area");
    [0, 1, 2, 3].map(|i| items[i].as_f64().expect("a number"))
}

fn get<'a>(value: &'a Json, key: &str) -> &'a Json {
    value.get(key).unwrap_or_else(|| panic!("no {key}"))
}

fn list<'a>(value: &'a Json, key: &str) -> &'a [Json] {
    get(value, key).as_array().expect("an array")
}

fn options(value: &Json) -> WindowOptions {
    let number = |key| value.get(key).and_then(Json::as_f64);
    let is = |key: &str, wanted: &str| value.get(key).is_some_and(|v| v.to_compact() == wanted);
    WindowOptions {
        kind: WindowKind::Hub,
        x: number("x"),
        y: number("y"),
        width: number("width").expect("width"),
        height: number("height").expect("height"),
        min_width: None,
        min_height: None,
        max_width: None,
        use_content_size: is("useContentSize", "true"),
        resizable: value.get("resizable").map(|_| is("resizable", "true")),
        always_on_top: false,
        fullscreenable: None,
        parent: None,
        modal: false,
        vibrancy: None,
        title_bar_style: is("titleBarStyle", "\"hiddenInset\"").then_some("hiddenInset"),
    }
}

fn rect([x, y, width, height]: [f64; 4]) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn mac_windows_open_where_electrons_do() {
    let mut compared = 0;
    for record in records("macos") {
        let display =
            &list(&record, "displays")[get(&record, "primary").as_f64().unwrap() as usize];
        let bounds = area(get(display, "bounds"));
        let work_area = rect(area(get(display, "workArea")));
        let placement = list(&record, "placement");
        let framed = placement.iter().find(|p| {
            let options = get(p, "options");
            options.get("titleBarStyle").is_none() && options.get("useContentSize").is_none()
        });
        let framed = framed.expect("a window with a frame");
        let title_bar = area(get(framed, "bounds"))[3] - area(get(framed, "content"))[3];
        for placed in placement {
            let options = options(get(placed, "options"));
            let ours = placement::mac(&options, (bounds[2], bounds[3]), work_area, title_bar);
            let mut size = ours.size;
            if options.use_content_size {
                size.1 += title_bar;
            }
            assert_eq!(
                [ours.origin.0, ours.origin.1, size.0, size.1],
                area(get(placed, "bounds")),
                "{}",
                placed.to_compact()
            );
            compared += 1;
        }
    }
    assert_eq!(compared, 17);
}

#[test]
fn x11_windows_open_where_electrons_do() {
    let records = records("linux");
    let mut compared = [0; 2];
    for pair in records.chunks(2) {
        let crtcs: Vec<[i32; 4]> = list(&pair[0], "displays")
            .iter()
            .map(|display| area(get(display, "bounds")).map(|v| v as i32))
            .collect();
        for record in pair {
            let scale = get(record, "scale").as_f64().expect("scale");
            let net = match get(record, "netWorkArea") {
                Json::Null => None,
                net => Some(area(net).map(|v| v as i32)),
            };
            let primary = get(record, "primary").as_f64().expect("primary") as usize;
            let displays = Displays::new(&crtcs, primary, net, scale);
            let at = format!("{crtcs:?} {net:?} {scale}");
            for (ours, theirs) in displays.displays.iter().zip(list(record, "displays")) {
                assert_eq!(ours.bounds, area(get(theirs, "bounds")), "{at}: bounds");
                assert_eq!(ours.work_area, area(get(theirs, "workArea")), "{at}");
                compared[0] += 1;
            }
            for placed in list(record, "placement") {
                let options = options(get(placed, "options"));
                let window = [0.0, 0.0, options.width, options.height];
                let on = displays.of_window(window).expect("a display");
                let (x, y) = placement::x11(&options, rect(on.work_area));
                let theirs = area(get(placed, "bounds"));
                assert_eq!([x, y], theirs[..2], "{at}: {}", placed.to_compact());
                compared[1] += 1;
            }
        }
    }
    assert_eq!(compared, [44, 408]);
}
