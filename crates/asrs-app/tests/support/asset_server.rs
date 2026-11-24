//! A stand-in for GitHub on a loopback port of its own.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const PATIENCE: Duration = Duration::from_secs(60);
pub const RELEASES: &str = "/repos/Mechanical-Advantage/AdvantageScopeAssets/releases";

#[derive(Clone, Default)]
pub struct Gate(Arc<AtomicBool>);

impl Gate {
    pub fn open(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn wait(&self) {
        let start = Instant::now();
        while !self.0.load(Ordering::SeqCst) && start.elapsed() < PATIENCE {
            thread::sleep(Duration::from_millis(2));
        }
    }
}

#[derive(Clone)]
pub enum Reply {
    Send {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    },
    /// As the reply, once the gate opens.
    Hold(Gate, Box<Reply>),
    /// Half of a body, and then nothing until the client leaves.
    Stall(Vec<u8>),
    /// Half of a body, and then the end of the connection.
    Cut(Vec<u8>),
    /// The end of the connection at once.
    Drop,
}

impl Reply {
    pub fn body(body: impl Into<Vec<u8>>) -> Reply {
        Reply::Send {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Reply {
        Reply::Send {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn redirect(to: String) -> Reply {
        Reply::Send {
            status: 302,
            headers: vec![("Location", to)],
            body: Vec::new(),
        }
    }
}

#[derive(Default)]
struct State {
    replies: Mutex<HashMap<String, VecDeque<Reply>>>,
    requests: Mutex<Vec<(String, String)>>,
    /// Clients that left while the server had more to send.
    left: AtomicUsize,
}

#[derive(Clone)]
pub struct Server {
    address: SocketAddr,
    state: Arc<State>,
}

impl Server {
    pub fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let state = Arc::new(State::default());
        let server = Server { address, state };
        let accepting = server.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let server = accepting.clone();
                thread::spawn(move || {
                    if let Ok(stream) = stream {
                        server.answer(stream);
                    }
                });
            }
        });
        server
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    /// Queues the answer to the next request for `path` without one; a path without answers is a
    /// 404.
    pub fn reply(&self, path: &str, reply: Reply) {
        let mut replies = self.state.replies.lock().expect("replies");
        replies.entry(path.to_owned()).or_default().push_back(reply);
    }

    /// The next reply to the releases API, whose assets are served from `/<name>`.
    pub fn releases(&self, tag: &str, names: &[&str]) {
        self.reply(RELEASES, Reply::body(self.release_list(tag, names)));
    }

    pub fn release_list(&self, tag: &str, names: &[&str]) -> String {
        let asset = |name: &&str| {
            let file = name.rsplit('/').next().expect("a name");
            let url = self.url(&format!("/{file}"));
            format!(r#"{{"name":"{name}","browser_download_url":"{url}"}}"#)
        };
        let assets: Vec<String> = names.iter().map(asset).collect();
        format!(
            r#"[{{"tag_name":"{tag}","assets":[{}]}}]"#,
            assets.join(",")
        )
    }

    /// The paths and heads requested so far.
    pub fn requests(&self) -> Vec<(String, String)> {
        self.state.requests.lock().expect("requests").clone()
    }

    pub fn count(&self, path: &str) -> usize {
        self.requests().iter().filter(|(p, _)| p == path).count()
    }

    pub fn left(&self) -> usize {
        self.state.left.load(Ordering::SeqCst)
    }

    fn answer(&self, mut stream: TcpStream) {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            if stream.read_exact(&mut byte).is_err() {
                return;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
        let log = (path.clone(), head);
        self.state.requests.lock().expect("requests").push(log);
        let queued = {
            let mut replies = self.state.replies.lock().expect("replies");
            replies.get_mut(&path).and_then(VecDeque::pop_front)
        };
        self.send(&mut stream, queued.unwrap_or_else(|| Reply::status(404)));
    }

    fn send(&self, stream: &mut TcpStream, reply: Reply) {
        match reply {
            Reply::Send {
                status,
                headers,
                body,
            } => {
                let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
                for (name, value) in headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
            Reply::Hold(gate, reply) => {
                gate.wait();
                self.send(stream, *reply);
            }
            Reply::Stall(body) => {
                send_half(stream, &body);
                let _ = stream.set_read_timeout(Some(PATIENCE));
                let waited = Instant::now();
                let _ = stream.read(&mut [0]);
                if waited.elapsed() < PATIENCE {
                    self.state.left.fetch_add(1, Ordering::SeqCst);
                }
            }
            Reply::Cut(body) => send_half(stream, &body),
            Reply::Drop => {}
        }
    }
}

fn send_half(stream: &mut TcpStream, body: &[u8]) {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body[..body.len() / 2]);
    let _ = stream.flush();
}

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &byte| {
        (0..8).fold(crc ^ u32::from(byte), |crc, _| {
            (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1))
        })
    })
}

/// A zip of stored entries with their unix modes.
pub fn zip(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for &(name, mode, data) in entries {
        let offset = out.len() as u32;
        let mut common = Vec::new();
        common.extend([20u8, 0, 0, 8, 0, 0, 0, 0, 0, 0]);
        common.extend(crc32(data).to_le_bytes());
        common.extend((data.len() as u32).to_le_bytes());
        common.extend((data.len() as u32).to_le_bytes());
        common.extend((name.len() as u16).to_le_bytes());
        common.extend([0, 0]);
        out.extend(b"PK\x03\x04");
        out.extend(&common);
        out.extend(name.as_bytes());
        out.extend(data);
        central.extend(b"PK\x01\x02\x14\x03");
        central.extend(&common);
        central.extend([0u8; 6]);
        central.extend((mode << 16).to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let start = out.len() as u32;
    let count = (entries.len() as u16).to_le_bytes();
    out.extend(&central);
    out.extend(b"PK\x05\x06\0\0\0\0");
    out.extend(count);
    out.extend(count);
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend([0, 0]);
    out
}
