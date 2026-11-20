//! What every mock server shares, mirroring `oracle/src/live/server.ts`.

use crate::cve1::array_index;
use crate::json::Json;
use crate::utf16::Utf16String;

use super::msgpack::is_safe_integer;
use super::{ClientEvent, Payload, ServerEvent};

/// The port in an absolute `ws:`/`http:` URL, 80 when it names none, -1 when it is not digits.
pub fn url_port(url: &str) -> f64 {
    let rest = url.find("://").map_or(url, |i| &url[i + 3..]);
    let rest = rest.find(['/', '?', '#']).map_or(rest, |i| &rest[..i]);
    match rest.rfind(':') {
        None => 80.0,
        Some(i) => {
            let digits = &rest[i + 1..];
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                digits.parse::<f64>().unwrap_or(-1.0)
            } else {
                -1.0
            }
        }
    }
}

/// `decodeURIComponent`, or `None` where it would throw.
pub fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Query parameters in order; the first occurrence of a key wins and a malformed escape leaves
/// that parameter out.
pub fn url_query(url: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let Some(q) = url.find('?') else {
        return out;
    };
    for part in url[q + 1..].split('&') {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        if let (Some(k), Some(v)) = (decode_uri_component(k), decode_uri_component(v))
            && !out.iter().any(|(existing, _)| *existing == k)
        {
            out.push((k, v));
        }
    }
    out
}

pub fn query_get<'a>(query: &'a [(String, String)], key: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Puts every object's keys in the order `JSON.parse` gives them: array indices ascending, then
/// the rest in source order.
pub fn js_order(json: Json) -> Json {
    match json {
        Json::Array(items) => Json::Array(items.into_iter().map(js_order).collect()),
        Json::Object(pairs) => {
            let (mut indexed, named): (Vec<_>, Vec<_>) = pairs
                .into_iter()
                .map(|(k, v)| (array_index(&k), k, js_order(v)))
                .partition(|(i, _, _)| i.is_some());
            indexed.sort_by_key(|(i, _, _)| *i);
            Json::Object(
                indexed
                    .into_iter()
                    .chain(named)
                    .map(|(_, k, v)| (k, v))
                    .collect(),
            )
        }
        other => other,
    }
}

/// The same restriction as `checkJsonNumbers`: safe integers, and non-integers with magnitude in
/// [1e-6, 1e21), so `Json::to_compact` writes what `JSON.stringify` writes.
pub fn check_json_numbers(json: &Json, at: &str) -> Result<(), String> {
    match json {
        Json::Number(n) => {
            let ok = !(*n == 0.0 && n.is_sign_negative())
                && (is_safe_integer(*n)
                    || (n.is_finite() && n.trunc() != *n && n.abs() >= 1e-6 && n.abs() < 1e21));
            if ok {
                Ok(())
            } else {
                Err(format!(
                    "{at}: number {n} cannot be written identically by both mocks"
                ))
            }
        }
        Json::Array(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(i, item)| check_json_numbers(item, &format!("{at}[{i}]"))),
        Json::Object(pairs) => pairs
            .iter()
            .try_for_each(|(k, v)| check_json_numbers(v, &format!("{at}.{k}"))),
        _ => Ok(()),
    }
}

pub fn hex_to_bytes(json: &Json) -> Result<Vec<u8>, String> {
    let hex = json
        .as_str()
        .and_then(Utf16String::as_string)
        .ok_or_else(|| format!("not a hex string: {}", json.to_compact()))?;
    if hex.len() % 2 != 0 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(format!("not lowercase hex: {hex:?}"));
    }
    Ok((0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("checked hex"))
        .collect())
}

/// `String(value)` for the scalar JSON values a session puts where upstream compares strings.
pub fn js_string(json: &Json) -> String {
    match json {
        Json::String(s) => s.to_string_lossy(),
        other => other.to_compact(),
    }
}

#[derive(Default)]
pub struct Base {
    up: bool,
    down_mode: DownMode,
    muted: bool,
    ws: Vec<(u32, String)>,
    tcp: Vec<u32>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum DownMode {
    #[default]
    Refuse,
    Hang,
}

/// The server's view while handling one event: the time, its connections and its outbox.
pub struct Cx<'a> {
    pub t: f64,
    pub epoch_ms: f64,
    base: &'a mut Base,
    out: &'a mut Vec<ServerEvent>,
}

