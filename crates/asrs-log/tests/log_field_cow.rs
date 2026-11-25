use std::sync::Arc;
use std::thread;

use asrs_js::string::JsString;
use asrs_js::time::SetRng;
use asrs_log::log::{Log, LogField, LogValues, LoggableType, SerializedLogField, StructuredType};
use asrs_log::realm::JsRealm;

fn number_field() -> LogField {
    let mut field = LogField::new(LoggableType::Number);
    for i in 0..5 {
        field.put_number(f64::from(i), f64::from(i) * 10.0);
    }
    field
}

fn number_column(values: &LogValues) -> &Arc<Vec<f64>> {
    match values {
        LogValues::Number(column) => column,
        other => panic!("not a number column: {other:?}"),
    }
}

#[test]
fn clone_shares_columns_until_written() {
    let mut field = number_field();
    let snapshot = field.clone();
    assert!(std::ptr::eq(
        field.get_timestamps(),
        snapshot.get_timestamps()
    ));
    assert!(Arc::ptr_eq(
        number_column(field.values()),
        number_column(snapshot.values())
    ));

    field.put_number(4.0, -1.0);
    assert!(std::ptr::eq(
        field.get_timestamps(),
        snapshot.get_timestamps()
    ));
    assert!(!Arc::ptr_eq(
        number_column(field.values()),
        number_column(snapshot.values())
    ));
    assert_eq!(
        snapshot.column::<f64>().unwrap(),
        &[0.0, 10.0, 20.0, 30.0, 40.0]
    );
    assert_eq!(
        field.column::<f64>().unwrap(),
        &[0.0, 10.0, 20.0, 30.0, -1.0]
    );

    field.put_number(10.0, 5.0);
    assert!(!std::ptr::eq(
        field.get_timestamps(),
        snapshot.get_timestamps()
    ));
    assert_eq!(snapshot.get_timestamps(), &[0.0, 1.0, 2.0, 3.0, 4.0]);
    assert_eq!(field.get_timestamps(), &[0.0, 1.0, 2.0, 3.0, 4.0, 10.0]);
}

#[test]
fn deduplicated_write_keeps_sharing() {
    let mut field = number_field();
    let snapshot = field.clone();
    field.put_number(4.5, 40.0);
    assert!(std::ptr::eq(
        field.get_timestamps(),
        snapshot.get_timestamps()
    ));
    assert!(Arc::ptr_eq(
        number_column(field.values()),
        number_column(snapshot.values())
    ));
}

#[test]
fn clear_on_snapshot_leaves_original() {
    let field = number_field();
    let mut snapshot = field.clone();
    snapshot.clear_before_time(3.5);
    assert_eq!(snapshot.get_timestamps(), &[3.5, 4.0]);
    assert!(snapshot.get_striping_reference());
    assert_eq!(field.get_timestamps(), &[0.0, 1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        field.column::<f64>().unwrap(),
        &[0.0, 10.0, 20.0, 30.0, 40.0]
    );
    assert!(!field.get_striping_reference());
}

#[test]
fn serialized_shares_and_snapshot_crosses_threads() {
    let mut field = LogField::new(LoggableType::String);
    field.put_string(1.0, JsString::from("a"));
    field.put_string(2.0, JsString::from("b"));
    let serialized = field.to_serialized();
    assert!(Arc::ptr_eq(
        &serialized.timestamps,
        &field.to_serialized().timestamps
    ));

    let snapshot = field.clone();
    let reader = thread::spawn(move || {
        snapshot
            .get_string(f64::NEG_INFINITY, f64::INFINITY, Some("export"), None)
            .unwrap()
    });
    field.put_string(3.0, JsString::from("c"));
    let read = reader.join().unwrap();
    assert_eq!(read.timestamps, vec![1.0, 2.0]);
    assert_eq!(read.values, vec![JsString::from("a"), JsString::from("b")]);
    assert_eq!(field.len(), 3);
    assert_eq!(LogField::from_serialized(serialized).len(), 2);
}

#[test]
fn field_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LogField>();
}

fn serialized_number_field(timestamps: Vec<f64>) -> SerializedLogField {
    SerializedLogField {
        timestamps: Arc::new(timestamps),
        values: LogValues::new(LoggableType::Number),
        structured_type: StructuredType::Null,
        wpilib_type: None,
        metadata_string: JsString::new(),
        striping_reference: false,
        type_warning: false,
    }
}

fn bits(column: &[f64]) -> Vec<u64> {
    column.iter().map(|t| t.to_bits()).collect()
}

fn assert_shift(timestamps: Vec<f64>, offset: f64, expect_shared: bool) {
    let mut field = serialized_number_field(timestamps.clone());
    let original = Arc::clone(&field.timestamps);
    field.shift_timestamps(offset);
    let expected: Vec<u64> = timestamps.iter().map(|t| (*t + offset).to_bits()).collect();
    assert_eq!(bits(&field.timestamps), expected);
    assert_eq!(Arc::ptr_eq(&field.timestamps, &original), expect_shared);
}

#[test]
fn shift_by_zero_shares_the_column() {
    assert_shift(vec![0.0, 1.5, 100.25, 1e300], 0.0, true);
}

