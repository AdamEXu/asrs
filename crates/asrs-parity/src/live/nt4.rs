//! The NT4 mock, mirroring `oracle/src/live/nt4Server.ts` (which documents its behaviour).

use crate::json::{self, Json};
use crate::utf16::Utf16String;

use super::Payload;
use super::msgpack::{Packed, Unpacked, decode_all, encode, is_safe_integer, number};
use super::server::{Cx, Protocol, check_json_numbers, hex_to_bytes, js_order};

const V41: &str = "v4.1.networktables.first.wpi.edu";
const V40: &str = "networktables.first.wpi.edu";
const RTT: &str = "rtt.networktables.first.wpi.edu";

fn type_index(ty: &str) -> f64 {
    match ty {
        "boolean" => 0.0,
        "double" => 1.0,
        "int" => 2.0,
        "float" => 3.0,
        "string" | "json" => 4.0,
        "boolean[]" => 16.0,
        "double[]" => 17.0,
        "int[]" => 18.0,
        "float[]" => 19.0,
        "string[]" => 20.0,
        _ => 5.0,
    }
}

type Properties = Vec<(Utf16String, Json)>;

struct Topic {
    id: f64,
    name: Utf16String,
    ty: String,
    properties: Properties,
    value: Option<(f64, Packed)>,
}

struct Subscription {
    topics: Vec<Utf16String>,
    prefix: bool,
    topics_only: bool,
}

struct Connection {
    id: u32,
    rtt: bool,
    subs: Vec<(u64, Subscription)>,
    announced: Vec<Utf16String>,
}

pub struct Nt4 {
    v41: bool,
    ports: Vec<f64>,
    offset_us: f64,
    http_status: f64,
    topics: Vec<Topic>,
    next_topic_id: f64,
    conns: Vec<Connection>,
}

fn string(json: &Json) -> Option<String> {
    json.as_str().map(Utf16String::to_string_lossy)
}

fn object(json: Option<&Json>) -> Option<&Properties> {
    match json {
        Some(Json::Object(pairs)) => Some(pairs),
        _ => None,
    }
}

fn map_key(n: f64) -> u64 {
    if n == 0.0 { 0.0f64 } else { n }.to_bits()
}

fn pack_value(ty: &str, value: &Json, at: &str) -> Result<Packed, String> {
    let bad = || format!("{at}: {} is not a {ty} value", value.to_compact());
    let list = |item: &str| -> Result<Packed, String> {
        let items = value.as_array().ok_or_else(bad)?;
        Ok(Packed::Arr(
            items
                .iter()
                .map(|v| pack_value(item, v, at))
                .collect::<Result<_, _>>()?,
        ))
    };
    match ty {
        "boolean" => match value {
            Json::Bool(b) => Ok(Packed::Bool(*b)),
            _ => Err(bad()),
        },
        "double" => value.as_f64().map(Packed::F64).ok_or_else(bad),
        "float" => value.as_f64().map(Packed::F32).ok_or_else(bad),
        "int" => value
            .as_f64()
            .filter(|n| is_safe_integer(*n))
            .map(Packed::Int)
            .ok_or_else(bad),
        "string" | "json" => value
            .as_str()
            .map(|s| Packed::Str(String::from_utf16_lossy(s.units())))
            .ok_or_else(bad),
        "boolean[]" => list("boolean"),
        "double[]" => list("double"),
        "float[]" => list("float"),
        "int[]" => list("int"),
        "string[]" => list("string"),
        _ => {
            value.as_str().ok_or_else(bad)?;
            Ok(Packed::Bin(hex_to_bytes(value)?))
        }
    }
}

/// `binaryPieces`: hex, or a list of hex strings and `[hex, count]` repeats.
fn binary_pieces(spec: &Json) -> Result<Vec<u8>, String> {
    let Some(pieces) = spec.as_array() else {
        return hex_to_bytes(spec);
    };
    let mut out = Vec::new();
    for piece in pieces {
        match piece.as_array() {
            None => out.extend(hex_to_bytes(piece)?),
            Some([hex, Json::Number(count)])
                if hex.as_str().is_some() && is_safe_integer(*count) && *count >= 0.0 =>
            {
                let bytes = hex_to_bytes(hex)?;
                for _ in 0..*count as u64 {
                    out.extend_from_slice(&bytes);
                }
            }
            Some(_) => return Err(format!("nt4: bad binary piece {}", piece.to_compact())),
        }
    }
    Ok(out)
}

