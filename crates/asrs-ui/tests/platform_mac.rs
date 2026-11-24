use asrs_ui::platform::probe::{
    native_menu_roundtrip, numpad_enter_for, read_wheel_sample, select_menu_roundtrip,
    tooltip_roundtrip, wheel_gesture_samples,
};
use asrs_ui::platform::{
    PopupEntry, PopupItem, PopupMenu, SelectMenu, SelectMenuItem, SelectMenuItemKind, WheelDelta,
    native_menu_label,
};

fn main() {
    if asrs_parity::ci::skip_in_ci("platform mac", "a display") {
        return;
    }
    if cfg!(target_os = "macos") {
        labels();
        menu_tree();
        select_menu();
        wheel_gesture();
        tooltip();
        assert_eq!(
            [0x24, 0x4C].map(numpad_enter_for),
            [Some(false), Some(true)]
        );
        println!(
            "platform mac: labels, menu tree, select menu, wheel gesture, tooltip and numpad Enter ok"
        );
    } else {
        println!("platform mac: skipped on this platform");
    }
}

fn tooltip() {
    let shown = tooltip_roundtrip(&["Play", "a: 1\nb: 2", "", "Pause"]);
    let expected = [Some("Play"), Some("a: 1\nb: 2"), None, Some("Pause")];
    let texts: Vec<_> = shown.iter().map(|(text, _)| text.as_deref()).collect();
    assert_eq!(texts, expected);
    let owned: Vec<_> = shown.iter().map(|(_, owned)| *owned).collect();
    assert_eq!(owned, [true, true, false, true]);
}

fn labels() {
    for (label, shown) in [
        (
            "Copy \"/Drive/Pose\" to clipboard",
            "Copy \"/Drive/Pose\" to clipboard",
        ),
        ("Save & Close", "Save  Close"),
        ("Save && Close", "Save & Close"),
        ("&File", "File"),
        ("Open(&O)", "Open"),
        ("Open(&O", "Open(O"),
        ("Export...", "Export\u{2026}"),
        ("a....b", "a\u{2026}.b"),
        ("..", ".."),
        ("trailing &", "trailing "),
        ("Joystick 0 \u{1f3ae}", "Joystick 0 \u{1f3ae}"),
    ] {
        assert_eq!(native_menu_label(label), shown, "label {label:?}");
    }
}

fn menu_tree() {
    let colors = PopupMenu::new()
        .entry(
            PopupEntry::new("Blue")
                .checked(true)
                .swatch([0x2b, 0x66, 0xa2]),
        )
        .entry(PopupEntry::new("Red").swatch([0xe4, 0x1a, 0x1c]))
        .entry(PopupEntry::new("Gamut corner").swatch([0, 255, 0]));
    let speeds = [25, 50, 100, 150, 200, 400, 800]
        .into_iter()
        .fold(PopupMenu::new(), |menu, speed| {
            menu.entry(PopupEntry::new(format!("{speed}%")).checked(speed == 100))
        });
    let model = PopupMenu::new()
        .entry(PopupEntry::new("Copy \"Pose\" to clipboard"))
        .entry(PopupEntry::new("Copy Tuned Values").enabled(false))
        .separator()
        .entry(PopupEntry::new("Color").submenu(colors.clone()))
        .entry(PopupEntry::new("Speed").submenu(speeds.clone()))
        .entry(PopupEntry::new("Hidden").visible(false))
        .entry(
            PopupEntry::new("Nested").submenu(
                PopupMenu::new()
                    .entry(
                        PopupEntry::new("Deeper").submenu(
                            PopupMenu::new()
                                .entry(PopupEntry::new("Leaf...").checked(true).enabled(false)),
                        ),
                    )
                    .separator()
                    .entry(PopupEntry::new("Sibling")),
            ),
        )
        .entry(PopupEntry::new("Nothing").submenu(PopupMenu::new()))
        .entry(
            PopupEntry::new("All hidden")
                .submenu(PopupMenu::new().entry(PopupEntry::new("x").visible(false))),
        )
        .separator()
        .entry(PopupEntry::new("Loop Visible Range").checked(true));

    let empty = PopupMenu::new().entry(PopupEntry::new("(empty)").enabled(false));
    let mut expected = model.clone();
    for item in &mut expected.items {
        if let PopupItem::Entry(entry) = item
            && let Some(sub) = &mut entry.submenu
            && !sub.has_visible_items()
        {
            *sub = empty.clone();
        }
    }
    fix_labels(&mut expected);

    let read = native_menu_roundtrip(&model).expect("native menu read back");
    assert_eq!(read, expected);
}

fn fix_labels(menu: &mut PopupMenu) {
    for item in &mut menu.items {
        if let PopupItem::Entry(entry) = item {
            entry.label = native_menu_label(&entry.label);
            if let Some(sub) = &mut entry.submenu {
                fix_labels(sub);
            }
        }
    }
}

fn wheel_gesture() {
    for sample in wheel_gesture_samples() {
        let reading = read_wheel_sample(&sample).expect("wheel reading");
        let expected = if sample.continuous {
            WheelDelta {
                x: sample.point_delta[0] as f32,
                y: sample.point_delta[1] as f32,
            }
        } else {
            WheelDelta {
                x: sample.fixed_delta[0] as f32 * 40.0,
                y: sample.fixed_delta[1] as f32 * 40.0,
            }
        };
        assert_eq!(reading.chromium, expected, "{sample:?}: {reading:?}");
    }
}

/// `WebMenuRunner`'s menu for Field2d's field select: optgroup rows disabled, option rows
/// indented in the drawn title and trimmed in the plain one, the menu font at the select's
/// size, each item tagged with its index, and the cell on the selected item.
fn select_menu() {
    let row = |label: &str, kind, enabled| SelectMenuItem {
        label: label.to_owned(),
        kind,
        enabled,
        tool_tip: String::new(),
    };
    let menu = SelectMenu {
        items: vec![
            row("FRC Fields", SelectMenuItemKind::Group, true),
            row("    2025 Field", SelectMenuItemKind::Option, true),
            row("    2024 Field", SelectMenuItemKind::Option, false),
            row("", SelectMenuItemKind::Separator, true),
            row("FTC Fields", SelectMenuItemKind::Group, true),
            row("    Into the Deep", SelectMenuItemKind::Option, true),
        ],
        font_size: 13.0,
        selected: Some(5),
        right_aligned: false,
    };
    let read = select_menu_roundtrip(&menu).expect("select menu");
    assert!(!read.pulls_down);
    assert_eq!(read.selected_tag, Some(5));
    type Row<'a> = (bool, &'a str, Option<&'a str>, Option<f64>, bool, isize);
    let rows: Vec<Row<'_>> = read
        .items
        .iter()
        .map(|i| {
            (
                i.separator,
                i.title.as_str(),
                i.attributed_title.as_deref(),
                i.font_size,
                i.enabled,
                i.tag,
            )
        })
        .collect();
    let font = Some(13.0);
    assert_eq!(
        rows,
        vec![
            (false, "FRC Fields", Some("FRC Fields"), font, false, 0),
            (false, "2025 Field", Some("    2025 Field"), font, true, 1),
            (false, "2024 Field", Some("    2024 Field"), font, false, 2),
            (true, "", None, None, false, 0),
            (false, "FTC Fields", Some("FTC Fields"), font, false, 4),
            (
                false,
                "Into the Deep",
                Some("    Into the Deep"),
                font,
                true,
                5
            ),
        ]
    );
}