impl Cx<'_> {
    pub fn ws_ids(&self) -> Vec<u32> {
        self.base.ws.iter().map(|(id, _)| *id).collect()
    }

    pub fn ws_open(&self, id: u32) -> bool {
        self.base.ws.iter().any(|(i, _)| *i == id)
    }

    pub fn ws_protocol(&self, id: u32) -> Option<&str> {
        self.base
            .ws
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, p)| p.as_str())
    }

    pub fn tcp_ids(&self) -> Vec<u32> {
        self.base.tcp.clone()
    }

    pub fn push_ws(&mut self, id: u32, data: Payload) {
        if !self.base.muted && self.ws_open(id) {
            self.out.push(ServerEvent::WsPush { id, data });
        }
    }

    pub fn push_tcp(&mut self, id: u32, data: Vec<u8>) {
        if !self.base.muted && self.base.tcp.contains(&id) {
            self.out.push(ServerEvent::TcpPush { id, data });
        }
    }
}

pub trait Protocol {
    fn ports(&self) -> Vec<f64>;
    fn accept_ws(&mut self, _url: &str, _protocols: &[String]) -> Option<String> {
        None
    }
    fn ws_opened(&mut self, _cx: &mut Cx<'_>, _id: u32) {}
    fn ws_message(&mut self, _cx: &mut Cx<'_>, _id: u32, _data: &Payload) -> Result<(), String> {
        Ok(())
    }
    fn http(&mut self, _cx: &mut Cx<'_>, _url: &str) -> Result<Option<(f64, String)>, String> {
        Ok(None)
    }
    fn tcp_data(&mut self, _cx: &mut Cx<'_>, _id: u32, _data: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn step(&mut self, cx: &mut Cx<'_>, op: &[Json]) -> Result<(), String>;
}

pub struct MockServer {
    base: Base,
    epoch_ms: f64,
    protocol: Box<dyn Protocol>,
}

impl MockServer {
    pub fn new(protocol: Box<dyn Protocol>, epoch_ms: f64) -> Self {
        Self {
            base: Base {
                up: true,
                ..Base::default()
            },
            epoch_ms,
            protocol,
        }
    }

    /// Whether a new connection to `port` gets an answer, after refusing it when it must.
    fn answers(&self, port: f64, out: &mut Vec<ServerEvent>, refuse: ServerEvent) -> bool {
        if !self.protocol.ports().contains(&port) {
            out.push(refuse);
            return false;
        }
        if !self.base.up {
            if self.base.down_mode == DownMode::Refuse {
                out.push(refuse);
            }
            return false;
        }
        true
    }

    pub fn client(&mut self, t: f64, event: &ClientEvent) -> Result<Vec<ServerEvent>, String> {
        let mut out = Vec::new();
        let epoch_ms = self.epoch_ms;
        match event {
            ClientEvent::WsOpen { id, url, protocols } => {
                let id = *id;
                if self.answers(url_port(url), &mut out, ServerEvent::WsReject { id }) {
                    match self.protocol.accept_ws(url, protocols) {
                        None => out.push(ServerEvent::WsReject { id }),
                        Some(protocol) => {
                            self.base.ws.push((id, protocol.clone()));
                            out.push(ServerEvent::WsAccept { id, protocol });
                            let mut cx = Cx {
                                t,
                                epoch_ms,
                                base: &mut self.base,
                                out: &mut out,
                            };
                            self.protocol.ws_opened(&mut cx, id);
                        }
                    }
                }
            }
            ClientEvent::WsSend { id, data } => {
                if self.base.ws.iter().any(|(i, _)| i == id) {
                    let mut cx = Cx {
                        t,
                        epoch_ms,
                        base: &mut self.base,
                        out: &mut out,
                    };
                    self.protocol.ws_message(&mut cx, *id, data)?;
                }
            }
            ClientEvent::WsClose { id } => self.base.ws.retain(|(i, _)| i != id),
            ClientEvent::HttpGet { id, url } => {
                let id = *id;
                if self.answers(url_port(url), &mut out, ServerEvent::HttpRefuse { id }) {
                    let mut cx = Cx {
                        t,
                        epoch_ms,
                        base: &mut self.base,
                        out: &mut out,
                    };
                    match self.protocol.http(&mut cx, url)? {
                        None => out.push(ServerEvent::HttpRefuse { id }),
                        Some((status, body)) => {
                            out.push(ServerEvent::HttpReply { id, status, body })
                        }
                    }
                }
            }
            ClientEvent::TcpSend { id, data } => {
                if self.base.tcp.contains(id) {
                    let mut cx = Cx {
                        t,
                        epoch_ms,
                        base: &mut self.base,
                        out: &mut out,
                    };
                    self.protocol.tcp_data(&mut cx, *id, data)?;
                }
            }
            ClientEvent::HttpAbort { .. } => {}
            ClientEvent::TcpOpen { id, port, .. } => {
                let id = *id;
                if self.answers(*port, &mut out, ServerEvent::TcpRefuse { id }) {
                    self.base.tcp.push(id);
                    out.push(ServerEvent::TcpAccept { id });
                }
            }
            ClientEvent::TcpClose { id } => self.base.tcp.retain(|i| i != id),
        }
        Ok(out)
    }

