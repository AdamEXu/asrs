//! Lines between asrs and `asrs-webview`, which shows the page on Linux.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageEvent {
    Load {
        hash: String,
    },
    Hash {
        id: u64,
        hash: String,
    },
    /// A link to another site, which opens a new window.
    Open(String),
    /// The page could not be shown, or its process ended.
    Failed(String),
}

pub fn report(message: &str) -> Option<PageEvent> {
    let mut parts = message.splitn(3, ' ');
    let (kind, id, hash) = (parts.next()?, parts.next()?, parts.next()?);
    let (id, hash) = (id.parse().ok()?, hash.to_owned());
    match kind {
        "load" => Some(PageEvent::Load { hash }),
        "hash" => Some(PageEvent::Hash { id, hash }),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToHost {
    /// In device pixels, with the scale factor of the window.
    Bounds([f64; 4], f64),
    Visible(bool),
    Dark(bool),
    Evaluate(String),
    Query(u64, String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum FromHost {
    Ready,
    Event(PageEvent),
    Result(u64, String),
    /// A press on the page, in device pixels from its corner.
    Press(u32, [f64; 2]),
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        out.push(match c {
            '\\' => match chars.next() {
                Some('n') => '\n',
                Some('r') => '\r',
                Some(c) => c,
                None => '\\',
            },
            c => c,
        });
    }
    out
}

fn flag(text: &str) -> Option<bool> {
    match text {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

impl ToHost {
    pub fn line(&self) -> String {
        match self {
            ToHost::Bounds([x, y, w, h], scale) => format!("bounds {x} {y} {w} {h} {scale}\n"),
            ToHost::Visible(on) => format!("visible {}\n", u8::from(*on)),
            ToHost::Dark(on) => format!("dark {}\n", u8::from(*on)),
            ToHost::Evaluate(script) => format!("evaluate {}\n", escape(script)),
            ToHost::Query(id, script) => format!("query {id} {}\n", escape(script)),
        }
    }

    pub fn parse(line: &str) -> Option<ToHost> {
        let (kind, rest) = line.split_once(' ')?;
        match kind {
            "bounds" => {
                let numbers: Vec<f64> = rest
                    .split(' ')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .ok()?;
                let [x, y, w, h, scale] = numbers[..] else {
                    return None;
                };
                Some(ToHost::Bounds([x, y, w, h], scale))
            }
            "visible" => Some(ToHost::Visible(flag(rest)?)),
            "dark" => Some(ToHost::Dark(flag(rest)?)),
            "evaluate" => Some(ToHost::Evaluate(unescape(rest))),
            "query" => {
                let (id, script) = rest.split_once(' ')?;
                Some(ToHost::Query(id.parse().ok()?, unescape(script)))
            }
            _ => None,
        }
    }
}

impl FromHost {
    pub fn line(&self) -> String {
        match self {
            FromHost::Ready => "ready\n".to_owned(),
            FromHost::Event(PageEvent::Load { hash }) => format!("load 0 {}\n", escape(hash)),
            FromHost::Event(PageEvent::Hash { id, hash }) => {
                format!("hash {id} {}\n", escape(hash))
            }
            FromHost::Event(PageEvent::Open(url)) => format!("open {}\n", escape(url)),
            FromHost::Event(PageEvent::Failed(error)) => format!("failed {}\n", escape(error)),
            FromHost::Result(id, json) => format!("result {id} {}\n", escape(json)),
            FromHost::Press(button, [x, y]) => format!("press {button} {x} {y}\n"),
        }
    }

    pub fn parse(line: &str) -> Option<FromHost> {
        if line == "ready" {
            return Some(FromHost::Ready);
        }
        let (kind, rest) = line.split_once(' ')?;
        match kind {
            "load" | "hash" => Some(FromHost::Event(report(&unescape(line))?)),
            "open" => Some(FromHost::Event(PageEvent::Open(unescape(rest)))),
            "failed" => Some(FromHost::Event(PageEvent::Failed(unescape(rest)))),
            "result" => {
                let (id, json) = rest.split_once(' ')?;
                Some(FromHost::Result(id.parse().ok()?, unescape(json)))
            }
            "press" => {
                let mut parts = rest.splitn(3, ' ');
                let button = parts.next()?.parse().ok()?;
                let x = parts.next()?.parse().ok()?;
                let y = parts.next()?.parse().ok()?;
                Some(FromHost::Press(button, [x, y]))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports() {
        assert_eq!(
            report("load 0 #/"),
            Some(PageEvent::Load { hash: "#/".into() })
        );
        assert_eq!(
            report("hash 3 "),
            Some(PageEvent::Hash {
                id: 3,
                hash: String::new()
            })
        );
        assert_eq!(report("hash x #/"), None);
        assert_eq!(report("other 1 #/"), None);
    }

    #[test]
    fn lines_round_trip() {
        let text = "a\\n\nb\r 'c' \\\\ é";
        for message in [
            ToHost::Bounds([1.5, 2.0, 300.0, 400.0], 1.25),
            ToHost::Visible(true),
            ToHost::Dark(false),
            ToHost::Evaluate(text.into()),
            ToHost::Query(7, text.into()),
        ] {
            let line = message.line();
            assert_eq!(line.matches('\n').count(), 1);
            assert_eq!(ToHost::parse(line.trim_end_matches('\n')), Some(message));
        }
        for message in [
            FromHost::Ready,
            FromHost::Event(PageEvent::Load { hash: text.into() }),
            FromHost::Event(PageEvent::Hash {
                id: 4,
                hash: String::new(),
            }),
            FromHost::Event(PageEvent::Open(text.into())),
            FromHost::Event(PageEvent::Failed(text.into())),
            FromHost::Result(9, text.into()),
            FromHost::Press(1, [10.5, 0.0]),
        ] {
            let line = message.line();
            assert_eq!(line.matches('\n').count(), 1);
            assert_eq!(FromHost::parse(line.trim_end_matches('\n')), Some(message));
        }
        assert_eq!(ToHost::parse("bounds 1 2 3 4"), None);
        assert_eq!(FromHost::parse("press 1 2"), None);
        assert_eq!(FromHost::parse("Gtk-WARNING: something"), None);
    }
}
