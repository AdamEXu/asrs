//! Replays `log/rlog-cycles` (oracle/src/rlogCycles.ts): what each `RLOGDecoder.decode` call
//! returned, or that it never did, on logs whose negative lengths send the offset back.

use asrs_formats::rlog::RlogDecoder;
use asrs_formats::worker::Abort;
use asrs_js::time::{SetClock, SetRng};
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;
use asrs_parity::json::Json;
use asrs_parity::vectors;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn rlog_cycles() {
    let records = vectors::load("log/rlog-cycles").unwrap_or_else(|e| panic!("{e}"));
    let (mut never, mut failed) = (0, 0);
    for record in &records {
        let name = record.string("name").expect("name");
        let array = |key: &str| record.field(key).and_then(Json::as_array).expect("array");
        let is_file = record.field("isFile") == Some(&Json::Bool(true));
        let mut realm = JsRealm::new(Box::new(SetRng::new(vec![0.5])));
        let mut log = Log::new(false);
        let mut decoder = RlogDecoder::new(is_file);
        let mut results = Vec::new();
        for frame in array("frames") {
            let frame = bytes(&frame.as_str().expect("hex").to_string_lossy());
            let end = decoder.decode(&mut realm, &mut log, &frame, &SetClock(0.0), None);
            results.push(match end {
                Ok(success) => Json::Bool(success),
                Err(Abort::NeverReturns) => Json::string("never"),
                Err(other) => panic!("{name}: {other:?}"),
            });
        }
        assert_eq!(results, array("results"), "{name}");
        never += usize::from(results.contains(&Json::string("never")));
        failed += usize::from(results.contains(&Json::Bool(false)));

        let Some(Json::Array(fields)) = record.field("fields") else {
            assert_eq!(record.field("fields"), Some(&Json::Null), "{name}");
            continue;
        };
        let got: Vec<Json> = log
            .get_field_keys()
            .iter()
            .map(|key| {
                let field = log.get_field(key).expect("a listed field");
                let times = field.get_timestamps();
                Json::Array(vec![
                    Json::string(&key.to_string_lossy()),
                    Json::Number(f64::from(field.get_type() as u8)),
                    Json::Array(times.iter().map(|t| Json::Number(*t)).collect()),
                ])
            })
            .collect();
        assert_eq!(&got, fields, "{name}");
    }
    assert!(records.len() >= 15 && never >= 8 && failed >= 4);
}
