use asrs_js::js_object;
use asrs_js::json::{json_copy, stringify, stringify_pretty};
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, ToJs, array_index};

fn keys<V>(object: &JsObject<V>) -> Vec<String> {
    object.keys().map(JsString::to_string_lossy).collect()
}

#[test]
fn property_order_puts_array_indices_first() {
    let mut object = JsObject::new();
    for key in [
        "b",
        "10",
        "a",
        "2",
        "4294967295",
        "4294967294",
        "01",
        "-1",
        "0",
        "1.0",
    ] {
        object.insert(key, JsValue::Null);
    }
    assert_eq!(
        keys(&object),
        [
            "0",
            "2",
            "10",
            "4294967294",
            "b",
            "a",
            "4294967295",
            "01",
            "-1",
            "1.0"
        ]
    );
}

#[test]
fn reassignment_keeps_position_and_delete_moves_to_end() {
    let mut object: JsObject<f64> = [("a", 1.0), ("b", 2.0), ("c", 3.0)].into_iter().collect();
    assert_eq!(object.insert("a", 4.0), Some(1.0));
    assert_eq!(keys(&object), ["a", "b", "c"]);
    assert_eq!(object.remove("a"), Some(4.0));
    object.insert("a", 5.0);
    assert_eq!(keys(&object), ["b", "c", "a"]);
    assert_eq!(object.get("a"), Some(&5.0));
    assert!(!object.contains_key("z"));
}

#[test]
fn array_index_bounds() {
    let index = |s: &str| array_index(&s.encode_utf16().collect::<Vec<u16>>());
    assert_eq!(index("0"), Some(0));
    assert_eq!(index("4294967294"), Some(4_294_967_294));
    assert_eq!(index("4294967295"), None);
    assert_eq!(index("00"), None);
    assert_eq!(index(""), None);
    assert_eq!(index("+1"), None);
}

struct State {
    name: String,
    width: f64,
    hidden: Option<bool>,
    children: Vec<u32>,
}

impl ToJs for State {
    fn to_js(&self) -> JsValue {
        js_object! {
            "name" => self.name,
            "width" => self.width,
            "hidden" => self.hidden,
            "children" => self.children,
        }
    }
}

#[test]
fn to_js_states_stringify_in_declared_order() {
    let state = State {
        name: "a\u{2028}\"".to_owned(),
        width: -0.0,
        hidden: None,
        children: vec![1, 2],
    };
    let value = state.to_js();
    assert_eq!(
        stringify(&value).unwrap().unwrap(),
        "{\"name\":\"a\u{2028}\\\"\",\"width\":0,\"children\":[1,2]}"
    );
    assert_eq!(
        stringify_pretty(&value).unwrap().unwrap(),
        "{\n  \"name\": \"a\u{2028}\\\"\",\n  \"width\": 0,\n  \"children\": [\n    1,\n    2\n  ]\n}"
    );
    let copy = json_copy(&value).unwrap();
    assert_eq!(copy.get("width"), Some(&JsValue::Number(0.0)));
    assert_eq!(copy.get("hidden"), None);
}

#[test]
fn undefined_and_lone_surrogates() {
    assert_eq!(stringify(&JsValue::Undefined).unwrap(), None);
    let lone = JsValue::String(JsString::from_units(vec![
        0xd800, 0x41, 0xd83d, 0xde00, 0xdc00,
    ]));
    assert_eq!(
        stringify(&lone).unwrap().unwrap(),
        "\"\\ud800A\u{1f600}\\udc00\""
    );
    assert_eq!(
        json_copy(&JsValue::Undefined)
            .unwrap_err()
            .message
            .to_string_lossy(),
        "\"undefined\" is not valid JSON"
    );
}
