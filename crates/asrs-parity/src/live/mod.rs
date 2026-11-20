//! Live sessions (`parity/specs/live/<protocol>/<name>.session.json`, format in
//! `oracle/README.md`): the session file, the trace the oracle records, and the scripted mock
//! servers, which mirror `oracle/src/live/*.ts` so a port can run against the same servers.
//!
//! [`ServerHost`] is sans-IO: a harness feeds it what its client sends, with the send time, and
//! asks it for what the server does at each time. [`replay`] drives it from a recorded trace.

pub mod msgpack;
mod nt4;
mod server;
mod simple;

use std::fs;
use std::path::{Path, PathBuf};

use crate::cve1::Value;
use crate::json::{self, Json};
use crate::utf16::Utf16String;
use crate::vectors::{self, Record};

pub use server::{
    MockServer, check_json_numbers, decode_uri_component, js_order, url_port, url_query,
};
pub use simple::seconds_text;

pub const SESSION_SUFFIX: &str = ".session.json";

#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Text(String),
    Binary(Vec<u8>),
}

impl Payload {
    fn value(&self) -> Value {
        match self {
            Payload::Text(s) => Value::string(s),
            Payload::Binary(b) => Value::Bytes(b.clone()),
        }
    }

    fn from_value(v: &Value) -> Result<Self, String> {
        match v {
            Value::String(s) => Ok(Payload::Text(s.to_string_lossy())),
            Value::Bytes(b) => Ok(Payload::Binary(b.clone())),
            other => Err(format!(
                "payload must be a string or bytes, not {}",
                other.kind()
            )),
        }
    }
}

/// What the client side sends, as the trace records it.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    WsOpen {
        id: u32,
        url: String,
        protocols: Vec<String>,
    },
    WsSend {
        id: u32,
        data: Payload,
    },
    WsClose {
        id: u32,
    },
    HttpGet {
        id: u32,
        url: String,
    },
    HttpAbort {
        id: u32,
    },
    TcpOpen {
        id: u32,
        host: String,
        port: f64,
    },
    TcpSend {
        id: u32,
        data: Vec<u8>,
    },
    TcpClose {
        id: u32,
    },
}

/// What a mock server does, as the trace records it.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerEvent {
    WsAccept { id: u32, protocol: String },
    WsReject { id: u32 },
    WsPush { id: u32, data: Payload },
    WsDrop { id: u32 },
    HttpReply { id: u32, status: f64, body: String },
    HttpRefuse { id: u32 },
    TcpAccept { id: u32 },
    TcpRefuse { id: u32 },
    TcpPush { id: u32, data: Vec<u8> },
    TcpDrop { id: u32 },
}

/// A trace line: `{"t":<ms>,"ev":<name>,"id":<connection>,"v":<CVE-1>}`, with `id` and `v`
/// left out where an event has none.
pub fn trace_line(t: f64, ev: &str, id: Option<u32>, v: Option<&Value>) -> String {
    let mut line = format!("{{\"t\":{},\"ev\":\"{ev}\"", Json::Number(t).to_compact());
    if let Some(id) = id {
        line.push_str(&format!(",\"id\":{id}"));
    }
    if let Some(v) = v {
        line.push_str(",\"v\":");
        line.push_str(&v.encode());
    }
    line.push('}');
    line
}

fn strings(v: &Value) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or("expected a list of strings")?
        .iter()
        .map(|s| {
            s.as_str()
                .map(Utf16String::to_string_lossy)
                .ok_or_else(|| "expected a string".to_owned())
        })
        .collect()
}

impl ClientEvent {
    pub const NAMES: [&str; 8] = [
        "ws-open",
        "ws-send",
        "ws-close",
        "http-get",
        "http-abort",
        "tcp-open",
        "tcp-send",
        "tcp-close",
    ];

    pub fn id(&self) -> u32 {
        match self {
            ClientEvent::WsOpen { id, .. }
            | ClientEvent::WsSend { id, .. }
            | ClientEvent::WsClose { id }
            | ClientEvent::HttpGet { id, .. }
            | ClientEvent::HttpAbort { id }
            | ClientEvent::TcpOpen { id, .. }
            | ClientEvent::TcpSend { id, .. }
            | ClientEvent::TcpClose { id } => *id,
        }
    }

