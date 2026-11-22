//! Electron accelerator strings (`StringToAccelerator` in `shell/browser/ui/accelerator_util.cc`
//! and `KeyboardCodeFromStr` in `shell/common/keyboard_util.cc`, Electron 39), and the macOS key
//! equivalents `ElectronMenuController` makes of them.

use asrs_services::window_manager::Platform;

/// A key, by its unshifted US-layout identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// GPUI's name for the key.
    pub name: String,
    /// Chromium's `character_ignoring_all_modifiers` for the key on macOS, `None` where
    /// `MacKeyCodeForWindowsKeyCode` has no key code.
    pub mac_char: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Accelerator {
    pub key: Option<Key>,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub command: bool,
    pub altgr: bool,
    pub shifted_char: Option<char>,
}

/// macOS `NSEventModifierFlags` bits.
pub const MAC_SHIFT: u32 = 1 << 17;
pub const MAC_CONTROL: u32 = 1 << 18;
pub const MAC_OPTION: u32 = 1 << 19;
pub const MAC_COMMAND: u32 = 1 << 20;

enum Code {
    Shift,
    Control,
    Alt,
    Command,
    AltGr,
    Key(Key),
}

fn key(name: &str, mac_char: Option<u16>) -> Code {
    Code::Key(Key {
        name: name.to_owned(),
        mac_char,
    })
}

fn char_key(c: char) -> Code {
    key(&c.to_string(), Some(c as u16))
}

fn from_char(c: char) -> Option<(Code, Option<char>)> {
    let base = match c {
        'a'..='z'
        | '0'..='9'
        | ','
        | '-'
        | '.'
        | '/'
        | ';'
        | '='
        | '['
        | '\''
        | '\\'
        | ']'
        | '`' => return Some((char_key(c), None)),
        ' ' => return Some((key("space", Some(32)), None)),
        '\u{8}' => return Some((key("backspace", Some(8)), None)),
        '\t' => return Some((key("tab", Some(9)), None)),
        '\r' => return Some((key("enter", Some(13)), None)),
        '\u{1b}' => return Some((key("escape", Some(27)), None)),
        '\u{7f}' => return Some((key("delete", Some(0xF728)), None)),
        '!' => '1',
        '"' => '\'',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '&' => '7',
        '(' => '9',
        ')' => '0',
        '*' => '8',
        '+' => '=',
        ':' => ';',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '@' => '2',
        '^' => '6',
        '_' => '-',
        '{' => '[',
        '|' => '\\',
        '}' => ']',
        '~' => '`',
        _ => return None,
    };
    Some((char_key(base), Some(c)))
}

fn from_identifier(id: &str, platform: Platform) -> Option<(Code, Option<char>)> {
    let command_or_control = || {
        if platform == Platform::Mac {
            Code::Command
        } else {
            Code::Control
        }
    };
    let code = match id {
        "alt" | "option" => Code::Alt,
        "altgr" => Code::AltGr,
        "cmd" | "command" | "meta" | "super" => Code::Command,
        "cmdorctrl" | "commandorcontrol" => command_or_control(),
        "control" | "ctrl" => Code::Control,
        "shift" => Code::Shift,
        "backspace" => key("backspace", Some(8)),
        "capslock" => key("capslock", Some(0)),
        "delete" => key("delete", Some(0xF728)),
        "down" => key("down", Some(0xF701)),
        "end" => key("end", Some(0xF72B)),
        "enter" | "return" => key("enter", Some(13)),
        "esc" | "escape" => key("escape", Some(27)),
        "home" => key("home", Some(0xF729)),
        "insert" => key("insert", Some(0xF727)),
        "left" => key("left", Some(0xF702)),
        "pagedown" => key("pagedown", Some(0xF72D)),
        "pageup" => key("pageup", Some(0xF72C)),
        "plus" => return Some((char_key('='), Some('+'))),
        "printscreen" => key("printscreen", None),
        "right" => key("right", Some(0xF703)),
        "scrolllock" => key("scrolllock", None),
        "space" => key("space", Some(32)),
        "tab" => key("tab", Some(9)),
        "up" => key("up", Some(0xF700)),
        "numadd" => key("+", Some(u16::from(b'+'))),
        "numdec" => key(".", Some(u16::from(b'.'))),
        "numdiv" => key("/", Some(u16::from(b'/'))),
        "nummult" => key("*", Some(u16::from(b'*'))),
        "numsub" => key("-", Some(u16::from(b'-'))),
        "numlock" | "medianexttrack" | "mediaplaypause" | "mediaprevioustrack" | "mediastop"
        | "volumedown" | "volumemute" | "volumeup" => key(id, None),
        _ => {
            if let Some(&[digit @ b'0'..=b'9']) = id.strip_prefix("num").map(str::as_bytes) {
                return Some((
                    key(&char::from(digit).to_string(), Some(u16::from(digit))),
                    None,
                ));
            }
            let n = id.strip_prefix('f')?.parse::<u16>().ok()?;
            if !(1..=24).contains(&n) || format!("f{n}") != id {
                return None;
            }
            key(id, (n <= 20).then_some(0xF703 + n))
        }
    };
    Some((code, None))
}

