//! A page of local files in the system's webview (WKWebView, WebView2, WebKitGTK), shown as a
//! native child view of a window.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use gpui_webview::WebView;
use wry::http::{Response, StatusCode, header};
use wry::{NewWindowResponse, Theme, WebContext, WebViewBuilder};

use crate::gpui::{AnyElement, App, AppContext, Entity, IntoElement, Task, Window};

const SCHEME: &str = "asrs-page";
const ORIGIN: &str = "asrs-page://localhost/";
/// WebView2 cannot host custom schemes; wry serves them from this origin there.
const WINDOWS_ORIGIN: &str = "http://asrs-page.localhost/";

/// Reports `location.hash` with the id of the last assignment the page has seen. The router of a
/// page that keeps its route in the hash rewrites it with `history.replaceState`, which fires no
/// event. Electron shows no context menu by default, so the engine's is suppressed.
const REPORTER: &str = r#"(() => {
  if (window.top !== window) return;
  let id = 0;
  let sent;
  const post = (kind) => {
    const message = kind + " " + id + " " + location.hash;
    if (kind === "hash" && message === sent) return;
    sent = "hash " + id + " " + location.hash;
    window.ipc.postMessage(message);
  };
  Object.defineProperty(window, "__asrsAssign", {
    value: (next, hash) => {
      id = next;
      location.hash = hash;
      post("hash");
    }
  });
  for (const name of ["pushState", "replaceState"]) {
    const original = history[name];
    history[name] = function (...args) {
      const result = original.apply(this, args);
      post("hash");
      return result;
    };
  }
  addEventListener("hashchange", () => post("hash"));
  addEventListener("popstate", () => post("hash"));
  addEventListener("load", () => post("load"));
  addEventListener("contextmenu", (event) => event.preventDefault());
})();"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageEvent {
    Load {
        hash: String,
    },
    Hash {
        id: u64,
        hash: String,
    },
    /// A link that opens a new window.
    Open(String),
}

pub struct PageOptions {
    pub root: PathBuf,
    pub index: &'static str,
    /// Origins frames inside the page may load, as URL prefixes.
    pub embeds: &'static [&'static str],
    /// Where WebView2 keeps its profile, which is beside the executable by default.
    pub data_dir: Option<PathBuf>,
    pub dark: bool,
}

pub struct Page {
    view: Entity<WebView>,
    _events: Task<()>,
}

impl Page {
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
        } = options;
        let (sender, receiver) = async_channel::unbounded();
        let reports = sender.clone();
        let mut context = data_dir
            .filter(|_| cfg!(target_os = "windows"))
            .map(|dir| WebContext::new(Some(dir)));
        let builder = match &mut context {
            Some(context) => WebViewBuilder::new_with_web_context(context),
            None => WebViewBuilder::new(),
        };
        let builder = builder
            .with_custom_protocol(SCHEME.to_owned(), move |_, request| {
                serve(&root, index, request.uri().path())
            })
            .with_initialization_script(REPORTER)
            .with_ipc_handler(move |request| {
                if in_page(&request.uri().to_string())
                    && let Some(event) = report(request.body())
                {
                    let _ = reports.try_send(event);
                }
            })
            .with_navigation_handler(move |url| {
                in_page(&url)
                    || url.starts_with("about:")
                    || embeds.iter().any(|prefix| url.starts_with(prefix))
            })
            .with_new_window_req_handler(move |url, _| {
                let _ = sender.try_send(PageEvent::Open(url));
                NewWindowResponse::Deny
            })
            .with_download_started_handler(|_, _| false)
            .with_devtools(false)
            .with_focused(false)
            .with_url(format!("{ORIGIN}{index}"));
        let view = WebView::build(builder, window, cx).map_err(|e| e.to_string())?;
        let view = cx.new(|_| view);
        let events = cx.spawn(async move |cx| {
            while let Ok(event) = receiver.recv().await {
                cx.update(|cx| on_event(event, cx));
            }
        });
        let page = Page {
            view,
            _events: events,
        };
        page.set_dark(dark, cx);
        Ok(page)
    }

    pub fn element(&self) -> AnyElement {
        self.view.clone().into_any_element()
    }

    pub fn set_visible(&self, visible: bool, cx: &mut App) {
        let view = self.view.read(cx);
        if view.visible() != visible
            && let Err(e) = view.set_visible(visible)
        {
            eprintln!("asrs: page visibility: {e}");
        }
    }

    pub fn set_dark(&self, dark: bool, cx: &mut App) {
        let theme = if dark { Theme::Dark } else { Theme::Light };
        if let Err(e) = self.view.read(cx).set_theme(theme) {
            eprintln!("asrs: page theme: {e}");
        }
    }

    /// `location.hash = hash`, after which the page reports `id`. `hash` is a serialized URL
    /// fragment.
    pub fn assign_hash(&self, id: u64, hash: &str, cx: &mut App) {
        let hash = hash.replace('\\', "\\\\").replace('\'', "\\'");
        self.evaluate(&format!("__asrsAssign({id}, '{hash}')"), cx);
    }

    pub fn evaluate(&self, script: &str, cx: &mut App) {
        if let Err(e) = self.view.read(cx).evaluate_script(script) {
            eprintln!("asrs: page script: {e}");
        }
    }

    /// Evaluates `script` and hands its result's JSON to `done`.
    pub fn query(&self, script: &str, done: impl Fn(String) + Send + 'static, cx: &mut App) {
        if let Err(e) = self
            .view
            .read(cx)
            .evaluate_script_with_callback(script, done)
        {
            eprintln!("asrs: page script: {e}");
        }
    }
}

fn in_page(url: &str) -> bool {
    url.starts_with(ORIGIN) || url.starts_with(WINDOWS_ORIGIN)
}

fn report(message: &str) -> Option<PageEvent> {
    let mut parts = message.splitn(3, ' ');
    let (kind, id, hash) = (parts.next()?, parts.next()?, parts.next()?);
    let (id, hash) = (id.parse().ok()?, hash.to_owned());
    match kind {
        "load" => Some(PageEvent::Load { hash }),
        "hash" => Some(PageEvent::Hash { id, hash }),
        _ => None,
    }
}

fn file(root: &Path, index: &str, path: &str) -> Option<PathBuf> {
    let path = path.trim_start_matches('/');
    let path = Path::new(if path.is_empty() { index } else { path });
    path.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then(|| root.join(path))
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn serve(root: &Path, index: &str, path: &str) -> Response<Cow<'static, [u8]>> {
    let found = file(root, index, path)
        .and_then(|file| std::fs::read(&file).ok().map(|bytes| (mime(&file), bytes)));
    let (status, kind, bytes) = match found {
        Some((kind, bytes)) => (StatusCode::OK, kind, bytes),
        None => (StatusCode::NOT_FOUND, "text/plain", Vec::new()),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, kind)
        .body(Cow::Owned(bytes))
        .expect("a static response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_files_below_the_root_are_served() {
        let root = Path::new("/docs");
        let at = |path| file(root, "index.html", path);
        assert_eq!(at("/"), Some(root.join("index.html")));
        assert_eq!(at("/assets/a.js"), Some(root.join("assets/a.js")));
        assert_eq!(at("/../secret"), None);
        assert_eq!(at("/assets/../../secret"), None);
        #[cfg(windows)]
        assert_eq!(at("/C:/secret"), None);
    }

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
        assert!(in_page("asrs-page://localhost/index.html#/"));
        assert!(!in_page("https://example.com/asrs-page://localhost/"));
    }
}