    pub fn line(&self, t: f64) -> String {
        let (ev, v) = match self {
            ClientEvent::WsOpen { url, protocols, .. } => (
                "ws-open",
                Some(Value::Array(vec![
                    Value::string(url),
                    Value::Array(protocols.iter().map(|p| Value::string(p)).collect()),
                ])),
            ),
            ClientEvent::WsSend { data, .. } => ("ws-send", Some(data.value())),
            ClientEvent::WsClose { .. } => ("ws-close", None),
            ClientEvent::HttpGet { url, .. } => ("http-get", Some(Value::string(url))),
            ClientEvent::HttpAbort { .. } => ("http-abort", None),
            ClientEvent::TcpOpen { host, port, .. } => (
                "tcp-open",
                Some(Value::Array(vec![
                    Value::string(host),
                    Value::number(*port),
                ])),
            ),
            ClientEvent::TcpSend { data, .. } => ("tcp-send", Some(Value::Bytes(data.clone()))),
            ClientEvent::TcpClose { .. } => ("tcp-close", None),
        };
        trace_line(t, ev, Some(self.id()), v.as_ref())
    }

    fn parse(ev: &str, id: u32, v: Option<&Value>) -> Result<Self, String> {
        let v = || v.ok_or_else(|| format!("{ev} needs \"v\""));
        let item = |i: usize| -> Result<&Value, String> {
            v()?.as_array()
                .and_then(|a| a.get(i))
                .ok_or_else(|| format!("{ev}: v[{i}] missing"))
        };
        let text = |value: &Value| {
            value
                .as_str()
                .map(Utf16String::to_string_lossy)
                .ok_or_else(|| format!("{ev}: expected a string"))
        };
        Ok(match ev {
            "ws-open" => ClientEvent::WsOpen {
                id,
                url: text(item(0)?)?,
                protocols: strings(item(1)?)?,
            },
            "ws-send" => ClientEvent::WsSend {
                id,
                data: Payload::from_value(v()?)?,
            },
            "ws-close" => ClientEvent::WsClose { id },
            "http-get" => ClientEvent::HttpGet {
                id,
                url: text(v()?)?,
            },
            "http-abort" => ClientEvent::HttpAbort { id },
            "tcp-open" => ClientEvent::TcpOpen {
                id,
                host: text(item(0)?)?,
                port: item(1)?.as_f64().ok_or("tcp-open: port must be a number")?,
            },
            "tcp-send" => match Payload::from_value(v()?)? {
                Payload::Binary(data) => ClientEvent::TcpSend { id, data },
                Payload::Text(_) => return Err("tcp-send carries bytes".to_owned()),
            },
            "tcp-close" => ClientEvent::TcpClose { id },
            other => return Err(format!("{other} is not a client event")),
        })
    }
}

impl ServerEvent {
    pub const NAMES: [&str; 10] = [
        "ws-accept",
        "ws-reject",
        "ws-push",
        "ws-drop",
        "http-reply",
        "http-refuse",
        "tcp-accept",
        "tcp-refuse",
        "tcp-push",
        "tcp-drop",
    ];

    pub fn line(&self, t: f64) -> String {
        let (ev, id, v) = match self {
            ServerEvent::WsAccept { id, protocol } => {
                ("ws-accept", id, Some(Value::string(protocol)))
            }
            ServerEvent::WsReject { id } => ("ws-reject", id, None),
            ServerEvent::WsPush { id, data } => ("ws-push", id, Some(data.value())),
            ServerEvent::WsDrop { id } => ("ws-drop", id, None),
            ServerEvent::HttpReply { id, status, body } => (
                "http-reply",
                id,
                Some(Value::Array(vec![
                    Value::number(*status),
                    Value::string(body),
                ])),
            ),
            ServerEvent::HttpRefuse { id } => ("http-refuse", id, None),
            ServerEvent::TcpAccept { id } => ("tcp-accept", id, None),
            ServerEvent::TcpRefuse { id } => ("tcp-refuse", id, None),
            ServerEvent::TcpPush { id, data } => ("tcp-push", id, Some(Value::Bytes(data.clone()))),
            ServerEvent::TcpDrop { id } => ("tcp-drop", id, None),
        };
        trace_line(t, ev, Some(*id), v.as_ref())
    }
}

/// One line of a session trace.
pub struct TraceEvent {
    pub t: f64,
    pub ev: String,
    pub id: Option<u32>,
    pub v: Option<Value>,
    pub line: String,
}

