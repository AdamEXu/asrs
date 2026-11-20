use std::thread;

use asrs_js::string::JsString;
use asrs_log::log::Log;

fn key(s: &str) -> JsString {
    JsString::from(s)
}

#[test]
fn snapshot_shares_columns_and_is_isolated_from_later_writes() {
    let mut log = Log::default();
    for i in 0..1000 {
        log.put_number(&key("/a"), f64::from(i), f64::from(i));
        log.put_number_array(&key("/b"), f64::from(i), vec![f64::from(i), 1.0]);
    }
    let snapshot = log.clone();
    let (Some(field), Some(shared)) = (log.get_field(&key("/a")), snapshot.get_field(&key("/a")))
    else {
        panic!("/a is missing");
    };
    assert!(std::ptr::eq(
        field.get_timestamps(),
        shared.get_timestamps()
    ));

    log.put_number(&key("/a"), 5000.0, -1.0);
    log.delete_field(&key("/b"));
    let worker = thread::spawn(move || {
        (
            snapshot.get_field_keys().len(),
            snapshot.get_field(&key("/a")).map(|f| f.len()),
            snapshot.get_timestamp_range(),
        )
    });
    let (keys, a_len, range) = worker.join().unwrap_or_default();
    assert_eq!(keys, log.get_field_keys().len() + 1);
    assert_eq!(a_len, Some(1000));
    assert_eq!(range, [0.0, 999.0]);
    assert_eq!(log.get_timestamp_range(), [0.0, 5000.0]);
}

#[test]
fn log_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Log>();
}
