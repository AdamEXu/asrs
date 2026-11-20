use asrs_parity::{Record, Value, diff, record_vectors};

fn check(record: &Record) -> Result<(), String> {
    let input = record.string("input").ok_or("record has no input")?;
    let kind = record.string("kind").ok_or("record has no kind")?;
    let expected = record.string("rust").or_else(|| record.string("output"));
    if kind == "roundtrip" && expected.as_deref() != Some(input.as_str()) {
        return Err("roundtrip record whose output differs from its input".to_owned());
    }
    match (Value::decode(&input), expected) {
        (Ok(value), Some(expected)) => match diff::report_lines(&expected, &value.encode()) {
            None => Ok(()),
            Some(report) => Err(format!(
                "{}: {report}",
                record.string("case").unwrap_or_default()
            )),
        },
        (Err(_), None) => Ok(()),
        (Ok(value), None) => Err(format!(
            "{}: the TS decoder rejects this but Rust decoded {}",
            record.string("case").unwrap_or_default(),
            diff::summary(&value)
        )),
        (Err(e), Some(_)) => Err(format!(
            "{}: {e}",
            record.string("case").unwrap_or_default()
        )),
    }
}

record_vectors!(torture_set_matches_the_ts_decoder, "cve1/torture", check);
