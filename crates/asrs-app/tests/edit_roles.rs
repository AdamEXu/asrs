#![allow(clippy::disallowed_methods)]

//! The edit roles of the application menu on a window with a text field, as Blink's editor
//! commands act on an `<input>`: `ExecuteCut`, `ExecuteDelete` and the rest with the source
//! `kMenuOrKeyBinding` leave the field and the clipboard alone without a selected range.

use std::sync::Arc;

use asrs_app::menus::bridge::MenuAction;
use asrs_app::menus::dispatch::dispatch;
use asrs_app::windows::{self, Rect, Windows};
use asrs_ui::gpui::{
    AnyWindowHandle, AppContext, ClipboardItem, Context, Entity, FocusHandle, HeadlessAppContext,
    InteractiveElement, IntoElement, NoopTextSystem, ParentElement, Render, Styled, Window, div,
};
use asrs_ui::input::{FieldKind, FieldStyle, PlainText, TextField};

struct Page {
    field: Entity<TextField<PlainText>>,
    body: FocusHandle,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .id("page")
            .track_focus(&self.body)
            .size_full()
            .child(self.field.clone())
    }
}

struct Fixture {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    page: Entity<Page>,
}

impl Fixture {
    fn new() -> Fixture {
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let (window, page) = cx.update(|cx| {
            asrs_ui::init(cx);
            windows::init(cx);
            asrs_ui::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| Page {
                    field: cx.new(|cx| {
                        let model = PlainText::new("one two");
                        TextField::new(model, FieldKind::Text, FieldStyle::NATIVE, window, cx)
                    }),
                    body: cx.focus_handle(),
                })
            })
            .expect("a window")
        });
        cx.update(|cx| {
            let bounds = Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            };
            Windows::adopt_hub(cx, window, bounds);
        });
        let mut fixture = Fixture { cx, window, page };
        fixture.focus_field();
        fixture
    }

    fn focus_field(&mut self) {
        let page = self.page.clone();
        self.update(move |window, cx| {
            let field = page.read(cx).field.clone();
            field.update(cx, |field, cx| field.focus(window, cx));
        });
    }

    fn update(&mut self, f: impl FnOnce(&mut Window, &mut asrs_ui::gpui::App)) {
        self.cx
            .update_window(self.window, |_, window, cx| f(window, cx))
            .expect("open");
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .expect("open");
        self.cx.run_until_parked();
    }

    fn role(&mut self, role: &str) -> String {
        let action = MenuAction::Role(role.to_owned());
        self.cx.update(|cx| dispatch(action, cx));
        self.cx.run_until_parked();
        self.update(|_, _| {});
        let page = self.page.clone();
        self.cx.update(|cx| page.read(cx).field.read(cx).value())
    }

    fn set_clipboard(&mut self, text: &str) {
        let item = ClipboardItem::new_string(text.to_owned());
        self.cx.update(|cx| cx.write_to_clipboard(item));
    }

    fn clipboard(&mut self) -> Option<String> {
        self.cx
            .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
    }
}

#[test]
fn a_focused_field_takes_the_edit_roles() {
    let mut f = Fixture::new();
    f.set_clipboard("kept");
    for role in ["copy", "cut", "delete"] {
        assert_eq!(f.role(role), "one two", "{role} of a caret");
        assert_eq!(f.clipboard().as_deref(), Some("kept"), "{role} of a caret");
    }

    f.role("selectall");
    assert_eq!(f.role("copy"), "one two");
    assert_eq!(f.clipboard().as_deref(), Some("one two"));
    assert_eq!(f.role("delete"), "");
    assert_eq!(f.role("undo"), "one two");
    assert_eq!(f.role("redo"), "");
    assert_eq!(f.role("undo"), "one two");

    f.set_clipboard("kept");
    f.role("selectall");
    assert_eq!(f.role("cut"), "");
    assert_eq!(f.clipboard().as_deref(), Some("one two"));
    assert_eq!(f.role("paste"), "one two");
    f.set_clipboard("three");
    f.role("selectall");
    assert_eq!(f.role("pasteandmatchstyle"), "three");
}

#[test]
fn without_a_focused_field_the_edit_roles_change_nothing() {
    let mut f = Fixture::new();
    let page = f.page.clone();
    f.update(move |window, cx| {
        let body = page.read(cx).body.clone();
        window.focus(&body, cx);
    });
    f.set_clipboard("kept");
    for role in [
        "selectall",
        "copy",
        "cut",
        "delete",
        "undo",
        "redo",
        "paste",
        "pasteandmatchstyle",
    ] {
        assert_eq!(f.role(role), "one two", "{role}");
        assert_eq!(f.clipboard().as_deref(), Some("kept"), "{role}");
    }
}

#[test]
fn a_disabled_field_is_not_edited() {
    let mut f = Fixture::new();
    f.role("selectall");
    let page = f.page.clone();
    f.update(move |_, cx| {
        let field = page.read(cx).field.clone();
        field.update(cx, |field, cx| field.set_disabled(true, cx));
    });
    f.set_clipboard("three");
    for role in ["cut", "paste", "pasteandmatchstyle", "delete", "undo"] {
        assert_eq!(f.role(role), "one two", "{role}");
    }
}
