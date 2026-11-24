#![allow(clippy::disallowed_methods)]

//! The hub's titles in the playback states no scenario stops in, against upstream's `hub.html`,
//! and what `TooltipController` and `TooltipAura` make of them where Chromium has those: the
//! delay, the place, and what hides the tooltip.

#[path = "support/hub_page.rs"]
mod hub_page;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use asrs_model::selection::SelectionMode;
use asrs_parity::vectors;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    Bounds, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, point, px,
};
use asrs_ui::tooltip::{self, ForcedHost, Host, Shown};
use hub_page::Page;

const SHOW_DELAY: Duration = Duration::from_millis(500);
const MS: Duration = Duration::from_millis(1);

/// The `title` of `button.tab-control.<class>` in `hub.html`.
fn upstream(class: &str) -> String {
    let file = vectors::workspace_root().join("reference/AdvantageScope/www/hub.html");
    let html = std::fs::read_to_string(file).expect("hub.html");
    let (_, rest) = html
        .split_once(&format!("class=\"tab-control {class}\" title=\""))
        .expect("the button");
    rest.split_once('"').expect("its title").0.to_owned()
}

impl Page<'_> {
    fn titles(&mut self) -> Vec<(String, Bounds<Pixels>)> {
        let window = self.window.into();
        self.headless
            .context()
            .update_window(window, |_, window, cx| tooltip::titles(window, cx))
            .expect("window")
    }

    fn bounds(&mut self, title: &str) -> Bounds<Pixels> {
        let titles = self.titles();
        let mut found = titles.iter().filter(|(t, _)| t == title);
        found.next().unwrap_or_else(|| panic!("no {title}")).1
    }

    fn shows(&mut self, classes: &[&str], hidden: &[&str]) {
        self.frames(3);
        let titles: Vec<String> = self.titles().into_iter().map(|(title, _)| title).collect();
        for class in classes {
            assert!(
                titles.contains(&upstream(class)),
                "no {class} in {titles:?}"
            );
        }
        for class in hidden {
            assert!(!titles.contains(&upstream(class)), "{class} in {titles:?}");
        }
    }

    fn hover(&mut self, position: Point<Pixels>) {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
        self.headless.capture(self.window.into()).expect("draw");
    }

    fn shown(&mut self) -> Option<Shown> {
        let window = self.window.into();
        self.headless
            .context()
            .update_window(window, |_, window, cx| tooltip::shown(window, cx))
            .expect("window")
    }

    fn pass(&mut self, time: Duration) {
        let cx = self.headless.context();
        cx.advance_clock(time);
        cx.run_until_parked();
    }

    /// Hovers the middle of the element titled `title` until its tooltip shows.
    fn show(&mut self, title: &str) -> Point<Pixels> {
        let middle = self.bounds(title).center();
        self.hover(point(px(700.0), px(630.0)));
        self.hover(middle);
        self.pass(SHOW_DELAY);
        assert_eq!(self.shown().map(|s| s.text).as_deref(), Some(title));
        middle
    }
}

fn states(headless: &mut HeadlessCapture) {
    let mut page = Page::open(headless, 1.0);
    page.shows(
        &["play", "close", "popup", "add-tab"],
        &["pause", "lock", "unlock"],
    );
    page.selection(|selection| selection.play());
    page.shows(&["pause"], &["play", "lock", "unlock"]);
    page.selection(|selection| selection.pause());
    page.selection(|selection| selection.set_live_connected(Rc::new(|| 1.0)));
    assert_eq!(page.mode(), SelectionMode::Locked);
    page.shows(&["unlock"], &["lock"]);
    page.selection(|selection| selection.unlock());
    page.shows(&["lock"], &["unlock"]);
    page.close();
}

/// A title that changes under a pointer at rest, as the buttons of a toggle do.
fn toggles(headless: &mut HeadlessCapture) {
    let mut page = Page::open(headless, 1.0);
    let (play, pause) = (upstream("play"), upstream("pause"));
    let middle = page.bounds(&play).center();
    page.hover(middle);
    let given = |page: &mut Page<'_>| {
        let window = page.window.into();
        page.headless
            .context()
            .update_window(window, |_, window, cx| tooltip::take_given(window, cx))
            .expect("window")
    };
    assert_eq!(given(&mut page), [play.clone()]);
    page.selection(|selection| selection.play());
    page.frames(2);
    assert_eq!(given(&mut page), [pause]);
    page.selection(|selection| selection.pause());
    page.frames(2);
    assert_eq!(given(&mut page), [play]);
    page.close();
}

fn aura(headless: &mut HeadlessCapture) {
    headless
        .context()
        .update(|cx| cx.set_global(ForcedHost(Host::Aura)));
    let mut page = Page::open(headless, 1.0);
    let (play, close) = (upstream("play"), upstream("close"));
    let middle = page.bounds(&play).center();

    page.hover(middle);
    page.pass(SHOW_DELAY - MS);
    assert_eq!(page.shown(), None, "before the delay");
    let moved = middle + point(px(2.0), px(1.0));
    page.hover(moved);
    page.pass(MS);
    assert_eq!(page.shown(), None, "a move starts the delay again");
    page.pass(SHOW_DELAY - MS);
    let shown = Shown {
        text: play.clone(),
        pointer: moved,
    };
    assert_eq!(page.shown(), Some(shown.clone()), "after the delay");
    page.hover(middle);
    page.pass(Duration::from_secs(60));
    assert_eq!(page.shown(), Some(shown), "it stays, where it was shown");

    let other = page.bounds(&close).center();
    page.hover(other);
    assert_eq!(page.shown(), None, "another element's tooltip waits too");
    page.pass(SHOW_DELAY);
    assert_eq!(page.shown().map(|s| s.text), Some(close.clone()));

    let press = |position| {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Right,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        })
    };
    let at = page.show(&play);
    page.dispatch(press(at));
    assert_eq!(page.shown(), None, "a press hides it");
    page.dispatch(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 1,
    }));
    page.hover(at + point(px(1.0), px(0.0)));
    page.pass(SHOW_DELAY * 2);
    assert_eq!(page.shown(), None, "until the pointer is on another title");
    page.show(&play);

    page.wheel(ScrollDelta::Pixels(point(px(0.0), px(1.0))));
    assert_eq!(page.shown(), None, "the wheel hides it");

    page.show(&close);
    let window = page.window.into();
    page.headless
        .context()
        .update_window(window, |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse("f7").expect("a key"), cx);
        })
        .expect("window");
    page.headless.context().run_until_parked();
    assert_eq!(page.shown(), None, "a key hides it");

    page.show(&play);
    page.dispatch(PlatformInput::MouseExited(MouseExitEvent::default()));
    assert_eq!(page.shown(), None, "leaving the window hides it");
    page.close();
}

fn main() {
    if asrs_parity::ci::skip_in_ci("tooltip hosts", "a GPU") {
        return;
    }
    if cfg!(target_os = "windows") {
        println!("tooltip hosts: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless");
    headless
        .context()
        .update(|cx| cx.set_global(ForcedHost(Host::Native)));
    states(&mut headless);
    toggles(&mut headless);
    aura(&mut headless);
    println!("tooltip hosts: playback states, toggles and TooltipAura ok");
}
