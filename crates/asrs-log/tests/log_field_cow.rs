use std::sync::Arc;
use std::thread;

use asrs_js::string::JsString;
use asrs_log::log::{LogField, LogValues, LoggableType};

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
