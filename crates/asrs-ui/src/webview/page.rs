//! The web page itself, however it is hosted.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use wry::http::{Response, StatusCode, header};
use wry::{NewWindowResponse, WebViewBuilder};

use super::wire::{self, PageEvent};

const SCHEME: &str = "asrs-page";
const ORIGIN: &str = "asrs-page://localhost/";
/// WebView2 cannot host custom schemes, so wry serves them from an http origin there.
const PAGE_ORIGIN: &str = if cfg!(windows) {
    "http://asrs-page.localhost/"
} else {
    ORIGIN
};

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

/// `builder` for the page whose files are below `root`. `embeds` are the URL prefixes frames
/// inside the page may load; WebView2 only reports navigations of the page itself, so there the
/// policy `serve` sends is what limits frames.
pub fn builder<'a>(
    builder: WebViewBuilder<'a>,
    root: PathBuf,
    index: String,
    embeds: Vec<String>,
    send: impl Fn(PageEvent) + Clone + Send + Sync + 'static,
) -> WebViewBuilder<'a> {
    let url = format!("{ORIGIN}{index}");
    let policy = policy(&embeds);
    let reports = send.clone();
    builder
        .with_custom_protocol(SCHEME.to_owned(), move |_, request| {
            serve(&root, &index, &policy, request.uri().path())
        })
        .with_initialization_script(REPORTER)
        .with_ipc_handler(move |request| {
            if in_page(&request.uri().to_string())
                && let Some(event) = wire::report(request.body())
            {
                reports(event);
            }
        })
        .with_navigation_handler(move |url| may_load(&url, &embeds))
        .with_new_window_req_handler(move |url, _| {
            if is_web(&url) {
                send(PageEvent::Open(url));
            }
            NewWindowResponse::Deny
        })
        .with_download_started_handler(|_, _| false)
        .with_devtools(false)
        .with_focused(false)
        .with_url(url)
}

fn policy(embeds: &[String]) -> String {
    let frames = embeds.join(" ");
    format!(
        "default-src 'self' 'unsafe-inline' data:; frame-src {frames}; object-src 'none'; \
         form-action 'none'"
    )
}

fn in_page(url: &str) -> bool {
    url.starts_with(PAGE_ORIGIN)
}

fn may_load(url: &str, embeds: &[String]) -> bool {
    in_page(url)
        || url.starts_with("about:")
        || (cfg!(not(windows)) && embeds.iter().any(|prefix| url.starts_with(prefix)))
}

fn is_web(url: &str) -> bool {
    ["https://", "http://"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
        && !in_page(url)
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

fn serve(root: &Path, index: &str, policy: &str, path: &str) -> Response<Cow<'static, [u8]>> {
    let found = file(root, index, path)
        .and_then(|file| std::fs::read(&file).ok().map(|bytes| (mime(&file), bytes)));
    let (status, kind, bytes) = match found {
        Some((kind, bytes)) => (StatusCode::OK, kind, bytes),
        None => (StatusCode::NOT_FOUND, "text/plain", Vec::new()),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, kind)
        .header(header::CONTENT_SECURITY_POLICY, policy)
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
    fn only_the_page_and_its_embeds_load() {
        let embeds = ["https://www.youtube.com/embed/".to_owned()];
        let page = format!("{PAGE_ORIGIN}index.html#/");
        assert!(in_page(&page));
        assert!(may_load(&page, &embeds));
        assert_eq!(
            may_load("https://www.youtube.com/embed/a", &embeds),
            cfg!(not(windows))
        );
        for url in [
            "https://www.youtube.com/watch?v=a",
            "https://example.com/",
            "https://example.com/asrs-page://localhost/",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(!in_page(url), "{url}");
            assert!(!may_load(url, &embeds), "{url}");
        }
        assert_eq!(
            in_page("http://asrs-page.localhost/index.html"),
            cfg!(windows)
        );
    }

    #[test]
    fn only_web_links_open_in_the_browser() {
        assert!(is_web("https://github.com/"));
        assert!(is_web("http://localhost:6768/ascope"));
        for url in [
            "file:///etc/passwd",
            "vscode://a",
            "asrs-page://localhost/img/a.png",
        ] {
            assert!(!is_web(url), "{url}");
        }
    }
}
