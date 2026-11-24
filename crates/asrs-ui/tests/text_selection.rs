//! Replays the `selection/gestures` vectors: trusted input to selectable pages in Blink, with the
//! selection's ends and the copied text after each operation, on each platform's editing
//! behaviour.

#![allow(clippy::disallowed_methods)]

use std::borrow::Cow;
use std::path::Path;

use asrs_parity::json::{self, Json};
use asrs_parity::vectors::{self, Record};
use asrs_ui::selection::document::{clipboard_text, end};
use asrs_ui::selection::{
    Behavior, Button, Caret, Document, Hit, Place, Position, Press, TextSelection,
};

const PLATFORMS: [&str; 3] = ["macos-aarch64", "linux-aarch64", "windows-aarch64"];

struct Page {
    texts: Vec<String>,
    places: Vec<Place>,
}

impl Document for Page {
    fn len(&self) -> usize {
        self.texts.len()
    }

    fn text(&self, block: usize) -> Cow<'_, str> {
        Cow::Borrowed(&self.texts[block])
    }

    fn place(&self, block: usize) -> Place {
        self.places[block]
    }
}

fn text(json: &Json) -> String {
    json.as_str().expect("a string").to_string_lossy()
}

fn index(json: &Json) -> usize {
    json.as_f64().expect("a number") as usize
}

fn flag(json: &Json, key: &str) -> bool {
    json.get(key) == Some(&Json::Bool(true))
}

fn pages() -> Vec<(String, Vec<Place>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let file = root.join("parity/specs/selection/gestures.entry.json");
    let spec = json::parse(&std::fs::read_to_string(file).expect("the spec")).expect("JSON");
    let pages = spec.get("fixture").and_then(|f| f.get("pages"));
    let pages = pages.and_then(Json::as_array).expect("pages");
    pages
        .iter()
        .map(|page| {
            let blocks = page.get("blocks").and_then(Json::as_array).expect("blocks");
            let places = blocks.iter().map(|block| Place {
                cell: flag(block, "cell"),
                tab: flag(block, "tab"),
                newline_before: flag(block, "before"),
                newline_after: flag(block, "after"),
                table_end: flag(block, "table_end"),
                closed: flag(block, "closed"),
                after_other: flag(block, "other"),
                gap: index(block.get("gap").expect("gap")),
            });
            (text(page.get("name").expect("name")), places.collect())
        })
        .collect()
}

/// A UTF-16 offset as a byte offset.
fn byte(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    let at = text.char_indices().find(|(_, c)| {
        let found = units >= utf16;
        units += c.len_utf16();
        found
    });
    at.map_or(text.len(), |(byte, _)| byte)
}

fn position(page: &Page, at: &Json) -> Position {
    let at = at.as_array().expect("a position");
    let block = index(&at[0]);
    Position {
        block,
        offset: byte(&page.texts[block], index(&at[1])),
    }
}

/// What the pointer is on at a target.
fn hit(page: &Page, target: &Json) -> Hit {
    let target = target.as_array().expect("a target");
    let block = index(&target[1]);
    let kind = text(&target[0]);
    if kind == "other" {
        let next = (block < page.len()).then_some(Position { block, offset: 0 });
        return Hit {
            caret: next.or_else(|| end(page)).map(Caret::new),
            selectable: false,
            text: false,
            link: None,
        };
    }
    let content = &page.texts[block];
    let offset = match kind.as_str() {
        "cell" => 0,
        "after" => content.len(),
        "char" => byte(content, index(&target[2]) + index(&target[3])),
        other => panic!("target {other:?}"),
    };
    let char = kind == "char";
    let link = target.get(5).and_then(Json::as_array).filter(|_| char);
    Hit {
        caret: Some(Caret {
            position: Position { block, offset },
            line_end: char && target[4] == Json::Bool(true),
        }),
        selectable: true,
        text: char,
        link: link.map(|l| (byte(content, index(&l[0])), byte(content, index(&l[1])))),
    }
}

struct Replay {
    page: Page,
    behavior: Behavior,
    state: TextSelection,
    pressed: Option<Json>,
}