    pub fn step(&mut self, t: f64, op: &[Json]) -> Result<Vec<ServerEvent>, String> {
        let mut out = Vec::new();
        let name = op
            .first()
            .and_then(Json::as_str)
            .map(Utf16String::to_string_lossy)
            .unwrap_or_default();
        match name.as_str() {
            "up" => self.base.up = true,
            "down" => {
                self.base.down_mode = match op.get(1).and_then(Json::as_str) {
                    Some(m) if m.eq_str("refuse") => DownMode::Refuse,
                    Some(m) if m.eq_str("hang") => DownMode::Hang,
                    _ => return Err("down: mode must be \"refuse\" or \"hang\"".to_owned()),
                };
                self.base.up = false;
            }
            "mute" => self.base.muted = op.get(1) == Some(&Json::Bool(true)),
            "drop" => {
                out.extend(
                    self.base
                        .ws
                        .drain(..)
                        .map(|(id, _)| ServerEvent::WsDrop { id }),
                );
                out.extend(
                    self.base
                        .tcp
                        .drain(..)
                        .map(|id| ServerEvent::TcpDrop { id }),
                );
            }
            _ => {
                let mut cx = Cx {
                    t,
                    epoch_ms: self.epoch_ms,
                    base: &mut self.base,
                    out: &mut out,
                };
                self.protocol.step(&mut cx, op)?;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn ports_come_from_the_authority() {
        assert_eq!(url_port("ws://127.0.0.1:5810/nt/AdvantageScope"), 5810.0);
        assert_eq!(url_port("http://10.0.0.2:1250/?action=getdevices"), 1250.0);
        assert_eq!(url_port("http://host/path:9"), 80.0);
        assert_eq!(url_port("ws://host:x1"), -1.0);
    }

    #[test]
    fn queries_decode_like_decode_uri_component() {
        let q =
            url_query("http://h:1/?model=Talon%20FX&id=1&model=again&bad=%E0%A4&plus=a+b&pct=%+1");
        let keys: Vec<(&str, &str)> = q.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(keys, [("model", "Talon FX"), ("id", "1"), ("plus", "a+b")]);
        assert_eq!(decode_uri_component("%C3%A9"), Some("é".to_owned()));
        assert_eq!(decode_uri_component("%C0%80"), None);
    }

    #[test]
    fn objects_take_json_parse_order() {
        let parsed =
            js_order(json::parse(r#"{"b":1,"10":2,"a":{"2":3,"x":4,"1":5},"01":6}"#).unwrap());
        assert_eq!(
            parsed.to_compact(),
            r#"{"10":2,"b":1,"a":{"1":5,"2":3,"x":4},"01":6}"#
        );
    }

    #[test]
    fn numbers_must_print_the_same_in_js_and_rust() {
        for ok in ["0", "9007199254740991", "0.000001", "-2.5", "123456.789"] {
            assert!(
                check_json_numbers(&json::parse(ok).unwrap(), "n").is_ok(),
                "{ok}"
            );
        }
        for bad in [
            "-0",
            "9007199254740992",
            "1152921504606846976",
            "1e-7",
            "1e21",
            "[1,{\"a\":1e300}]",
        ] {
            assert!(
                check_json_numbers(&json::parse(bad).unwrap(), "n").is_err(),
                "{bad}"
            );
        }
    }
}