impl Nt4 {
    pub fn new(config: &Json, epoch_ms: f64) -> Result<Self, String> {
        let present = |key: &str| config.get(key).filter(|v| **v != Json::Null);
        let v41 = match present("version").map(string) {
            None => true,
            Some(Some(v)) if v == "4.1" => true,
            Some(Some(v)) if v == "4.0" => false,
            Some(v) => return Err(format!("nt4: unknown version {v:?}")),
        };
        let ports = match present("ports") {
            None => vec![5810.0],
            Some(p) => p
                .as_array()
                .and_then(|ports| ports.iter().map(Json::as_f64).collect::<Option<Vec<_>>>())
                .ok_or("nt4: ports must be a list of numbers")?,
        };
        let offset_us = present("offsetUs").map_or(Some(0.0), Json::as_f64);
        let offset_us = offset_us
            .filter(|n| is_safe_integer(*n))
            .ok_or("nt4: offsetUs must be a safe integer")?;
        let mut nt4 = Self {
            v41,
            ports,
            offset_us,
            http_status: 200.0,
            topics: Vec::new(),
            next_topic_id: 1.0,
            conns: Vec::new(),
        };
        let empty = Json::Object(Vec::new());
        for t in present("topics")
            .and_then(Json::as_array)
            .unwrap_or_default()
        {
            let properties = t
                .get("properties")
                .filter(|p| **p != Json::Null)
                .unwrap_or(&empty);
            let index = nt4.add_topic(t.get("name"), t.get("type"), properties)?;
            if let Some(value) = t.get("value") {
                let ts = nt4.server_time_us(epoch_ms, 0.0);
                let topic = &mut nt4.topics[index];
                let packed = pack_value(&topic.ty, value, &topic.name.to_string_lossy())?;
                topic.value = Some((ts, packed));
            }
        }
        Ok(nt4)
    }

    fn server_time_us(&self, epoch_ms: f64, t: f64) -> f64 {
        (epoch_ms + t) * 1000.0 + self.offset_us
    }

    fn add_topic(
        &mut self,
        name: Option<&Json>,
        ty: Option<&Json>,
        properties: &Json,
    ) -> Result<usize, String> {
        let (Some(Json::String(name)), Some(Json::String(ty))) = (name, ty) else {
            return Err("nt4: topic needs a name and a type".to_owned());
        };
        if self.find(name).is_some() {
            return Err(format!("nt4: topic {name} exists"));
        }
        let Json::Object(properties) = properties else {
            return Err(format!("nt4: topic {name} properties must be an object"));
        };
        check_json_numbers(
            &Json::Object(properties.clone()),
            &format!("nt4 topic {name} properties"),
        )?;
        self.topics.push(Topic {
            id: self.next_topic_id,
            name: name.clone(),
            ty: ty.to_string_lossy(),
            properties: properties.clone(),
            value: None,
        });
        self.next_topic_id += 1.0;
        Ok(self.topics.len() - 1)
    }

    fn find(&self, name: &Utf16String) -> Option<usize> {
        self.topics.iter().position(|t| t.name == *name)
    }

    fn known(&self, name: Option<&Json>) -> Result<usize, String> {
        name.and_then(Json::as_str)
            .and_then(|n| self.find(n))
            .ok_or_else(|| {
                format!(
                    "nt4: unknown topic {}",
                    name.map_or("undefined".to_owned(), Json::to_compact)
                )
            })
    }

    fn main_conns(&self, cx: &Cx<'_>) -> Vec<usize> {
        (0..self.conns.len())
            .filter(|&i| !self.conns[i].rtt && cx.ws_open(self.conns[i].id))
            .collect()
    }

    fn matches(sub: &Subscription, name: &Utf16String) -> bool {
        sub.topics.iter().any(|p| {
            if sub.prefix {
                name.units().starts_with(p.units())
            } else {
                name == p
            }
        })
    }

    fn wants_topic(conn: &Connection, name: &Utf16String) -> bool {
        conn.subs.iter().any(|(_, s)| Self::matches(s, name))
    }