impl TraceEvent {
    pub fn from_record(record: &Record) -> Result<Self, String> {
        let at = |e: String| format!("line {}: {e}", record.line);
        let t = record
            .field("t")
            .and_then(Json::as_f64)
            .ok_or_else(|| at("needs a numeric \"t\"".to_owned()))?;
        let ev = record
            .string("ev")
            .ok_or_else(|| at("needs a string \"ev\"".to_owned()))?;
        let id = record
            .field("id")
            .and_then(Json::as_f64)
            .map(|id| id as u32);
        let v = match record.field("v") {
            None => None,
            Some(_) => Some(record.value("v").map_err(|e| at(e.to_string()))?),
        };
        Ok(Self {
            t,
            ev,
            id,
            v,
            line: record.text.clone(),
        })
    }

    pub fn is_server(&self) -> bool {
        ServerEvent::NAMES.contains(&self.ev.as_str())
    }

    pub fn client_event(&self) -> Result<Option<ClientEvent>, String> {
        if !ClientEvent::NAMES.contains(&self.ev.as_str()) {
            return Ok(None);
        }
        let id = self.id.ok_or_else(|| format!("{} needs an id", self.ev))?;
        ClientEvent::parse(&self.ev, id, self.v.as_ref()).map(Some)
    }
}

#[derive(Clone, Debug)]
pub struct Step {
    pub t: f64,
    pub op: Vec<Json>,
}

/// A parsed session file. Objects are in `JSON.parse` key order.
#[derive(Clone, Debug)]
pub struct Session {
    pub protocol: String,
    pub seed: u64,
    pub epoch_ms: f64,
    pub latency_ms: f64,
    pub end_ms: f64,
    pub client: Json,
    pub server: Json,
    pub server_steps: Vec<Step>,
    pub client_steps: Vec<Step>,
}

const PROTOCOLS: [(&str, &[&str]); 4] = [
    ("nt4", &["nt4", "nt4-publisher"]),
    ("rlog", &["rlog"]),
    ("phoenix", &["phoenix"]),
    ("ftc", &["ftc"]),
];

impl Session {
    pub fn parse(text: &str) -> Result<Self, String> {
        let json = js_order(json::parse(text).map_err(|e| e.to_string())?);
        let present = |key: &str| json.get(key).filter(|v| **v != Json::Null);
        let integer = |key: &str, default: Option<f64>, min: f64| -> Result<f64, String> {
            match present(key).map(Json::as_f64).unwrap_or(default) {
                Some(n) if msgpack::is_safe_integer(n) && n >= min => Ok(n),
                _ => Err(format!("{key} must be an integer >= {min}")),
            }
        };
        let protocol = present("protocol")
            .and_then(Json::as_str)
            .map(Utf16String::to_string_lossy)
            .unwrap_or_default();
        let sources = PROTOCOLS
            .iter()
            .find(|(p, _)| *p == protocol)
            .map(|(_, s)| *s)
            .ok_or_else(|| format!("unknown protocol {protocol:?}"))?;
        let seed = integer("seed", None, 0.0)? as u64;
        let epoch_ms = integer("epochMs", Some(1_700_000_000_000.0), 0.0)?;
        let latency_ms = integer("latencyMs", Some(1.0), 1.0)?;
        let end_ms = integer("endMs", None, 0.0)?;
        let client = present("client").cloned().ok_or("needs \"client\"")?;
        let source = client.get("source").and_then(Json::as_str);
        if !source.is_some_and(|s| sources.iter().any(|x| s.eq_str(x))) {
            return Err(format!("source {source:?} does not speak {protocol}"));
        }
        let mut session = Self {
            protocol,
            seed,
            epoch_ms,
            latency_ms,
            end_ms,
            client,
            server: present("server")
                .cloned()
                .unwrap_or(Json::Object(Vec::new())),
            server_steps: Vec::new(),
            client_steps: Vec::new(),
        };
        let mut last = 0.0;
        for (i, step) in present("steps")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let t = match step.get("t").and_then(Json::as_f64) {
                Some(t) if msgpack::is_safe_integer(t) && t >= 0.0 => t,
                _ => return Err(format!("steps[{i}].t must be an integer >= 0")),
            };
            if t < last || t > end_ms {
                return Err(format!(
                    "steps[{i}]: times must be non-decreasing and at most endMs"
                ));
            }
            last = t;
            let (server, client) = (
                step.get("server").and_then(Json::as_array),
                step.get("client").and_then(Json::as_array),
            );
            match (server, client) {
                (Some(op), None) => session.server_steps.push(Step { t, op: op.to_vec() }),
                (None, Some(op)) => session.client_steps.push(Step { t, op: op.to_vec() }),
                _ => {
                    return Err(format!(
                        "steps[{i}] needs exactly one of \"server\" and \"client\""
                    ));
                }
            }
        }
        Ok(session)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn server(&self) -> Result<MockServer, String> {
        let config = &self.server;
        let protocol: Box<dyn server::Protocol> = match self.protocol.as_str() {
            "nt4" => Box::new(nt4::Nt4::new(config, self.epoch_ms)?),
            "rlog" => Box::new(simple::Rlog::new(config)),
            "phoenix" => Box::new(simple::Phoenix::new(config)?),
            "ftc" => Box::new(simple::Ftc::new(config)?),
            other => return Err(format!("unknown protocol {other:?}")),
        };
        Ok(MockServer::new(protocol, self.epoch_ms))
    }
}

