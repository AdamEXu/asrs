//! The documentation tab's renderer element (`#renderer0` in `www/hub.html`): upstream's built
//! documentation in the system's webview, where upstream has an iframe. The native view draws
//! above everything GPUI draws in its bounds.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use asrs_hub::hub::HubModel;
use asrs_js::string::JsString;
use asrs_tabs::documentation::{DocumentationFrame, DocumentationRenderer};
use asrs_ui::gpui::{
    AnyElement, App, Context, CursorStyle, Global, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Styled, Window, div, px,
};
use asrs_ui::parity::probed;
use asrs_ui::text::{self, Text, TextStyle};
use asrs_ui::theme::{Appearance, FONT_FAMILY};
use asrs_ui::webview::{Page, PageEvent, PageOptions};

use super::frame::{FrameHost, HubFrame};

const INDEX: &str = "index.html";
const ONLINE: &str = "https://docs.advantagescope.org";
/// The video player on four pages of the documentation.
const EMBEDS: &[&str] = &["https://www.youtube.com/"];
const UNAVAILABLE: &str = "The documentation cannot be shown here. Click to open it in a browser.";

/// The folder of the built documentation and the app's user-data folder. Without it, as in
/// parity scenarios, no page is loaded.
pub struct DocumentationRoot {
    pub build: PathBuf,
    pub user_data: PathBuf,
}

impl Global for DocumentationRoot {}

type SharedFrame = Rc<RefCell<DocumentationFrame>>;

/// One tab's frame and the native page shown for it, if it could be opened.
pub struct DocumentationPage {
    /// Keeps the tab's key, which is this allocation's address, from being reused.
    frame: SharedFrame,
    page: Option<Page>,
    dark: bool,
}

#[derive(Default)]
pub struct DocumentationViews {
    tabs: HashMap<usize, DocumentationPage>,
}

fn key(frame: &SharedFrame) -> usize {
    Rc::as_ptr(frame) as *const () as usize
}

fn renderers(hub: &HubModel) -> impl Iterator<Item = (&DocumentationRenderer, bool)> {
    hub.tabs().tabs().filter_map(|tab| {
        let renderer = tab.renderer().downcast_ref::<DocumentationRenderer>()?;
        Some((renderer, !tab.content_hidden()))
    })
}

fn open_page(frame: &SharedFrame, dark: bool, window: &mut Window, cx: &mut App) -> Option<Page> {
    let root = cx.try_global::<DocumentationRoot>()?;
    let build = root.build.clone();
    let data_dir = Some(root.user_data.join("webview"));
    if !build.join(INDEX).is_file() {
        eprintln!("asrs: no documentation in {}", build.display());
        return None;
    }
    let frame = Rc::downgrade(frame);
    let options = PageOptions {
        root: build,
        index: INDEX,
        embeds: EMBEDS,
        data_dir,
        dark,
    };
    let opened = Page::open(options, window, cx, move |event, cx| {
        let Some(frame) = frame.upgrade() else {
            return;
        };
        match event {
            PageEvent::Load { hash } => frame.borrow_mut().load(JsString::from(hash)),
            PageEvent::Hash { id, hash } => {
                frame.borrow_mut().navigated(id, JsString::from(hash));
            }
            PageEvent::Open(url) => cx.open_url(&url),
        }
    });
    opened
        .map_err(|e| eprintln!("asrs: opening the documentation: {e}"))
        .ok()
}

/// The address of a documentation page on upstream's site.
fn online(hash: &JsString) -> String {
    let hash = hash.to_string_lossy();
    format!("{ONLINE}{}", hash.strip_prefix('#').unwrap_or("/"))
}

impl DocumentationPage {
    pub fn new(
        frame: &SharedFrame,
        dark: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> DocumentationPage {
        let page = open_page(frame, dark, window, cx);
        if page.is_none() {
            frame.borrow_mut().load(JsString::new());
        }
        DocumentationPage {
            frame: frame.clone(),
            page,
            dark,
        }
    }

    pub fn page(&self) -> Option<&Page> {
        self.page.as_ref()
    }

    /// Passes on what the renderer assigned since the last frame.
    pub fn sync(&mut self, dark: bool, cx: &mut App) {
        let assignment = self.frame.borrow_mut().take_assignment();
        let Some(page) = &self.page else {
            return;
        };
        if self.dark != dark {
            self.dark = dark;
            page.set_dark(dark, cx);
        }
        if let Some(assignment) = assignment {
            page.assign_hash(assignment.id, &assignment.hash, cx);
        }
    }

    pub fn element(
        &self,
        [.., width, height]: [f64; 4],
        appearance: Appearance,
        scale: f32,
        cx: &mut App,
    ) -> AnyElement {
        match &self.page {
            Some(page) => page.element(),
            None if cx.has_global::<DocumentationRoot>() => {
                let frame = self.frame.clone();
                let font = format!("14px {}", FONT_FAMILY.join(", "));
                let style = TextStyle::from_css([("font", font.as_str())]).expect("text style");
                let (w, h) = text::measure(cx, &style, UNAVAILABLE, scale);
                div()
                    .absolute()
                    .left(px(((width - w) / 2.0) as f32))
                    .top(px(((height - h) / 2.0) as f32))
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        let url = online(frame.borrow().hash());
                        cx.open_url(&url);
                    })
                    .child(Text::new(UNAVAILABLE, style).color(appearance.pick(0x222222, 0xeeeeee)))
                    .into_any_element()
            }
            None => div().into_any_element(),
        }
    }
}

impl<H: FrameHost> HubFrame<H> {
    pub(super) fn documentation_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let mut live = Vec::new();
        let mut shown = None;
        for (renderer, visible) in renderers(self.host().hub(cx)) {
            live.push(key(renderer.frame()));
            if visible && renderer.frame().borrow().src().is_some() {
                shown = Some(renderer.frame().clone());
            }
        }
        let shown_key = shown.as_ref().map(key);
        let tabs = &mut self.documentation.tabs;
        tabs.retain(|key, _| live.contains(key));
        let dark = appearance == Appearance::Dark;
        for (key, tab) in tabs.iter_mut() {
            tab.sync(dark, cx);
            if let Some(page) = &tab.page {
                page.set_visible(Some(*key) == shown_key, cx);
            }
        }
        let frame = shown?;
        let tab = tabs
            .entry(key(&frame))
            .or_insert_with(|| DocumentationPage::new(&frame, dark, window, cx));
        let content = tab.element(rect, appearance, scale, cx);
        let container = div()
            .absolute()
            .size_full()
            .overflow_hidden()
            .child(content);
        Some(probed("documentation-container", container).into_any_element())
    }
}