impl Replay {
    fn run(&mut self, op: &[Json]) {
        let page = &self.page;
        let press = |button, click_count, shift| Press {
            button,
            click_count,
            shift,
        };
        match text(&op[0]).as_str() {
            "all" => self.state.select_all(page),
            "press" => {
                let clicks = index(&op[2]);
                let shift = op[3] == Json::Bool(true);
                let press = press(Button::Left, clicks, shift);
                self.state.press(page, hit(page, &op[1]), press);
                self.pressed = Some(op[1].clone());
            }
            "move" => {
                // The vectors drag no selected text.
                let caret = hit(page, &op[1]).caret;
                self.state.drag(page, caret, 1000.0);
            }
            "release" => {
                let moved = self.pressed.as_ref() != Some(&op[1]);
                self.state.release(page, moved);
            }
            "menu" => {
                let hit = hit(page, &op[1]);
                self.state.press(page, hit, press(Button::Right, 1, false));
                self.state.release(page, false);
            }
            other => panic!("operation {other:?}"),
        }
    }

    fn copied(&self) -> String {
        let text = self.state.text(&self.page).unwrap_or_default();
        let text = clipboard_text(&text).replace("\r\n", "\n");
        if self.behavior == Behavior::Windows {
            text.replace('\n', "\r\n")
        } else {
            text
        }
    }
}

fn start(record: &Record, pages: &[(String, Vec<Place>)]) -> Option<Replay> {
    let texts = record.field("texts").and_then(Json::as_array)?;
    let name = record.string("page").expect("page");
    let behavior = match record.string("platform").expect("platform").as_str() {
        "darwin" => Behavior::Mac,
        "win32" => Behavior::Windows,
        _ => Behavior::Unix,
    };
    let (_, places) = pages.iter().find(|(n, _)| *n == name).expect("the page");
    assert_eq!(texts.len(), places.len(), "{name}");
    Some(Replay {
        page: Page {
            texts: texts.iter().map(text).collect(),
            places: places.clone(),
        },
        behavior,
        state: TextSelection::new(behavior),
        pressed: None,
    })
}

#[test]
fn gestures_select_what_blink_selects() {
    let pages = pages();
    let mut failures = Vec::new();
    let mut compared = 0;
    for platform in PLATFORMS {
        let id = format!("selection/gestures/{platform}");
        let records = vectors::load(&id).unwrap_or_else(|e| panic!("{id}: {e}"));
        let mut replay: Option<Replay> = None;
        let mut sequence = None;
        let mut broken = false;
        for (i, record) in records.iter().enumerate() {
            if let Some(next) = start(record, &pages) {
                replay = Some(next);
                sequence = None;
                continue;
            }
            let replay = replay.as_mut().expect("a page record first");
            let number = record.field("sequence").map(index);
            if number != sequence {
                sequence = number;
                broken = false;
                replay.state = TextSelection::new(replay.behavior);
            }
            let op = record.field("op").and_then(Json::as_array).expect("op");
            replay.run(op);
            compared += 1;
            let expected = record.field("ends").and_then(Json::as_array);
            let expected = expected.map(|e| [&e[0], &e[1]].map(|p| position(&replay.page, p)));
            let ends = replay.state.ends(&replay.page);
            let copied = match record.field("copied") {
                Some(Json::Null) | None => None,
                Some(copied) => Some(text(copied)),
            };
            let same_text = copied.as_ref().is_none_or(|c| *c == replay.copied());
            if !broken && (ends != expected || !same_text) {
                broken = true;
                failures.push(format!(
                    "{id} record {i} {}/{number:?} {}:\n  ends {ends:?}\n  want {expected:?}\n  copied {:?}\n  want   {copied:?}",
                    record.string("page").unwrap_or_default(),
                    record.field("op").map(Json::to_compact).unwrap_or_default(),
                    replay.copied(),
                ));
            }
        }
    }
    let shown = failures.len().min(12);
    assert!(
        failures.is_empty(),
        "{} of {compared}:\n{}",
        failures.len(),
        failures[..shown].join("\n")
    );
    assert!(compared > 4000, "{compared} operations");
}
