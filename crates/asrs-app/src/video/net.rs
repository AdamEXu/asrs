//! The video processor's HTTP: `fetch` with a timeout and the `download` package's GET to a file.

use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use asrs_services::video::FetchResponse;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body, http};

const MAX_BODY: u64 = 256 << 20;

type Failure = Box<dyn std::error::Error + Send + Sync>;

fn agent(timeout: Option<Duration>) -> Agent {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = TlsConfig::builder()
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(provider)
        .build();
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(timeout)
        .tls_config(tls)
        .build()
        .new_agent()
}

fn get(
    url: &str,
    headers: &[(String, String)],
    timeout: Option<Duration>,
) -> Result<http::Response<Body>, Failure> {
    let mut request = agent(timeout).get(url);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    Ok(request.call()?)
}

/// `fetch(url, { headers, signal: AbortSignal.timeout(timeout) })` and its body; `None` where it
/// rejects.
pub fn fetch(url: &str, headers: &[(String, String)], timeout: Duration) -> Option<FetchResponse> {
    let mut response = get(url, headers, Some(timeout)).ok()?;
    let status = response.status().as_u16();
    let body = response.body_mut().with_config().limit(MAX_BODY);
    let body = body.read_to_vec().ok()?;
    Some(FetchResponse { status, body })
}

/// `download(url, folder, { filename })`.
pub fn download(url: &str, folder: &Path, filename: &str) -> Result<(), Failure> {
    let mut response = get(url, &[], None)?;
    if !response.status().is_success() {
        return Err(format!("{url}: {}", response.status()).into());
    }
    fs::create_dir_all(folder)?;
    let mut file = File::create(folder.join(filename))?;
    io::copy(&mut response.body_mut().as_reader(), &mut file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    /// Answers one request per response and returns their request heads.
    fn serve(responses: Vec<String>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("address"));
        let server = thread::spawn(move || {
            let mut heads = Vec::new();
            for response in responses {
                let (stream, _) = listener.accept().expect("accept");
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                while !head.ends_with("\r\n\r\n") {
                    if reader.read_line(&mut head).expect("request") == 0 {
                        break;
                    }
                }
                heads.push(head);
                let mut stream = reader.into_inner();
                stream.write_all(response.as_bytes()).expect("response");
            }
            heads
        });
        (address, server)
    }

    fn response(status: &str, extra: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn fetch_sends_its_headers_and_keeps_error_statuses() {
        let (address, server) = serve(vec![response("401 Unauthorized", "", "{}")]);
        let headers = [("X-TBA-Auth-Key".to_owned(), "key".to_owned())];
        let reply = fetch(&format!("{address}/api"), &headers, Duration::from_secs(10));
        assert_eq!(
            reply,
            Some(FetchResponse {
                status: 401,
                body: b"{}".to_vec()
            })
        );
        let head = server.join().expect("server").remove(0).to_lowercase();
        assert!(head.starts_with("get /api "), "{head}");
        assert!(head.contains("x-tba-auth-key: key\r\n"), "{head}");
    }

    #[test]
    fn an_unreachable_server_rejects() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        drop(listener);
        assert_eq!(
            fetch(&format!("http://{address}/"), &[], Duration::from_secs(3)),
            None
        );
    }

    #[test]
    fn a_silent_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let reply = fetch(
            &format!("http://{address}/"),
            &[],
            Duration::from_millis(200),
        );
        assert_eq!(reply, None);
    }

    #[test]
    fn downloads_follow_redirects_and_reject_errors() {
        let dir = std::env::temp_dir().join(format!("asrs-video-net-{}", std::process::id()));
        let (address, server) = serve(vec![
            response("302 Found", "Location: /final\r\n", ""),
            response("200 OK", "", "binary"),
            response("404 Not Found", "", "gone"),
        ]);
        download(&format!("{address}/start"), &dir, "ffmpeg-download").expect("download");
        let file = dir.join("ffmpeg-download");
        assert_eq!(fs::read(&file).expect("file"), b"binary");
        assert!(download(&format!("{address}/gone"), &dir, "other").is_err());
        assert!(!dir.join("other").exists());
        assert!(server.join().expect("server")[1].starts_with("GET /final "));
        fs::remove_dir_all(&dir).expect("clean up");
    }
}
