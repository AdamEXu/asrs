use crate::cve1::Value;
use crate::json::{self, Json, write_string};

/// One `fn` request line: `{"fn": "<name>", "args": [<CVE-1>, ...]}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub name: String,
    pub args: Vec<Value>,
}

impl Request {
    pub fn new(name: &str, args: Vec<Value>) -> Self {
        Self {
            name: name.to_owned(),
            args,
        }
    }

    /// Errors carry the message the TS runner would put in `{"error": ...}`.
    pub fn parse(line: &str) -> Result<Self, String> {
        let json = json::parse(line).map_err(|e| e.to_string())?;
        Self::from_json(&json)
    }

    pub fn from_json(json: &Json) -> Result<Self, String> {
        let name = match json.get("fn") {
            Some(Json::String(name)) if matches!(json, Json::Object(_)) => name.to_string_lossy(),
            _ => return Err(r#"request must be an object with a string "fn""#.to_owned()),
        };
        let args = match json.get("args") {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(items)) => items
                .iter()
                .map(Value::from_json)
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?,
            Some(_) => return Err(r#""args" must be an array"#.to_owned()),
        };
        Ok(Self { name, args })
    }

    pub fn encode(&self) -> String {
        let mut out = String::from(r#"{"fn":"#);
        write_string(&self.name.encode_utf16().collect::<Vec<_>>(), &mut out);
        out.push_str(r#","args":["#);
        for (i, arg) in self.args.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            arg.encode_into(&mut out);
        }
        out.push_str("]}");
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Ok(Value),
    Throw(Value),
    /// The request was malformed or the name did not resolve; not upstream behaviour.
    Error(String),
}

impl Response {
    pub fn encode(&self) -> String {
        let mut out = String::new();
        match self {
            Response::Ok(v) => {
                out.push_str(r#"{"ok":"#);
                v.encode_into(&mut out);
            }
            Response::Throw(v) => {
                out.push_str(r#"{"throw":"#);
                v.encode_into(&mut out);
            }
            Response::Error(message) => {
                out.push_str(r#"{"error":"#);
                write_string(&message.encode_utf16().collect::<Vec<_>>(), &mut out);
            }
        }
        out.push('}');
        out
    }

    pub fn parse(line: &str) -> Result<Self, String> {
        let json = json::parse(line).map_err(|e| e.to_string())?;
        match &json {
            Json::Object(pairs) if pairs.len() == 1 => Self::from_record(&json),
            _ => Err("a response must have exactly one of ok, throw or error".to_owned()),
        }
    }

    /// Reads the response part of a JSON object that may carry other fields, such as a
    /// vector record that merges the request with its response.
    pub fn from_record(json: &Json) -> Result<Self, String> {
        let decode = |v: &Json| Value::from_json(v).map_err(|e| e.to_string());
        if let Some(v) = json.get("ok") {
            return decode(v).map(Response::Ok);
        }
        if let Some(v) = json.get("throw") {
            return decode(v).map(Response::Throw);
        }
        match json.get("error") {
            Some(Json::String(message)) => Ok(Response::Error(message.to_string_lossy())),
            _ => Err("no ok, throw or error field".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_follow_the_ts_runner() {
        let r = Request::parse(r#"{"fn":"Math.sin","args":[1]}"#).unwrap();
        assert_eq!(r, Request::new("Math.sin", vec![Value::number(1.0)]));
        assert_eq!(
            r.encode(),
            r#"{"fn":"Math.sin","args":[{"n":"3ff0000000000000"}]}"#
        );
        assert_eq!(Request::parse(r#"{"fn":"x"}"#).unwrap().args, vec![]);
        assert!(Request::parse(r#"{"fn":1}"#).is_err());
        assert!(Request::parse(r#"{"fn":"x","args":{}}"#).is_err());
        assert!(Request::parse(r#"[1]"#).is_err());
    }

    #[test]
    fn responses_round_trip() {
        for line in [
            r#"{"ok":{"u":null}}"#,
            r#"{"throw":{"err":["TypeError","x"]}}"#,
            r#"{"error":"bad \"name\""}"#,
        ] {
            assert_eq!(Response::parse(line).unwrap().encode(), line);
        }
    }
}