impl Accelerator {
    /// `accelerator_util::StringToAccelerator`. `None` where Electron rejects the string.
    pub fn parse(shortcut: &str, platform: Platform) -> Option<Accelerator> {
        if !shortcut.is_ascii() {
            return None;
        }
        let mut accelerator = Accelerator::default();
        for token in shortcut.split('+').map(str::trim).filter(|t| !t.is_empty()) {
            let lower = token.to_ascii_lowercase();
            let mut chars = lower.chars();
            let parsed = match (chars.next(), chars.next()) {
                (Some(c), None) => from_char(c),
                _ => from_identifier(&lower, platform),
            };
            let (code, shifted) = parsed.unwrap_or((Code::Key(unknown()), None));
            accelerator.shifted_char = shifted;
            if shifted.is_some() {
                accelerator.shift = true;
            }
            match code {
                Code::Shift => accelerator.shift = true,
                Code::Control => accelerator.control = true,
                Code::Alt => accelerator.alt = true,
                Code::Command => accelerator.command = true,
                Code::AltGr => accelerator.altgr = true,
                Code::Key(key) => accelerator.key = Some(key),
            }
        }
        match &accelerator.key {
            Some(key) if !key.name.is_empty() => Some(accelerator),
            _ => None,
        }
    }

    /// The key equivalent and modifier mask `makeMenuItemForIndex:fromModel:` gives an item
    /// with this accelerator.
    pub fn mac_key_equivalent(&self) -> Option<(String, u32)> {
        let mut mask = 0;
        if self.control {
            mask |= MAC_CONTROL;
        }
        if self.alt {
            mask |= MAC_OPTION;
        }
        if self.command {
            mask |= MAC_COMMAND;
        }
        let character = match self.shifted_char {
            Some(c) => c as u16,
            None => {
                if self.shift {
                    mask |= MAC_SHIFT;
                }
                self.key.as_ref()?.mac_char?
            }
        };
        Some((String::from_utf16_lossy(&[character]), mask))
    }

    /// The same shortcut as a GPUI keystroke, such as `cmd-shift-o`.
    pub fn gpui_keystroke(&self) -> Option<String> {
        let key = self.key.as_ref()?;
        let mut out = String::new();
        for (on, name) in [
            (self.control, "ctrl-"),
            (self.alt || self.altgr, "alt-"),
            (self.shift, "shift-"),
            (self.command, "cmd-"),
        ] {
            if on {
                out.push_str(name);
            }
        }
        out.push_str(&key.name);
        Some(out)
    }
}

fn unknown() -> Key {
    Key {
        name: String::new(),
        mac_char: None,
    }
}
