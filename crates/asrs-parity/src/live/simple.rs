//! The RLOG, Phoenix and FTC Dashboard mocks, mirroring `oracle/src/live/simpleServers.ts`
//! (which documents their behaviour).

use crate::json::{self, Json};
use crate::utf16::Utf16String;

use super::Payload;
use super::msgpack::is_safe_integer;
use super::server::{
    Cx, Protocol, check_json_numbers, hex_to_bytes, js_order, js_string, query_get, url_query,
};

fn op_name(op: &[Json]) -> String {
    op.first()
        .and_then(Json::as_str)
        .map(Utf16String::to_string_lossy)
        .unwrap_or_default()
}

fn present<'a>(config: &'a Json, key: &str) -> Option<&'a Json> {
    config.get(key).filter(|v| **v != Json::Null)
}

pub struct Rlog {
    port: f64,
}

impl Rlog {
    pub fn new(config: &Json) -> Self {
        Self {
            port: present(config, "port")
                .and_then(Json::as_f64)
                .unwrap_or(5800.0),
        }
    }
}

impl Protocol for Rlog {
    fn ports(&self) -> Vec<f64> {
        vec![self.port]
    }

    fn step(&mut self, cx: &mut Cx<'_>, op: &[Json]) -> Result<(), String> {
        let hex = op.get(1).unwrap_or(&Json::Null);
        let bytes = match op_name(op).as_str() {
            "send" => {
                let payload = hex_to_bytes(hex)?;
                let mut bytes = (payload.len() as i32).to_be_bytes().to_vec();
                bytes.extend(payload);
                bytes
            }
            "sendraw" => hex_to_bytes(hex)?,
            other => return Err(format!("rlog: unknown step {other:?}")),
        };
        for id in cx.tcp_ids() {
            cx.push_tcp(id, bytes.clone());
        }
        Ok(())
    }
}

/// Seconds with exactly six decimals, from integer microseconds, without float formatting.
pub fn seconds_text(us: f64) -> Result<String, String> {
    if !is_safe_integer(us) || us < 0.0 {
        return Err(format!("phoenix: bad timestamp {us}"));
    }
    let us = us as u64;
    Ok(format!("{}.{:06}", us / 1_000_000, us % 1_000_000))
}

struct Device {
    device: Json,
    signals: Vec<Json>,
    values: Vec<(Utf16String, Json)>,
}

pub struct Phoenix {
    devices: Vec<Device>,
    bus_util: Json,
    start_us: f64,
    points: f64,
    step_us: f64,
    errors: Vec<(String, f64)>,
}

fn strict_eq(json: Option<&Json>, s: Option<&str>) -> bool {
    matches!((json, s), (Some(Json::String(a)), Some(b)) if a.eq_str(b))
}

fn string_of(json: Option<&Json>) -> String {
    json.map_or_else(|| "undefined".to_owned(), js_string)
}

impl Phoenix {
    pub fn new(config: &Json) -> Result<Self, String> {
        let int = |key: &str, default: f64| -> Result<f64, String> {
            match present(config, key) {
                None => Ok(default),
                Some(Json::Number(n)) if is_safe_integer(*n) && *n >= 0.0 => Ok(*n),
                Some(_) => Err(
                    "phoenix: startUs, points and stepUs must be safe non-negative integers"
                        .to_owned(),
                ),
            }
        };
        let bus_util = present(config, "busUtil")
            .cloned()
            .unwrap_or(Json::Number(0.0));
        check_json_numbers(&bus_util, "phoenix busUtil")?;
        let phoenix = Self {
            devices: Self::devices(present(config, "devices"))?,
            bus_util,
            start_us: int("startUs", 0.0)?,
            points: int("points", 2.0)?,
            step_us: int("stepUs", 1000.0)?,
            errors: Vec::new(),
        };
        if phoenix.step_us == 0.0 {
            return Err("phoenix: stepUs must be positive".to_owned());
        }
        Ok(phoenix)
    }

