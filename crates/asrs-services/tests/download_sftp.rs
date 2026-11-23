//! Replays `files/download-sftp` (ssh2's SFTP session and fastGet against scripted packets and a
//! scripted fs) through `asrs_services::download::sftp`.

use std::collections::HashMap;

use asrs_parity::vectors::load;
use asrs_parity::{Json, Record};
use asrs_services::download::downloader::DirEntry;
use asrs_services::download::sftp::{Output, Sftp};
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect()
}

fn text(record: &Record, key: &str) -> String {
    record.string(key).unwrap_or_else(|| panic!("no {key:?}"))
}

fn id(record: &Record) -> u32 {
    record.field("id").and_then(Json::as_f64).expect("id") as u32
}

fn parts(record: &Record) -> Vec<u8> {
    let mut out = Vec::new();
    for part in record
        .field("parts")
        .and_then(Json::as_array)
        .expect("parts")
    {
        match part.as_str() {
            Some(text) => out.extend(unhex(&text.to_string_lossy())),
            None => {
                let field = |key| part.get(key).and_then(Json::as_f64).expect("a number") as usize;
                out.resize(out.len() + field("len"), field("fill") as u8);
            }
        }
    }
    out
}

fn optional(value: Option<f64>) -> Json {
    value.map_or(Json::Null, Json::Number)
}

fn entry(entry: &DirEntry) -> Json {
    Json::object([
        ("filename", Json::string(&entry.filename)),
        ("mode", optional(entry.mode.map(f64::from))),
        ("size", optional(entry.size)),
        ("dir", Json::Bool(entry.is_directory())),
    ])
}

fn effect(output: &Output) -> Json {
    let k = |name| ("k", Json::string(name));
    let n = Json::Number;
    match output {
        Output::Send(bytes) => Json::object([k("send"), ("hex", Json::string(&hex(bytes)))]),
        Output::Close => Json::object([k("close")]),
        Output::Ready => Json::object([k("ready")]),
        Output::Error(message) => Json::object([k("error"), ("message", Json::string(message))]),
        Output::Readdir { id, result } => {
            let result = match result {
                Ok(list) => ("list", Json::Array(list.iter().map(entry).collect())),
                Err(message) => ("error", Json::string(message)),
            };
            Json::object([k("readdir"), ("id", n(f64::from(*id))), result])
        }
        Output::Step {
            id,
            total,
            chunk,
            size,
        } => Json::object([
            k("step"),
            ("id", n(f64::from(*id))),
            ("total", n(*total)),
            ("nb", n(*chunk)),
            ("size", n(*size)),
        ]),
        Output::Got { id, result } => Json::object([
            k("get"),
            ("id", n(f64::from(*id))),
            (
                "error",
                match result {
                    Ok(()) => Json::Null,
                    Err(message) => Json::string(message),
                },
            ),
        ]),
        Output::FsOpen { path } => Json::object([
            k("fsOpen"),
            ("path", Json::string(path)),
            ("flags", Json::string("w")),
        ]),
        Output::FsWrite { fd, data, position } => Json::object([
            k("fsWrite"),
            ("fd", n(*fd as f64)),
            ("len", n(data.len() as f64)),
            ("sha256", Json::string(&hex(&Sha256::digest(data)[..8]))),
            ("pos", n(*position)),
        ]),
        Output::FsClose { fd } => Json::object([k("fsClose"), ("fd", n(*fd as f64))]),
    }
}

#[test]
fn sessions() {
    let mut session: Option<Sftp> = None;
    let mut next_fd = 10;
    let mut opens = std::collections::VecDeque::new();
    let mut compared: HashMap<String, usize> = HashMap::new();
    let records = load("files/download-sftp").expect("vector");
    for record in &records {
        let op = text(record, "op");
        if op == "begin" {
            let ident = text(record, "ident");
            session = Some(Sftp::new(
                ident.starts_with("SSH-2.0-OpenSSH") || ident.starts_with("SSH-2.0-dropbear"),
            ));
            next_fd = 10;
            opens.clear();
            continue;
        }
        let sftp = session.as_mut().expect("begin comes first");
        let mut threw = None;
        match op.as_str() {
            "init" => sftp.init(),
            "push" => threw = sftp.push(&parts(record)).err(),
            "end" => threw = sftp.end().err(),
            "readdir" => sftp.readdir(id(record), &text(record, "path")),
            "fastGet" => {
                sftp.fast_get(id(record), &text(record, "remote"), &text(record, "local"));
            }
            "fs" => {
                let fd: Option<u64> = opens.pop_front().expect("a pending fs call");
                let result = match record.field("error").expect("error") {
                    Json::Null => Ok(fd.unwrap_or(0)),
                    _ => Err(text(record, "error")),
                };
                sftp.fs_done(result);
            }
            other => panic!("unknown op {other:?}"),
        }
        let outputs = sftp.take();
        for output in &outputs {
            match output {
                Output::FsOpen { .. } => {
                    opens.push_back(Some(next_fd));
                    next_fd += 1;
                }
                Output::FsWrite { .. } | Output::FsClose { .. } => opens.push_back(None),
                _ => {}
            }
        }
        let expected = record.field("effects").expect("effects").to_compact();
        let actual = Json::Array(outputs.iter().map(effect).collect()).to_compact();
        assert_eq!(
            (record.string("threw"), expected),
            (threw.clone(), actual),
            "line {} ({op}, case {:?})",
            record.line,
            record.field("case"),
        );
        for output in &outputs {
            let kind = effect(output)
                .get("k")
                .and_then(Json::as_str)
                .expect("k")
                .to_string_lossy();
            *compared.entry(kind).or_default() += 1;
        }
        if threw.is_some() {
            *compared.entry("threw".to_owned()).or_default() += 1;
        }
    }
    for kind in [
        "send", "close", "ready", "error", "readdir", "step", "get", "fsOpen", "fsWrite",
        "fsClose", "threw",
    ] {
        assert!(compared.contains_key(kind), "never reached {kind}");
    }
}
