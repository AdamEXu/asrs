//! `fetch` for the live sources: the [`FetchTransport`] a source calls, the [`FetchEvent`] a
//! request settles with, and plain HTTP/1.1 GETs over real sockets for the tokio transports.
//! Requests never follow redirects.

use std::collections::BTreeMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

use crate::channel::EventSender;
use crate::live_url::LiveUrl;
use crate::net_options::NetOptions;

const MAX_HEAD: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// `response.url`.
    pub url: String,
    /// What `response.text()` or `response.json()` reads. Transports that settle at the
    /// headers, as NT4's alive check needs, leave it empty.
    pub body: String,
}

/// A settled `fetch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchEvent {
    Response(RequestId, Response),
    /// Rejected with a network error.
    Failed(RequestId),
}

impl FetchEvent {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Response(id, _) | Self::Failed(id) => *id,
        }
    }
}

pub trait FetchTransport {
    /// `fetch(url)` (a GET). It settles later as a [`FetchEvent`].
    fn fetch(&mut self, url: &str) -> RequestId;

    /// The request's `AbortSignal` fired: if it has not settled, it never will.
    fn fetch_abort(&mut self, id: RequestId);
}

/// The in-flight GET tasks of a tokio transport, which owns the id space and the event queue.
pub(crate) struct FetchTasks {
    options: NetOptions,
    read_body: bool,
    /// `None` for a request that settled without a connection.
    tasks: BTreeMap<u32, Option<JoinHandle<()>>>,
}

impl FetchTasks {
    pub(crate) fn new(options: NetOptions, read_body: bool) -> Self {
        Self {
            options,
            read_body,
            tasks: BTreeMap::new(),
        }
    }

    pub(crate) fn spawn<E: Send + 'static>(
        &mut self,
        id: RequestId,
        url: &str,
        events: &EventSender<E>,
        wrap: fn(FetchEvent) -> E,
    ) {
        let events = events.clone();
        let Some(url) = request_url(url) else {
            self.tasks.insert(id.0, None);
            let _ = events.send(wrap(FetchEvent::Failed(id)));
            return;
        };
        let read_body = self.read_body;
        let options = self.options.clone();
        let task = self.options.handle.spawn(async move {
            let event = match get(&url, read_body, &options).await {
                Some(response) => FetchEvent::Response(id, response),
                None => FetchEvent::Failed(id),
            };
            let _ = events.send(wrap(event));
        });
        self.tasks.insert(id.0, Some(task));
    }

    /// Whether a settled request is still awaited; it is forgotten either way.
    pub(crate) fn admit(&mut self, id: RequestId) -> bool {
        self.tasks.remove(&id.0).is_some()
    }

    pub(crate) fn abort(&mut self, id: RequestId) {
        if let Some(Some(task)) = self.tasks.remove(&id.0) {
            task.abort();
        }
    }
}

impl Drop for FetchTasks {
    fn drop(&mut self) {
        for task in self.tasks.values().flatten() {
            task.abort();
        }
    }
}

enum Framing {
    Chunked,
    Length(usize),
    UntilClose,
}

struct Reader {
    stream: TcpStream,
    buffer: Vec<u8>,
}

impl Reader {
    async fn fill(&mut self) -> Option<()> {
        let mut chunk = [0u8; 4096];
        let n = self.stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        self.buffer.extend_from_slice(&chunk[..n]);
        Some(())
    }

    /// Bytes through the first occurrence of `delimiter`, which is consumed but not returned.
    async fn until(&mut self, delimiter: &[u8]) -> Option<Vec<u8>> {
        loop {
            if let Some(i) = self
                .buffer
                .windows(delimiter.len())
                .position(|w| w == delimiter)
            {
                let out = self.buffer[..i].to_vec();
                self.buffer.drain(..i + delimiter.len());
                return Some(out);
            }
            if self.buffer.len() > MAX_HEAD {
                return None;
            }
            self.fill().await?;
        }
    }

    async fn take(&mut self, n: usize) -> Option<Vec<u8>> {
        while self.buffer.len() < n {
            self.fill().await?;
        }
        Some(self.buffer.drain(..n).collect())
    }

    async fn rest(mut self) -> Option<Vec<u8>> {
        self.stream.read_to_end(&mut self.buffer).await.ok()?;
        Some(self.buffer)
    }