    fn devices(list: Option<&Json>) -> Result<Vec<Device>, String> {
        let Some(list) = list else {
            return Ok(Vec::new());
        };
        check_json_numbers(list, "phoenix devices")?;
        let items = list.as_array().ok_or("phoenix: devices must be a list")?;
        items
            .iter()
            .map(|d| {
                let values = match present(d, "values") {
                    None => Vec::new(),
                    Some(Json::Object(pairs)) => pairs.clone(),
                    Some(_) => return Err("phoenix: values must be an object".to_owned()),
                };
                if values.iter().any(|(_, v)| v.as_array().is_none()) {
                    return Err("phoenix: values must map ids to arrays".to_owned());
                }
                Ok(Device {
                    device: d.get("device").cloned().ok_or("phoenix: device missing")?,
                    signals: present(d, "signals")
                        .and_then(Json::as_array)
                        .unwrap_or_default()
                        .to_vec(),
                    values,
                })
            })
            .collect()
    }

    fn general(&self, action: &str) -> String {
        let code = self
            .errors
            .iter()
            .find(|(a, _)| a == action)
            .map_or(0.0, |(_, c)| *c);
        format!(
            "{{\"GeneralReturn\":{{\"Error\":{},\"ErrorMessage\":\"\"}}",
            Json::Number(code).to_compact()
        )
    }

    fn find(&self, query: &[(String, String)]) -> Option<&Device> {
        self.devices.iter().find(|d| {
            strict_eq(d.device.get("Model"), query_get(query, "model"))
                && Some(string_of(d.device.get("ID")).as_str()) == query_get(query, "id")
                && strict_eq(d.device.get("CANbus"), query_get(query, "canbus"))
        })
    }

    fn plot(&self, t: f64, device: &Device, query: &[(String, String)]) -> Result<String, String> {
        let ids: Vec<&str> = query_get(query, "signals")
            .unwrap_or("")
            .split(',')
            .collect();
        let known: Vec<&str> = ids
            .iter()
            .enumerate()
            .filter(|(i, id)| {
                ids.iter().position(|x| x == *id) == Some(*i)
                    && device
                        .signals
                        .iter()
                        .any(|s| string_of(s.get("Id")) == **id)
            })
            .map(|(_, id)| *id)
            .collect();
        let now_us = self.start_us + t * 1000.0;
        let mut points = Vec::new();
        let mut k = self.points - 1.0;
        while k >= 0.0 {
            let us = now_us - k * self.step_us;
            k -= 1.0;
            if us < 0.0 {
                continue;
            }
            let ordinal = (us / self.step_us).floor();
            let signals: Vec<String> = known
                .iter()
                .map(|id| {
                    let list = device
                        .values
                        .iter()
                        .find(|(k, _)| k.eq_str(id))
                        .and_then(|(_, v)| v.as_array())
                        .unwrap_or_default();
                    let value = if list.is_empty() {
                        Json::Number(0.0)
                    } else {
                        list[(ordinal % list.len() as f64) as usize].clone()
                    };
                    format!("{}:{}", Json::string(id).to_compact(), value.to_compact())
                })
                .collect();
            points.push(format!(
                "{{\"Ordinal\":{},\"Timestamp\":{},\"Signals\":{{{}}}}}",
                Json::Number(ordinal).to_compact(),
                seconds_text(us)?,
                signals.join(",")
            ));
        }
        Ok(format!(
            "{},\"Count\":{},\"Points\":[{}]}}",
            self.general("plotpro"),
            points.len(),
            points.join(",")
        ))
    }
}

const UNKNOWN_DEVICE: &str =
    "{\"GeneralReturn\":{\"Error\":1,\"ErrorMessage\":\"unknown device\"}}";

impl Protocol for Phoenix {
    fn ports(&self) -> Vec<f64> {
        vec![1250.0]
    }