/// Every session file under `parity/specs/live`, sorted.
pub fn session_files() -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
            } else if path.to_string_lossy().ends_with(SESSION_SUFFIX) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(
        &vectors::workspace_root().join("parity/specs/live"),
        &mut out,
    );
    out.sort();
    out
}

/// The vector id of a session file: its path under `parity/specs` without the suffix.
pub fn vector_id(path: &Path) -> String {
    let specs = vectors::workspace_root().join("parity/specs");
    let rel = path.strip_prefix(&specs).unwrap_or(path);
    let rel = rel.to_string_lossy().replace('\\', "/");
    rel.strip_suffix(SESSION_SUFFIX).unwrap_or(&rel).to_owned()
}

enum Work {
    Arrival(ClientEvent),
    Step(Vec<Json>),
}

/// A mock server on the session clock, as `ServerHost` in `oracle/src/live/server.ts`: client
/// traffic arrives `latencyMs` after it is sent, and at each time the server takes every arrival
/// (in send order) before its steps for that time (in file order).
pub struct ServerHost {
    server: MockServer,
    latency_ms: f64,
    queue: Vec<(f64, u8, u64, Work)>,
    seq: u64,
}

impl ServerHost {
    pub fn new(session: &Session) -> Result<Self, String> {
        let mut host = Self {
            server: session.server()?,
            latency_ms: session.latency_ms,
            queue: Vec::new(),
            seq: 0,
        };
        for step in &session.server_steps {
            host.queue
                .push((step.t, 1, host.seq, Work::Step(step.op.clone())));
            host.seq += 1;
        }
        Ok(host)
    }

    /// Queues what the client sent at `sent_at`.
    pub fn arrive(&mut self, sent_at: f64, event: ClientEvent) {
        self.queue
            .push((sent_at + self.latency_ms, 0, self.seq, Work::Arrival(event)));
        self.seq += 1;
    }

    pub fn next_time(&self) -> Option<f64> {
        self.queue.iter().map(|q| q.0).reduce(f64::min)
    }

    /// Runs everything queued for `t`; each server event is delivered `latencyMs` later.
    pub fn process(&mut self, t: f64) -> Result<Vec<ServerEvent>, String> {
        let (mut due, rest): (Vec<_>, Vec<_>) = self.queue.drain(..).partition(|q| q.0 == t);
        self.queue = rest;
        due.sort_by_key(|q| (q.1, q.2));
        let mut out = Vec::new();
        for (_, _, _, work) in due {
            out.extend(match work {
                Work::Arrival(event) => self.server.client(t, &event)?,
                Work::Step(op) => self.server.step(t, &op)?,
            });
        }
        Ok(out)
    }
}

/// Replays a recorded trace's client traffic against the Rust mock and checks that it does
/// exactly what the TS mock did, line for line.
pub fn replay(session: &Session, records: &[Record]) -> Result<usize, String> {
    let mut host = ServerHost::new(session)?;
    let mut expected = Vec::new();
    for record in records {
        let event = TraceEvent::from_record(record)?;
        if event.is_server() {
            expected.push(event.line.clone());
        } else if let Some(client) = event.client_event()? {
            host.arrive(event.t, client);
        }
    }
    let mut produced = Vec::new();
    while let Some(t) = host.next_time().filter(|t| *t <= session.end_ms) {
        produced.extend(host.process(t)?.iter().map(|e| e.line(t)));
    }
    if let Some(i) =
        (0..expected.len().max(produced.len())).find(|&i| expected.get(i) != produced.get(i))
    {
        return Err(format!(
            "server event {i} differs\n  oracle: {}\n  rust:   {}",
            expected.get(i).map_or("(none)", String::as_str),
            produced.get(i).map_or("(none)", String::as_str)
        ));
    }
    Ok(produced.len())
}
