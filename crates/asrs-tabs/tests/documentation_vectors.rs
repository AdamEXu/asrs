#![allow(clippy::disallowed_methods)]

//! The `tabs/documentation` records with no page behind the frame: what the page does on its own
//! (`load`, `settle` and `click` steps) is fed in, and what `saveState` returns after every
//! `save`, `restore` and `render` step is compared. `documentation_window` in asrs-app replays
//! the same records on the real page.

use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_model::tab_controller::TabRenderer;
use asrs_parity::cve1::Value;
use asrs_parity::vectors;
use asrs_tabs::documentation::{DOCS_URL, DocumentationRenderer};

fn js(value: &Value) -> JsValue {
    match value {
        Value::Null => JsValue::Null,
        Value::Bool(b) => JsValue::Bool(*b),
        Value::Number(bits) => JsValue::Number(f64::from_bits(*bits)),
        Value::String(s) => JsValue::String(JsString::from_units(s.units().to_vec())),
        Value::Array(items) => JsValue::Array(items.iter().map(js).collect()),
        Value::Object(fields) => {
            let mut object = JsObject::new();
            for (key, value) in fields {
                object.insert(key.to_string_lossy().as_str(), js(value));
            }
            JsValue::Object(object)
        }
        other => panic!("a {} state", other.kind()),
    }
}

fn hash(value: &JsValue) -> JsString {
    match value {
        JsValue::String(s) => s.clone(),
        other => panic!("{other:?} is no hash"),
    }
}

#[test]
fn saved_hashes() {
    let records = vectors::load("tabs/documentation").unwrap();
    let (mut compared, mut fed, mut steps_total) = (0, 0, 0);
    for record in &records {
        let case = Value::from_json(&record.json).unwrap();
        let name = case.get("name").unwrap().as_str().unwrap();
        let mut renderer = DocumentationRenderer::new();
        let cx = &mut ();
        let mut seen = 0;
        for (i, step) in case
            .get("steps")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .enumerate()
        {
            steps_total += 1;
            let expected = js(step.get("saved").unwrap());
            let op = step.get("op").unwrap().as_str().unwrap().to_string_lossy();
            let page = match op.as_str() {
                "save" => None,
                "restore" => {
                    let state = js(step.get("state").unwrap());
                    TabRenderer::<()>::restore_state(&mut renderer, cx, &state).unwrap();
                    None
                }
                "render" => {
                    let command = Rc::new(JsValue::Null);
                    TabRenderer::<()>::render(&mut renderer, cx, &*command).unwrap();
                    None
                }
                "load" => {
                    renderer.frame().borrow_mut().load(hash(&expected));
                    seen = 0;
                    Some(())
                }
                "settle" | "click" => {
                    let mut frame = renderer.frame().borrow_mut();
                    if let Some(assignment) = frame.take_assignment() {
                        seen = assignment.id;
                    }
                    frame.navigated(seen, hash(&expected));
                    Some(())
                }
                other => panic!("{other} step"),
            };
            if page.is_some() {
                fed += 1;
                continue;
            }
            assert_eq!(
                TabRenderer::<()>::save_state(&renderer, cx),
                expected,
                "{name} step {i} ({op})"
            );
            compared += 1;
        }
    }
    assert_eq!(records.len(), 29);
    assert_eq!(compared + fed, steps_total);
    assert_eq!((compared, fed), (124, 88));
}

#[test]
fn a_report_from_before_the_latest_assignment_is_out_of_date() {
    let mut renderer = DocumentationRenderer::new();
    let cx = &mut ();
    let frame = renderer.frame().clone();
    frame.borrow_mut().load(JsString::from("#/"));
    let state = JsValue::string("#/whats-new");
    TabRenderer::<()>::restore_state(&mut renderer, cx, &state).unwrap();
    frame
        .borrow_mut()
        .navigated(0, JsString::from("#/legal/support"));
    assert_eq!(TabRenderer::<()>::save_state(&renderer, cx), state);
    let assignment = frame.borrow_mut().take_assignment().unwrap();
    assert_eq!(assignment.hash, "#/whats-new");
    assert_eq!(frame.borrow_mut().take_assignment(), None);
    frame
        .borrow_mut()
        .navigated(assignment.id, JsString::from("#/"));
    assert_eq!(
        TabRenderer::<()>::save_state(&renderer, cx),
        JsValue::string("#/")
    );
}

#[test]
fn the_first_render_sets_the_source_and_a_reload_drops_what_was_assigned() {
    let mut renderer = DocumentationRenderer::new();
    let cx = &mut ();
    let frame = renderer.frame().clone();
    assert_eq!(frame.borrow().src(), None);
    let command = Rc::new(JsValue::Null);
    TabRenderer::<()>::render(&mut renderer, cx, &*command).unwrap();
    assert_eq!(frame.borrow().src(), Some(DOCS_URL));
    frame.borrow_mut().load(JsString::from("#/"));
    let state = JsValue::string("#/whats-new");
    TabRenderer::<()>::restore_state(&mut renderer, cx, &state).unwrap();
    frame.borrow_mut().load(JsString::from("#/"));
    assert_eq!(frame.borrow_mut().take_assignment(), None);
    frame
        .borrow_mut()
        .navigated(0, JsString::from("#/legal/support"));
    assert_eq!(
        TabRenderer::<()>::save_state(&renderer, cx),
        JsValue::string("#/legal/support")
    );
}