    fn http(&mut self, cx: &mut Cx<'_>, url: &str) -> Result<Option<(f64, String)>, String> {
        let query = url_query(url);
        let action = query_get(&query, "action").unwrap_or("");
        let body = match action {
            "getdevices" => format!(
                "{},\"BusUtilPerc\":{},\"DeviceArray\":{}}}",
                self.general(action),
                self.bus_util.to_compact(),
                Json::Array(self.devices.iter().map(|d| d.device.clone()).collect()).to_compact()
            ),
            "getsignals" => match self.find(&query) {
                None => UNKNOWN_DEVICE.to_owned(),
                Some(device) => format!(
                    "{},\"Signals\":{}}}",
                    self.general(action),
                    Json::Array(device.signals.clone()).to_compact()
                ),
            },
            "plotpro" => match self.find(&query) {
                None => UNKNOWN_DEVICE.to_owned(),
                Some(device) => self.plot(cx.t, device, &query)?,
            },
            _ => return Ok(Some((404.0, String::new()))),
        };
        Ok(Some((200.0, body)))
    }

    fn step(&mut self, _cx: &mut Cx<'_>, op: &[Json]) -> Result<(), String> {
        match op_name(op).as_str() {
            "devices" => self.devices = Self::devices(Some(op.get(1).unwrap_or(&Json::Null)))?,
            "error" => {
                let Some(Json::Number(code)) = op
                    .get(2)
                    .filter(|c| c.as_f64().is_some_and(is_safe_integer))
                else {
                    return Err("phoenix: error code must be an integer".to_owned());
                };
                if let Some(action) = op.get(1).and_then(Json::as_str) {
                    let action = action.to_string_lossy();
                    self.errors.retain(|(a, _)| *a != action);
                    self.errors.push((action, *code));
                }
            }
            other => return Err(format!("phoenix: unknown step {other:?}")),
        }
        Ok(())
    }
}

pub struct Ftc {
    status: Json,
}

impl Ftc {
    pub fn new(config: &Json) -> Result<Self, String> {
        let status = present(config, "status")
            .cloned()
            .unwrap_or(Json::Object(Vec::new()));
        check_json_numbers(&status, "ftc status")?;
        Ok(Self { status })
    }
}

impl Protocol for Ftc {
    fn ports(&self) -> Vec<f64> {
        vec![8000.0]
    }

    fn accept_ws(&mut self, _url: &str, _protocols: &[String]) -> Option<String> {
        Some(String::new())
    }

    fn ws_message(&mut self, cx: &mut Cx<'_>, id: u32, data: &Payload) -> Result<(), String> {
        let Payload::Text(text) = data else {
            return Ok(());
        };
        let Ok(message) = json::parse(text) else {
            return Ok(());
        };
        if matches!(message.get("type"), Some(Json::String(t)) if t.eq_str("GET_ROBOT_STATUS")) {
            let reply = Json::object([
                ("type", Json::string("RECEIVE_ROBOT_STATUS")),
                ("status", self.status.clone()),
            ]);
            cx.push_ws(id, Payload::Text(reply.to_compact()));
        }
        Ok(())
    }

    fn step(&mut self, cx: &mut Cx<'_>, op: &[Json]) -> Result<(), String> {
        let arg = op.get(1).cloned().unwrap_or(Json::Null);
        let text = match op_name(op).as_str() {
            "status" => {
                check_json_numbers(&arg, "ftc status")?;
                self.status = arg;
                return Ok(());
            }
            "send" => {
                check_json_numbers(&arg, "ftc send")?;
                js_order(arg).to_compact()
            }
            "sendtext" => arg
                .as_str()
                .map(Utf16String::to_string_lossy)
                .ok_or("ftc: sendtext needs a string")?,
            other => return Err(format!("ftc: unknown step {other:?}")),
        };
        for id in cx.ws_ids() {
            cx.push_ws(id, Payload::Text(text.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::seconds_text;

    #[test]
    fn seconds_keep_six_decimals() {
        assert_eq!(seconds_text(5_012_000.0).unwrap(), "5.012000");
        assert_eq!(seconds_text(7.0).unwrap(), "0.000007");
        assert!(seconds_text(-1.0).is_err());
        assert!(seconds_text(0.5).is_err());
    }
}