    async fn body(mut self, framing: Framing) -> Option<Vec<u8>> {
        match framing {
            Framing::Length(n) => self.take(n).await,
            Framing::UntilClose => self.rest().await,
            Framing::Chunked => {
                let mut body = Vec::new();
                loop {
                    let line = self.until(b"\r\n").await?;
                    let size = std::str::from_utf8(&line).ok()?;
                    let size = size.split(';').next()?.trim_ascii();
                    let size = usize::from_str_radix(size, 16).ok()?;
                    if size == 0 {
                        return Some(body);
                    }
                    body.extend(self.take(size).await?);
                    self.take(2).await?;
                }
            }
        }
    }
}

/// The URL of a request that reaches the network.
fn request_url(url: &str) -> Option<LiveUrl> {
    LiveUrl::parse(url).filter(|url| {
        url.scheme == "http"
            && !url.has_credentials
            && !url.dangling_markup
            && !url.port_is_blocked()
    })
}

/// `fetch(url)` reduced to the response, read through the body when `read_body` is set;
/// `None` is a network error.
async fn get(url: &LiveUrl, read_body: bool, options: &NetOptions) -> Option<Response> {
    let (to_host, to_port) = options.dial(&url.host, url.port);
    let mut stream = TcpStream::connect((to_host.as_str(), to_port)).await.ok()?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        url.target, url.authority
    );
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut reader = Reader {
        stream,
        buffer: Vec::new(),
    };
    let (status, head) = loop {
        let head = reader.until(b"\r\n\r\n").await?;
        let status = status_code(&head)?;
        if !(100..200).contains(&status) {
            break (status, head);
        }
    };
    let body = if read_body {
        let body = reader.body(framing(&head)).await?;
        // Blink strips one BOM. Node's fetch, which main uses, strips two.
        let body = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&body);
        String::from_utf8_lossy(body).into_owned()
    } else {
        String::new()
    };
    Some(Response {
        status,
        url: url.href(),
        body,
    })
}

fn status_code(head: &[u8]) -> Option<u16> {
    let line_end = head
        .windows(2)
        .position(|w| w == b"\r\n")
        .unwrap_or(head.len());
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

fn framing(head: &[u8]) -> Framing {
    let head = String::from_utf8_lossy(head);
    let mut framing = Framing::UntilClose;
    for line in head.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim_ascii();
        if name.eq_ignore_ascii_case("transfer-encoding")
            && value
                .rsplit(',')
                .next()
                .is_some_and(|v| v.trim_ascii().eq_ignore_ascii_case("chunked"))
        {
            return Framing::Chunked;
        }
        if name.eq_ignore_ascii_case("content-length")
            && !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit())
            && let Some(n) = value.bytes().try_fold(0usize, |n, b| {
                n.checked_mul(10)?.checked_add(usize::from(b - b'0'))
            })
        {
            framing = Framing::Length(n);
        }
    }
    framing
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn options() -> NetOptions {
        NetOptions::new(tokio::runtime::Handle::current())
    }

    async fn serve_once(reply: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            socket.write_all(reply).await.unwrap();
            let mut rest = Vec::new();
            let _ = socket.read_to_end(&mut rest).await;
        });
        format!("http://127.0.0.1:{port}/?action=getdevices")
    }

    #[tokio::test]
    async fn bodies_follow_content_length_and_chunked_framing() {
        let url =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n\xEF\xBB\xBF{\"a\":1}").await;
        let response = get(&request_url(&url).unwrap(), true, &options())
            .await
            .unwrap();
        assert_eq!((response.status, response.body.as_str()), (200, "{\"a\""));

        let url = serve_once(
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;x=1\r\n{\"a\r\n4\r\n\":1}\r\n0\r\n\r\n",
        )
        .await;
        let response = get(&request_url(&url).unwrap(), true, &options())
            .await
            .unwrap();
        assert_eq!(
            (response.status, response.body.as_str()),
            (200, "{\"a\":1}")
        );
        assert_eq!(response.url, url);

        let url = serve_once(b"HTTP/1.1 404 Not Found\r\nContent-Length: 5\r\n\r\nnope!").await;
        let response = get(&request_url(&url).unwrap(), false, &options())
            .await
            .unwrap();
        assert_eq!((response.status, response.body.as_str()), (404, ""));
    }
}