    fn wants_value(conn: &Connection, name: &Utf16String) -> bool {
        conn.subs
            .iter()
            .any(|(_, s)| !s.topics_only && Self::matches(s, name))
    }

    fn announce_message(topic: &Topic) -> Json {
        Json::object([
            ("method", Json::string("announce")),
            (
                "params",
                Json::object([
                    ("name", Json::String(topic.name.clone())),
                    ("id", Json::Number(topic.id)),
                    ("type", Json::string(&topic.ty)),
                    ("properties", Json::Object(topic.properties.clone())),
                ]),
            ),
        ])
    }

    fn send_json(cx: &mut Cx<'_>, id: u32, messages: Vec<Json>) {
        if !messages.is_empty() {
            cx.push_ws(id, Payload::Text(Json::Array(messages).to_compact()));
        }
    }

    fn value_array(topic: &Topic, ts: f64, packed: &Packed) -> Packed {
        Packed::Arr(vec![
            Packed::Int(topic.id),
            number(ts),
            Packed::Int(type_index(&topic.ty)),
            packed.clone(),
        ])
    }

    fn announce_to_all(&mut self, cx: &mut Cx<'_>, topic: usize) {
        for i in self.main_conns(cx) {
            let name = self.topics[topic].name.clone();
            let conn = &mut self.conns[i];
            if !Self::wants_topic(conn, &name) {
                continue;
            }
            if !conn.announced.contains(&name) {
                conn.announced.push(name);
            }
            let id = conn.id;
            Self::send_json(cx, id, vec![Self::announce_message(&self.topics[topic])]);
        }
    }

    fn send_values(&self, cx: &mut Cx<'_>, updates: &[(usize, f64, Packed)]) {
        for i in self.main_conns(cx) {
            let conn = &self.conns[i];
            let arrays: Vec<Packed> = updates
                .iter()
                .filter(|(t, _, _)| Self::wants_value(conn, &self.topics[*t].name))
                .map(|(t, ts, p)| Self::value_array(&self.topics[*t], *ts, p))
                .collect();
            if !arrays.is_empty() {
                cx.push_ws(conn.id, Payload::Binary(encode(&arrays)));
            }
        }
    }

    fn update_properties(
        &mut self,
        cx: &mut Cx<'_>,
        topic: usize,
        update: &Properties,
    ) -> Result<(), String> {
        check_json_numbers(
            &Json::Object(update.clone()),
            &format!("nt4 topic {} properties", self.topics[topic].name),
        )?;
        let properties = &mut self.topics[topic].properties;
        for (key, value) in update {
            if *value == Json::Null {
                properties.retain(|(k, _)| k != key);
            } else if let Some(slot) = properties.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value.clone();
            } else {
                properties.push((key.clone(), value.clone()));
            }
        }
        if let Json::Object(ordered) = js_order(Json::Object(std::mem::take(properties))) {
            *properties = ordered;
        }
        let name = self.topics[topic].name.clone();
        for i in self.main_conns(cx) {
            if self.conns[i].announced.contains(&name) {
                let message = Json::object([
                    ("method", Json::string("properties")),
                    (
                        "params",
                        Json::object([
                            ("name", Json::String(name.clone())),
                            ("update", Json::Object(update.clone())),
                        ]),
                    ),
                ]);
                Self::send_json(cx, self.conns[i].id, vec![message]);
            }
        }
        Ok(())
    }

    fn subscribe(&mut self, cx: &mut Cx<'_>, conn: usize, params: &Properties) {
        let get = |key: &str| params.iter().find(|(k, _)| k.eq_str(key)).map(|(_, v)| v);
        let (Some(Json::Number(subuid)), Some(Json::Array(topics))) =
            (get("subuid"), get("topics"))
        else {
            return;
        };
        let options = object(get("options"));
        let flag = |key: &str| {
            options.is_some_and(|o| {
                o.iter()
                    .any(|(k, v)| k.eq_str(key) && *v == Json::Bool(true))
            })
        };
        let sub = Subscription {
            topics: topics.iter().filter_map(|t| t.as_str().cloned()).collect(),
            prefix: flag("prefix"),
            topics_only: flag("topicsonly"),
        };
        let matched: Vec<usize> = (0..self.topics.len())
            .filter(|&t| Self::matches(&sub, &self.topics[t].name))
            .collect();
        let topics_only = sub.topics_only;
        let c = &mut self.conns[conn];
        let key = map_key(*subuid);
        match c.subs.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = sub,
            None => c.subs.push((key, sub)),
        }
        let fresh: Vec<usize> = matched
            .iter()
            .copied()
            .filter(|&t| !c.announced.contains(&self.topics[t].name))
            .collect();
        c.announced
            .extend(fresh.iter().map(|&t| self.topics[t].name.clone()));
        let id = c.id;
        Self::send_json(
            cx,
            id,
            fresh
                .iter()
                .map(|&t| Self::announce_message(&self.topics[t]))
                .collect(),
        );
        if !topics_only {
            let arrays: Vec<Packed> = matched
                .iter()
                .filter_map(|&t| {
                    let topic = &self.topics[t];
                    topic
                        .value
                        .as_ref()
                        .map(|(ts, p)| Self::value_array(topic, *ts, p))
                })
                .collect();
            if !arrays.is_empty() {
                cx.push_ws(id, Payload::Binary(encode(&arrays)));
            }
        }
    }

    fn update(
        &mut self,
        cx: &Cx<'_>,
        name: Option<&Json>,
        value: Option<&Json>,
        ts_us: Option<&Json>,
        raw: bool,
    ) -> Result<(usize, f64, Packed), String> {
        let topic = self.known(name)?;
        let ts = match ts_us {
            None => self.server_time_us(cx.epoch_ms, cx.t),
            Some(Json::Number(n)) if is_safe_integer(*n) => *n,
            Some(_) => return Err("nt4: tsUs must be a safe integer".to_owned()),
        };
        let value = value.unwrap_or(&Json::Null);
        let packed = if raw {
            Packed::Raw(hex_to_bytes(value)?)
        } else {
            let t = &self.topics[topic];
            pack_value(&t.ty, value, &t.name.to_string_lossy())?
        };
        self.topics[topic].value = Some((ts, packed.clone()));
        Ok((topic, ts, packed))
    }
}