#[test]
fn shift_by_negative_zero_shares_even_with_negative_zero() {
    assert_shift(vec![-0.0, 0.0, 1.0], -0.0, true);
}

#[test]
fn shift_by_zero_copies_when_a_negative_zero_would_flip_sign() {
    let mut field = serialized_number_field(vec![1.0, -0.0]);
    let original = Arc::clone(&field.timestamps);
    field.shift_timestamps(0.0);
    assert!(!Arc::ptr_eq(&field.timestamps, &original));
    assert_eq!(
        bits(&field.timestamps),
        vec![1.0f64.to_bits(), 0.0f64.to_bits()]
    );
}

#[test]
fn shift_shares_canonical_nan_and_infinities() {
    assert_shift(vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY], 0.0, true);
    assert_shift(vec![f64::INFINITY, f64::NEG_INFINITY], 123.456, true);
}

#[test]
fn shift_shares_huge_values_when_the_offset_rounds_away() {
    assert_shift(vec![1e308, -1e308], 1.0, true);
    assert_shift(vec![1e308], 1e293, false);
}

#[test]
fn shift_shares_a_quiet_nan_payload() {
    assert_shift(vec![f64::from_bits(0x7ff8_0000_0000_0001), 1.0], 0.0, true);
}

#[test]
fn shift_copies_when_a_signaling_nan_would_be_quieted() {
    assert_shift(vec![f64::from_bits(0x7ff0_0000_0000_0001), 1.0], 0.0, false);
}

#[test]
fn structured_clone_canonicalizes_a_nan_payload_then_shift_shares_the_canonical_column() {
    let field = serialized_number_field(vec![f64::from_bits(0x7ff8_0000_0000_0001), 1.0]);
    let original = Arc::clone(&field.timestamps);
    let mut cloned = field.structured_clone();
    assert!(!Arc::ptr_eq(&cloned.timestamps, &original));
    assert_eq!(
        bits(&cloned.timestamps),
        vec![f64::NAN.to_bits(), 1.0f64.to_bits()]
    );
    let canonical = Arc::clone(&cloned.timestamps);
    cloned.shift_timestamps(0.0);
    assert!(Arc::ptr_eq(&cloned.timestamps, &canonical));
}

#[test]
fn shift_by_nonzero_offset_copies() {
    assert_shift(vec![0.0, 1.0, 2.0], 10.0, false);
}

#[test]
fn shift_empty_column_shares() {
    assert_shift(Vec::new(), 5.0, true);
}

fn number_log() -> (Log, JsString) {
    let key = JsString::from("/A");
    let mut log = Log::new(false);
    log.put_number(&key, 0.0, 1.0).unwrap();
    log.put_number(&key, 1.0, 2.0).unwrap();
    (log, key)
}

#[test]
fn merge_with_zero_offset_shares_the_timestamp_column() {
    let (source, key) = number_log();
    let mut target = Log::new(false);
    let mut realm = JsRealm::new(Box::new(SetRng::new(vec![0.5])));
    let offset = target
        .merge_with(&source, &JsString::new(), &mut realm)
        .unwrap();
    assert_eq!(offset.to_bits(), 0.0f64.to_bits());
    let merged = target.get_field(&key).unwrap().to_serialized();
    let original = source.get_field(&key).unwrap().to_serialized();
    assert!(Arc::ptr_eq(&merged.timestamps, &original.timestamps));
    assert_eq!(bits(&merged.timestamps), bits(&original.timestamps));
}

#[test]
fn merge_with_nonzero_offset_copies_and_shifts() {
    let (source, key) = number_log();
    let mut target = Log::new(false);
    target.update_range_with_timestamp(100.0);
    target.update_range_with_timestamp(200.0);
    let mut realm = JsRealm::new(Box::new(SetRng::new(vec![0.5])));
    let offset = target
        .merge_with(&source, &JsString::new(), &mut realm)
        .unwrap();
    assert_eq!(offset.to_bits(), 100.0f64.to_bits());
    let merged = target.get_field(&key).unwrap().to_serialized();
    let original = source.get_field(&key).unwrap().to_serialized();
    assert!(!Arc::ptr_eq(&merged.timestamps, &original.timestamps));
    assert_eq!(merged.timestamps.as_slice(), &[100.0, 101.0]);
}

#[test]
fn writing_a_merged_field_leaves_the_shared_source_column_alone() {
    let (source, key) = number_log();
    let mut target = Log::new(false);
    let mut realm = JsRealm::new(Box::new(SetRng::new(vec![0.5])));
    target
        .merge_with(&source, &JsString::new(), &mut realm)
        .unwrap();
    target.put_number(&key, 0.5, 9.0).unwrap();
    let source_field = source.get_field(&key).unwrap();
    assert_eq!(source_field.get_timestamps(), &[0.0, 1.0]);
    assert_eq!(source_field.column::<f64>().unwrap(), &[1.0, 2.0]);
    let merged = target.get_field(&key).unwrap();
    assert_eq!(merged.get_timestamps(), &[0.0, 0.5, 1.0]);
    assert_eq!(merged.column::<f64>().unwrap(), &[1.0, 9.0, 2.0]);
}
