use asrs_js::string::JsString;
use asrs_log::log::{Log, LogError, LoggableType};

#[test]
fn deep_json_throws_after_the_writes_upstream_makes() {
    let depth = 20_000;
    let doc = "[".repeat(depth) + &"]".repeat(depth);
    let mut log = Log::default();
    let result = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let result = log.put_json(&JsString::from("/deep"), 1.0, JsString::from(doc));
            (result, log)
        })
        .unwrap()
        .join()
        .unwrap();
    let (result, log) = result;
    assert_eq!(result, Err(LogError::StackOverflow));
    let lengths = log
        .get_field_keys()
        .iter()
        .filter(|k| k.to_string_lossy().ends_with("/length"))
        .count();
    assert_eq!(lengths, 9416);
    assert_eq!(
        log.get_type(&JsString::from("/deep")),
        Some(LoggableType::String)
    );
}