impl Protocol for Nt4 {
    fn ports(&self) -> Vec<f64> {
        self.ports.clone()
    }

    fn http(&mut self, _cx: &mut Cx<'_>, _url: &str) -> Result<Option<(f64, String)>, String> {
        Ok(Some((self.http_status, String::new())))
    }

    fn accept_ws(&mut self, _url: &str, protocols: &[String]) -> Option<String> {
        let offered: &[&str] = if self.v41 { &[V41, V40, RTT] } else { &[V40] };
        offered
            .iter()
            .find(|p| protocols.iter().any(|o| o == *p))
            .map(|p| (*p).to_owned())
    }

    fn ws_opened(&mut self, cx: &mut Cx<'_>, id: u32) {
        self.conns.push(Connection {
            id,
            rtt: cx.ws_protocol(id) == Some(RTT),
            subs: Vec::new(),
            announced: Vec::new(),
        });
    }

    fn ws_message(&mut self, cx: &mut Cx<'_>, id: u32, data: &Payload) -> Result<(), String> {
        let Some(conn) = self.conns.iter().position(|c| c.id == id) else {
            return Ok(());
        };
        let text = match data {
            Payload::Binary(bytes) => {
                for item in decode_all(bytes) {
                    let Unpacked::Arr(items) = item else { continue };
                    if let (Some(Unpacked::Number(first)), Some(Unpacked::Number(client))) =
                        (items.first(), items.get(3))
                        && *first == -1.0
                    {
                        let reply = Packed::Arr(vec![
                            Packed::Int(-1.0),
                            number(self.server_time_us(cx.epoch_ms, cx.t)),
                            Packed::Int(2.0),
                            number(*client),
                        ]);
                        cx.push_ws(id, Payload::Binary(encode(&[reply])));
                    }
                }
                return Ok(());
            }
            Payload::Text(text) => text,
        };
        if self.conns[conn].rtt {
            return Ok(());
        }
        let Ok(Json::Array(messages)) = json::parse(text).map(js_order) else {
            return Ok(());
        };
        for message in &messages {
            let Some(params) = object(message.get("params")) else {
                continue;
            };
            let method = message.get("method").and_then(string);
            let get = |key: &str| params.iter().find(|(k, _)| k.eq_str(key)).map(|(_, v)| v);
            match method.as_deref() {
                Some("subscribe") => self.subscribe(cx, conn, params),
                Some("unsubscribe") => {
                    if let Some(Json::Number(subuid)) = get("subuid") {
                        let key = map_key(*subuid);
                        self.conns[conn].subs.retain(|(k, _)| *k != key);
                    }
                }
                Some("publish") => {
                    if let (Some(Json::String(name)), Some(Json::String(_))) =
                        (get("name"), get("type"))
                        && self.find(name).is_none()
                    {
                        let properties = match get("properties") {
                            Some(p @ Json::Object(_)) => p.clone(),
                            _ => Json::Object(Vec::new()),
                        };
                        let topic = self.add_topic(get("name"), get("type"), &properties)?;
                        self.announce_to_all(cx, topic);
                    }
                }
                Some("setproperties") => {
                    let topic = get("name")
                        .and_then(Json::as_str)
                        .and_then(|n| self.find(n));
                    if let (Some(topic), Some(Json::Object(update))) = (topic, get("update")) {
                        let update = update.clone();
                        self.update_properties(cx, topic, &update)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn step(&mut self, cx: &mut Cx<'_>, op: &[Json]) -> Result<(), String> {
        let name = op.first().and_then(string).unwrap_or_default();
        let empty = Json::Object(Vec::new());
        match name.as_str() {
            "announce" => {
                let properties = op.get(3).filter(|p| **p != Json::Null).unwrap_or(&empty);
                let topic = self.add_topic(op.get(1), op.get(2), properties)?;
                self.announce_to_all(cx, topic);
            }
            "unannounce" => {
                let topic = self.known(op.get(1))?;
                let removed = self.topics.remove(topic);
                for i in self.main_conns(cx) {
                    let conn = &mut self.conns[i];
                    let Some(at) = conn.announced.iter().position(|n| *n == removed.name) else {
                        continue;
                    };
                    conn.announced.remove(at);
                    let message = Json::object([
                        ("method", Json::string("unannounce")),
                        (
                            "params",
                            Json::object([
                                ("name", Json::String(removed.name.clone())),
                                ("id", Json::Number(removed.id)),
                            ]),
                        ),
                    ]);
                    let id = conn.id;
                    Self::send_json(cx, id, vec![message]);
                }
            }
            "properties" => {
                let Some(Json::Object(update)) = op.get(2) else {
                    return Err("nt4: properties needs an update object".to_owned());
                };
                let topic = self.known(op.get(1))?;
                self.update_properties(cx, topic, update)?;
            }
            "set" | "setraw" => {
                let update = self.update(cx, op.get(1), op.get(2), op.get(3), name == "setraw")?;
                self.send_values(cx, &[update]);
            }
            "batch" => {
                let items = op
                    .get(1)
                    .and_then(Json::as_array)
                    .ok_or("nt4: batch needs a list")?;
                let mut updates = Vec::new();
                for item in items {
                    let parts = item.as_array().ok_or("nt4: batch items are lists")?;
                    updates.push(self.update(
                        cx,
                        parts.first(),
                        parts.get(1),
                        parts.get(2),
                        false,
                    )?);
                }
                self.send_values(cx, &updates);
            }
            "text" => {
                let value = op.get(1).ok_or("nt4: text needs a value")?;
                check_json_numbers(value, "nt4 text")?;
                for i in self.main_conns(cx) {
                    cx.push_ws(self.conns[i].id, Payload::Text(value.to_compact()));
                }
            }
            "httpStatus" => {
                self.http_status = op
                    .get(1)
                    .and_then(Json::as_f64)
                    .filter(|n| is_safe_integer(*n))
                    .ok_or("nt4: httpStatus needs an integer")?;
            }
            "textraw" => {
                let text = op
                    .get(1)
                    .and_then(Json::as_str)
                    .ok_or("nt4: textraw needs a string")?
                    .to_string_lossy();
                for i in self.main_conns(cx) {
                    cx.push_ws(self.conns[i].id, Payload::Text(text.clone()));
                }
            }
            "binary" => {
                let bytes = binary_pieces(op.get(1).unwrap_or(&Json::Null))?;
                for i in self.main_conns(cx) {
                    cx.push_ws(self.conns[i].id, Payload::Binary(bytes.clone()));
                }
            }
            other => return Err(format!("nt4: unknown step {other:?}")),
        }
        Ok(())
    }
}
