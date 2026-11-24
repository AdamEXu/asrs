//! Replays `export/mcap-reentry` (oracle/src/mcapReentry.ts): upstream's export worker on sampled
//! MCAP exports that outgrow a chunk, by file hash, progress hash or rejection.

use asrs_formats::export::{ExportError, ExportOptions, Format, Sampling, export};
use asrs_formats::mcap::ARRAY_BUFFER_MAX;
use asrs_js::string::JsString;
use asrs_js::time::SetRng;
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;
use asrs_parity::json::Json;
use asrs_parity::vectors;
use sha2::{Digest, Sha256};

fn hex(hash: Sha256) -> String {
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn sample_log(fields: &[Json], samples: f64) -> Log {
    let mut log = Log::new(true);
    for field in fields {
        let text = |key: &str| {
            let text = field.get(key).and_then(Json::as_str).expect(key);
            text.to_string_lossy()
        };
        let number = |key: &str| field.get(key).and_then(Json::as_f64).expect(key);
        let string = || JsString::from("x".repeat(number("length") as usize).as_str());
        let key = JsString::from(text("key").as_str());
        match text("type").as_str() {
            "number" => log.put_number(&key, number("start"), number("value")),
            "string" => log.put_string(&key, number("start"), string()),
            "string[]" => log.put_string_array(&key, number("start"), vec![string()]),
            other => panic!("field type {other}"),
        }
    }
    let mut serialized = log.to_serialized();
    serialized.timestamp_range = Some([0.0, samples - 1.0]);
    let mut realm = JsRealm::new(Box::new(SetRng::new(vec![0.5])));
    Log::from_serialized(serialized, &mut realm).expect("no descriptors")
}

#[test]
fn mcap_reentry() {
    let records = vectors::load("export/mcap-reentry").unwrap_or_else(|e| panic!("{e}"));
    let options = ExportOptions {
        format: Format::Mcap,
        sampling: Sampling::Fixed,
        sampling_period: 1000.0,
        prefixes: JsString::new(),
        include_generated: true,
    };
    let (limit, cases) = records.split_first().expect("records");
    assert_eq!(
        limit.field("arrayBufferMax").and_then(Json::as_f64),
        Some(ARRAY_BUFFER_MAX as f64)
    );
    let mut rejected = 0;
    for case in cases {
        let name = case.string("name").expect("name");
        let array = |key: &str| case.field(key).and_then(Json::as_array).expect("array");
        let samples = case.field("samples").and_then(Json::as_f64);
        let log = sample_log(array("fields"), samples.expect("samples"));

        let (mut posted, mut progress) = (0usize, Sha256::new());
        let output = export(&log, &options, &mut |value| {
            posted += 1;
            progress.update(value.to_le_bytes());
        });
        assert_eq!(
            [Json::Number(posted as f64), Json::string(&hex(progress))],
            array("progress"),
            "{name}: progress"
        );
        match (case.field("file"), output) {
            (Some(Json::Null), Err(ExportError::Rejected)) => rejected += 1,
            (Some(Json::Array(file)), Ok(output)) => {
                let bytes = output.into_bytes();
                let length = Json::Number(bytes.len() as f64);
                let hash = Json::string(&hex(Sha256::new_with_prefix(&bytes)));
                assert_eq!(&[length, hash], file.as_slice(), "{name}");
            }
            (file, output) => panic!("{name}: upstream {file:?}, port {:?}", output.err()),
        }
    }
    assert_eq!((cases.len(), rejected), (14, 1));
}
