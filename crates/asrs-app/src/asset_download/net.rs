//! The HTTP of the asset download loop: main's `fetch`, and each request got 8 sends.

use std::cell::Cell;
use std::io::{ErrorKind, Read};
use std::sync::Arc;
use std::time::Duration;

use asrs_services::asset_downloader::{FetchOutcome, FetchRequest, HttpOutcome, HttpRequest};
use rustls::CertificateError;
use ureq::config::AutoHeaderValue;
use ureq::http::{Response, Uri};
use ureq::tls::{RootCerts, TlsConfig};
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport,
};
use ureq::{Agent, Body, Error, RequestBuilder, config::Config};

/// GitHub's limit on a release asset.
const MAX_BODY: u64 = 2 << 30;
/// undici's `maxRedirect`.
const FETCH_REDIRECTS: u32 = 20;

thread_local! {
    /// What node calls the failure ureq does not tell apart: a lookup's, or a connection's that
    /// ureq reports as a plain end of the stream.
    static CODE: Cell<Option<&'static str>> = const { Cell::new(None) };
}

#[derive(Clone)]
pub(super) struct Net {
    agent: Agent,
}

impl Net {
    pub(super) fn new() -> Net {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(provider)
            .build();
        let config: Config = Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_idle_connections(0)
            .proxy(None)
            .tls_config(tls)
            .build();
        Net {
            agent: Agent::with_parts(
                config,
                DefaultConnector::new().chain(Watch),
                Lookup::default(),
            ),
        }
    }

    /// Fails every request, those in flight and those still to come.
    pub(super) fn shutdown(&self) {
        self.agent.clone().shutdown();
    }

    pub(super) fn fetch(&self, request: &FetchRequest) -> FetchOutcome {
        let timeout = Duration::from_millis(request.timeout_ms as u64);
        let call = headers(self.agent.get(&request.url), &request.headers)
            .config()
            .timeout_global(Some(timeout))
            .max_redirects(FETCH_REDIRECTS)
            .build()
            .call();
        let Ok(mut response) = call else {
            return FetchOutcome::NetworkError;
        };
        let status = response.status().as_u16();
        let body = match status {
            204 | 304 => None,
            _ => match response
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_vec()
            {
                Ok(body) => Some(body),
                Err(_) => return FetchOutcome::NetworkError,
            },
        };
        FetchOutcome::Response { status, body }
    }

    pub(super) fn request(&self, request: &HttpRequest) -> HttpOutcome {
        CODE.set(None);
        let call = headers(self.agent.get(&request.url), &request.headers)
            .config()
            .accept(AutoHeaderValue::None)
            .build()
            .call();
        match call {
            Ok(response) => answer(response),
            Err(e) => HttpOutcome::Error(code(&e)),
        }
    }
}

fn headers<T>(request: RequestBuilder<T>, headers: &[(String, String)]) -> RequestBuilder<T> {
    headers.iter().fold(request, |request, (name, value)| {
        request.header(name, value)
    })
}

fn answer(response: Response<Body>) -> HttpOutcome {
    let (parts, body) = response.into_parts();
    let status = parts.status.as_u16();
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(name, value)| {
            let value = value.as_bytes().iter().map(|&b| char::from(b)).collect();
            (name.as_str().to_owned(), value)
        })
        .collect();
    let mut data = Vec::new();
    if !(200..=299).contains(&status) {
        return HttpOutcome::Response {
            status,
            headers,
            body: data,
        };
    }
    let read = body.into_reader().take(MAX_BODY).read_to_end(&mut data);
    let code = match (read, CODE.take()) {
        (Ok(_), None) => {
            return HttpOutcome::Response {
                status,
                headers,
                body: data,
            };
        }
        (_, Some(code)) => Some(code.to_owned()),
        (Err(e), None) if e.kind() == ErrorKind::UnexpectedEof => None,
        (Err(e), None) => match Error::from(e) {
            Error::Decompress(..) => return HttpOutcome::Undecodable,
            e => Some(code(&e)),
        },
    };
    HttpOutcome::Truncated {
        status,
        headers,
        body: data,
        code,
    }
}

