use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gpui_webview::WebView;
use wry::{Theme, WebContext, WebViewBuilder};

use super::PageOptions;
use super::page;
use super::wire::PageEvent;
use crate::gpui::{AnyElement, App, AppContext, Entity, IntoElement, Task, Window, div};
use crate::platform::parent::Parent;

pub struct Page {
    state: Rc<State>,
    _opening: Task<()>,
    _events: Task<()>,
}

struct State {
    view: RefCell<Option<Entity<WebView>>>,
    visible: Cell<bool>,
    dark: Cell<bool>,
}

impl Page {
    /// The webview is built in a task of its own. WebView2 builds it inside a nested message
    /// loop, which runs other tasks; those panic if `App` is borrowed, as it is here.
    pub fn open(
        options: PageOptions,
        window: &mut Window,
        cx: &mut App,
        on_event: impl Fn(PageEvent, &mut App) + 'static,
    ) -> Result<Page, String> {
        let PageOptions {
            root,
            index,
            embeds,
            data_dir,
            dark,
            ..
        } = options;
        let window = window.window_handle();
        let (sender, receiver) = async_channel::unbounded();
        let state = Rc::new(State {
            view: RefCell::new(None),
            visible: Cell::new(true),
            dark: Cell::new(dark),
        });
        let opened = state.clone();
        let opening = cx.spawn(async move |cx| {
            let Ok(parent) = window.update(cx, |_, window, _| Parent::of(window)) else {
                return;
            };
            let reports = sender.clone();
            let built = parent.map_err(|e| e.to_string()).and_then(|parent| {
                build(root, index, embeds, data_dir, &parent, move |event| {
                    let _ = reports.try_send(event);
                })
                .map_err(|e| e.to_string())
            });
            let webview = match built {
                Ok(webview) => webview,
                Err(e) => {
                    let _ = sender.try_send(PageEvent::Failed(e));
                    return;
                }
            };
            let _ = window.update(cx, |_, window, cx| {
                let view = WebView::new(webview, window, cx);
                let view = cx.new(|_| view);
                *opened.view.borrow_mut() = Some(view);
                opened.apply_visible(cx);
                opened.apply_dark(cx);
                window.refresh();
            });
        });
        let events = cx.spawn(async move |cx| {
            while let Ok(event) = receiver.recv().await {
                cx.update(|cx| on_event(event, cx));
            }
        });
        Ok(Page {
            state,
            _opening: opening,
            _events: events,
        })
    }

    pub fn element(&self) -> AnyElement {
        match &*self.state.view.borrow() {
            Some(view) => view.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    pub fn set_visible(&self, visible: bool, cx: &mut App) {
        if self.state.visible.replace(visible) != visible {
            self.state.apply_visible(cx);
        }
    }

    pub fn set_dark(&self, dark: bool, cx: &mut App) {
        if self.state.dark.replace(dark) != dark {
            self.state.apply_dark(cx);
        }
    }

    /// `location.hash = hash`, after which the page reports `id`. `hash` is a serialized URL
    /// fragment.
    pub fn assign_hash(&self, id: u64, hash: &str, cx: &mut App) {
        self.evaluate(&super::assignment(id, hash), cx);
    }

    pub fn evaluate(&self, script: &str, cx: &mut App) {
        self.state.with(|view| {
            if let Err(e) = view.read(cx).evaluate_script(script) {
                eprintln!("asrs: page script: {e}");
            }
        });
    }

    /// Evaluates `script` and hands its result's JSON to `done`.
    pub fn query(&self, script: &str, done: impl Fn(String) + Send + 'static, cx: &mut App) {
        self.state.with(|view| {
            if let Err(e) = view.read(cx).evaluate_script_with_callback(script, done) {
                eprintln!("asrs: page script: {e}");
            }
        });
    }
}

impl State {
    fn with(&self, f: impl FnOnce(&Entity<WebView>)) {
        if let Some(view) = &*self.view.borrow() {
            f(view);
        }
    }

    fn apply_visible(&self, cx: &mut App) {
        self.with(|view| {
            if let Err(e) = view.read(cx).set_visible(self.visible.get()) {
                eprintln!("asrs: page visibility: {e}");
            }
        });
    }

    fn apply_dark(&self, cx: &mut App) {
        let theme = if self.dark.get() {
            Theme::Dark
        } else {
            Theme::Light
        };
        self.with(|view| {
            if let Err(e) = view.read(cx).set_theme(theme) {
                eprintln!("asrs: page theme: {e}");
            }
        });
    }
}

fn build(
    root: PathBuf,
    index: &str,
    embeds: &[&str],
    data_dir: Option<PathBuf>,
    parent: &Parent,
    send: impl Fn(PageEvent) + Clone + Send + Sync + 'static,
) -> wry::Result<wry::WebView> {
    let mut context = data_dir
        .filter(|_| cfg!(target_os = "windows"))
        .map(|dir| WebContext::new(Some(dir)));
    let builder = match &mut context {
        Some(context) => WebViewBuilder::new_with_web_context(context),
        None => WebViewBuilder::new(),
    };
    let embeds = embeds.iter().map(|&e| e.to_owned()).collect();
    page::builder(builder, root, index.to_owned(), embeds, send).build_as_child(parent)
}
