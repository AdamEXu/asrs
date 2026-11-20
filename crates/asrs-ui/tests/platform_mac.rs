use asrs_ui::platform::probe::{native_menu_roundtrip, read_wheel_sample, wheel_gesture_samples};
use asrs_ui::platform::{PopupEntry, PopupItem, PopupMenu, WheelDelta, native_menu_label};

fn main() {
    if cfg!(target_os = "macos") {
        labels();
        menu_tree();
        wheel_gesture();
        println!("platform mac: labels, menu tree and wheel gesture ok");
    } else {
        println!("platform mac: skipped on this platform");
    }
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