/// node's `error.code` for a failed request, as far as got's retries tell codes apart.
fn code(error: &Error) -> String {
    if let Some(code) = CODE.take() {
        return code.to_owned();
    }
    match error {
        Error::Io(e) => {
            let inner = e.get_ref().and_then(|e| e.downcast_ref());
            inner.map_or_else(|| io_code(e.kind()), tls_code)
        }
        Error::Rustls(e) => tls_code(e),
        Error::HostNotFound => "ENOTFOUND",
        Error::Timeout(_) => "ETIMEDOUT",
        _ => "EPROTO",
    }
    .to_owned()
}

fn io_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::ConnectionRefused => "ECONNREFUSED",
        ErrorKind::BrokenPipe => "EPIPE",
        ErrorKind::NetworkUnreachable => "ENETUNREACH",
        ErrorKind::HostUnreachable => "EHOSTUNREACH",
        ErrorKind::TimedOut => "ETIMEDOUT",
        _ => "ECONNRESET",
    }
}

fn tls_code(error: &rustls::Error) -> &'static str {
    let rustls::Error::InvalidCertificate(error) = error else {
        return "EPROTO";
    };
    match error {
        CertificateError::Expired | CertificateError::ExpiredContext { .. } => "CERT_HAS_EXPIRED",
        CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
            "CERT_NOT_YET_VALID"
        }
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
            "ERR_TLS_CERT_ALTNAME_INVALID"
        }
        CertificateError::Revoked => "CERT_REVOKED",
        _ => "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
    }
}

#[derive(Debug, Default)]
struct Lookup(DefaultResolver);

impl Resolver for Lookup {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, Error> {
        let resolved = self.0.resolve(uri, config, timeout);
        if let Err(Error::Io(e)) = &resolved {
            let windows_try_again = cfg!(windows) && e.raw_os_error() == Some(11002);
            let again = windows_try_again || e.to_string().contains("Temporary failure");
            CODE.set(Some(if again { "EAI_AGAIN" } else { "ENOTFOUND" }));
        }
        resolved
    }
}

/// Notes a connection that is reset, which ureq takes for the end of the body.
#[derive(Debug)]
struct Watch;

impl<In: Transport> Connector<In> for Watch {
    type Out = Watched<In>;

    fn connect(
        &self,
        _: &ConnectionDetails<'_>,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, Error> {
        Ok(chained.map(Watched))
    }
}

#[derive(Debug)]
struct Watched<T>(T);

