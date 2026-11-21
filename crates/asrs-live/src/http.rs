//! A plain HTTP/1.1 GET over a real socket, for the `fetch` half of the live transports.
//! `get_head` resolves once the headers are in, the way `NT4.ts`'s alive check uses fetch;
//! `get_body` reads the response to EOF, the way `response.json()` does. Neither follows
//! redirects.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::http::Uri;

const MAX_HEAD: usize = 65536;

/// What the URL parser does to a URL before the request goes out: bytes a URI cannot carry
/// are percent-encoded.
pub(crate) fn percent_encode(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for b in url.bytes() {
        if b.is_ascii_graphic()
            && !matches!(
                b,
                b'"' | b'<' | b'>' | b'`' | b'{' | b'}' | b'|' | b'\\' | b'^'
            )
        {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub(crate) fn host_port(uri: &Uri, default_port: u16) -> Option<(String, u16)> {
    let host = uri.host()?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    Some((host.to_owned(), uri.port_u16().unwrap_or(default_port)))
}

struct HttpResponse {
    status: u16,
    authority: String,
    port: u16,
    path: String,
    head_body: Vec<u8>,
    stream: TcpStream,
}

/// Sends the GET and reads through the response headers; any already-buffered body bytes
/// stay in `head_body`.
async fn get_response(url: &str) -> Option<HttpResponse> {
    let uri = Uri::try_from(percent_encode(url)).ok()?;
    if uri.scheme_str() != Some("http") {
        return None;
    }
    let (host, port) = host_port(&uri, 80)?;
    let authority = uri.authority()?.as_str().to_owned();
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    let path = if path.is_empty() { "/" } else { path };
    let mut stream = TcpStream::connect((host.as_str(), port)).await.ok()?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let status = loop {
        let head_end = loop {
            if let Some(i) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            if buffer.len() > MAX_HEAD {
                return None;
            }
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buffer.extend_from_slice(&chunk[..n]);
        };
        let status = status_code(&buffer[..head_end])?;
        buffer.drain(..head_end);
        if !(100..200).contains(&status) {
            break status;
        }
    };
    Some(HttpResponse {
        status,
        authority,
        port,
        path: path.to_owned(),
        head_body: buffer,
        stream,
    })
}

/// `fetch(url)` reduced to the status and `response.url`; `None` is a network error.
pub(crate) async fn get_head(url: &str) -> Option<(u16, String)> {
    let HttpResponse {
        status,
        authority,
        port,
        path,
        ..
    } = get_response(url).await?;
    let authority = if port == 80 {
        authority.trim_end_matches(":80").to_owned()
    } else {
        authority
    };
    Some((status, format!("http://{authority}{path}")))
}

/// `fetch(url)` reduced to the status and the body `response.text()` yields; `None` is a
/// network error.
pub(crate) async fn get_body(url: &str) -> Option<(u16, String)> {
    let HttpResponse {
        status,
        head_body,
        mut stream,
        ..
    } = get_response(url).await?;
    let mut body = head_body;
    stream.read_to_end(&mut body).await.ok()?;
    Some((status, String::from_utf8_lossy(&body).into_owned()))
}

fn status_code(head: &[u8]) -> Option<u16> {
    let line_end = head.windows(2).position(|w| w == b"\r\n")?;
    let line = std::str::from_utf8(&head[..line_end]).ok()?;
    let mut parts = line.split(' ');
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let code = parts.next()?;
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(code.bytes().fold(0u16, |n, b| n * 10 + u16::from(b - b'0')))
}