impl<T: Transport> Transport for Watched<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.0.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
        self.0.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
        let awaited = self.0.await_input(timeout);
        if let Err(Error::Io(e)) = &awaited
            && matches!(
                e.kind(),
                ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            )
        {
            CODE.set(Some("ECONNRESET"));
        }
        awaited
    }

    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }

    fn is_tls(&self) -> bool {
        self.0.is_tls()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    const PATIENCE: Duration = Duration::from_secs(60);

    /// Reads one request and hands its connection to `answer`; the request's head comes back.
    fn serve(answer: impl FnOnce(TcpStream) + Send + 'static) -> (SocketAddr, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.set_read_timeout(Some(PATIENCE)).expect("timeout");
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).expect("request");
                head.push(byte[0]);
            }
            answer(stream);
            String::from_utf8(head).expect("head")
        });
        (address, server)
    }

    fn get(answer: impl FnOnce(TcpStream) + Send + 'static) -> (HttpOutcome, String) {
        let (address, server) = serve(answer);
        let outcome = Net::new().request(&HttpRequest {
            url: format!("http://{address}/a%20b.zip?q=1"),
            method: "GET".to_owned(),
            headers: vec![
                ("accept-encoding".to_owned(), "gzip, deflate".to_owned()),
                ("user-agent".to_owned(), "agent".to_owned()),
            ],
        });
        (outcome, server.join().expect("server"))
    }

    fn fetch(url: String, timeout_ms: f64) -> FetchOutcome {
        Net::new().fetch(&FetchRequest {
            url,
            method: "GET".to_owned(),
            headers: vec![("pragma".to_owned(), "no-cache".to_owned())],
            timeout_ms,
        })
    }

    fn send(mut stream: &TcpStream, data: &[u8]) {
        stream.write_all(data).expect("response");
    }

    /// Ends the connection with an RST, once the peer has read what was sent.
    fn reset(stream: TcpStream) {
        thread::sleep(Duration::from_millis(200));
        let socket = socket2::Socket::from(stream);
        socket.set_linger(Some(Duration::ZERO)).expect("linger");
    }

    #[test]
    fn a_request_sends_what_it_is_given_and_no_more() {
        let (outcome, head) = get(|stream| {
            send(
                &stream,
                b"HTTP/1.1 200 OK\r\nX-Name: caf\xe9\r\nContent-Length: 4\r\n\r\nbody",
            );
        });
        let mut lines: Vec<String> = head.trim_end().lines().map(str::to_owned).collect();
        assert_eq!(lines.remove(0), "GET /a%20b.zip?q=1 HTTP/1.1");
        let mut names: Vec<String> = lines
            .iter()
            .map(|line| line.split(':').next().unwrap_or_default().to_lowercase())
            .collect();
        names.sort();
        assert_eq!(names, ["accept-encoding", "host", "user-agent"], "{head}");
        let HttpOutcome::Response {
            status,
            headers,
            body,
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!((status, &body[..]), (200, &b"body"[..]));
        assert_eq!(headers[0], ("x-name".to_owned(), "caf\u{e9}".to_owned()));
    }

    #[test]
    fn redirects_and_errors_come_back_without_their_bodies() {
        for status in [302, 404, 500] {
            let (outcome, _) = get(move |stream| {
                let head = format!(
                    "HTTP/1.1 {status} X\r\nLocation: /next\r\nContent-Length: 100\r\n\r\npart"
                );
                send(&stream, head.as_bytes());
                let _ = (&stream).read(&mut [0]);
            });
            let HttpOutcome::Response {
                status: got, body, ..
            } = outcome
            else {
                panic!("{outcome:?}");
            };
            assert_eq!((got, body.len()), (status, 0));
        }
    }

    #[test]
    fn a_large_body_is_read_whole() {
        let body: Vec<u8> = (0..24u32 << 20).map(|i| (i * 31 >> 7) as u8).collect();
        let sent = body.clone();
        let (outcome, _) = get(move |stream| {
            send(
                &stream,
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            );
            for chunk in sent.chunks(1 << 20) {
                send(&stream, format!("{:x}\r\n", chunk.len()).as_bytes());
                send(&stream, chunk);
                send(&stream, b"\r\n");
            }
            send(&stream, b"0\r\n\r\n");
        });
        let HttpOutcome::Response { body: got, .. } = outcome else {
            panic!("not a response");
        };
        assert!(got == body, "{} bytes of {}", got.len(), body.len());
    }

    #[test]
    fn a_body_that_ends_early_keeps_what_arrived() {
        let length: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhalf";
        let chunked: &[u8] = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n64\r\nhalf";
        let until_close: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nhalf";
        for (head, resets) in [
            (length, false),
            (length, true),
            (chunked, false),
            (chunked, true),
            (until_close, true),
        ] {
            let (outcome, _) = get(move |stream| {
                send(&stream, head);
                if resets {
                    reset(stream);
                }
            });
            let HttpOutcome::Truncated { body, code, .. } = outcome else {
                panic!("{outcome:?}");
            };
            assert_eq!(body, b"half");
            let expected = resets.then_some("ECONNRESET");
            assert_eq!(code.as_deref(), expected, "{}", head.escape_ascii());
        }
    }

    #[test]
    fn a_connection_that_drops_before_a_response_is_an_error() {
        let (outcome, _) = get(|stream| send(&stream, b"HTTP/1.1 200 OK\r\nConte"));
        assert_eq!(outcome, HttpOutcome::Error("ECONNRESET".to_owned()));
        let (outcome, _) = get(reset);
        assert_eq!(outcome, HttpOutcome::Error("ECONNRESET".to_owned()));
    }

    #[test]
    fn a_closed_port_refuses() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/", listener.local_addr().expect("address"));
        drop(listener);
        let outcome = Net::new().request(&HttpRequest {
            url: url.clone(),
            method: "GET".to_owned(),
            headers: Vec::new(),
        });
        assert_eq!(outcome, HttpOutcome::Error("ECONNREFUSED".to_owned()));
        assert_eq!(fetch(url, 60_000.0), FetchOutcome::NetworkError);
    }

    #[test]
    fn a_body_unlike_its_encoding_is_undecodable() {
        let (outcome, _) = get(|stream| {
            send(
                &stream,
                b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 14\r\n\r\nnot compressed",
            );
        });
        assert_eq!(outcome, HttpOutcome::Undecodable);
    }

    #[test]
    fn a_fetch_ends_at_its_timeout_wherever_the_server_stalls() {
        for sent in [
            &b""[..],
            b"HTTP/1.1 200 OK\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n[",
        ] {
            let (address, server) = serve(move |stream| {
                send(&stream, sent);
                let _ = (&stream).read(&mut [0]);
            });
            let outcome = fetch(format!("http://{address}/"), 300.0);
            assert_eq!(outcome, FetchOutcome::NetworkError);
            server.join().expect("server");
        }
    }

    #[test]
    fn a_null_body_is_told_from_an_empty_one() {
        for (status, body) in [(204, None), (304, None), (200, Some(Vec::new()))] {
            let (address, _server) = serve(move |stream| {
                let head = format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n");
                send(&stream, head.as_bytes());
            });
            let outcome = fetch(format!("http://{address}/"), 60_000.0);
            assert_eq!(outcome, FetchOutcome::Response { status, body });
        }
    }

    #[test]
    fn shutdown_ends_requests_wherever_they_wait() {
        for sent in [
            &b""[..],
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhalf",
        ] {
            let net = Net::new();
            let (ready, waiting) = std::sync::mpsc::channel();
            let (address, server) = serve(move |stream| {
                send(&stream, sent);
                ready.send(()).expect("ready");
                let _ = (&stream).read(&mut [0]);
            });
            let request = {
                let net = net.clone();
                thread::spawn(move || {
                    net.request(&HttpRequest {
                        url: format!("http://{address}/"),
                        method: "GET".to_owned(),
                        headers: Vec::new(),
                    })
                })
            };
            waiting.recv_timeout(PATIENCE).expect("a request");
            let start = Instant::now();
            net.shutdown();
            let outcome = request.join().expect("the request");
            assert!(
                !matches!(outcome, HttpOutcome::Response { .. }),
                "{outcome:?}"
            );
            assert!(start.elapsed() < PATIENCE);
            server.join().expect("server");
        }
    }

    #[test]
    fn certificate_errors_have_node_codes() {
        use rustls::Error::InvalidCertificate;
        let expired = tls_code(&InvalidCertificate(CertificateError::Expired));
        assert_eq!(expired, "CERT_HAS_EXPIRED");
        let name = tls_code(&InvalidCertificate(CertificateError::NotValidForName));
        assert_eq!(name, "ERR_TLS_CERT_ALTNAME_INVALID");
        let issuer = tls_code(&InvalidCertificate(CertificateError::UnknownIssuer));
        assert_eq!(issuer, "UNABLE_TO_VERIFY_LEAF_SIGNATURE");
        assert_eq!(tls_code(&rustls::Error::HandshakeNotComplete), "EPROTO");
    }
}
