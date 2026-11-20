#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/selection/<case>.entry.json` against the real
//! `SelectionImpl` and compares the emitted records with the committed
//! `dom/selection/<case>` vector, line for line. The harness only converts
//! fixture inputs to method calls and the resulting state back to CVE-1
//! values; all selection logic lives in `asrs_model::selection_impl`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::time::Clock;
use asrs_log::log::Log;
use asrs_model::selection::Selection;
use asrs_model::selection_impl::{ControlButton, ElementRect, KeyTarget, SelectionImpl};
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors;

struct ScriptedClock(Rc<Cell<f64>>);

impl Clock for ScriptedClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

fn err(msg: impl Into<String>) -> String {
    msg.into()
}

fn number(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| err(format!("expected a number, got {}", v.kind())))
}

fn opt_number(v: &Value) -> Result<Option<f64>, String> {
    Ok(match v {
        Value::Null | Value::Undefined => None,
        _ => Some(number(v)?),
    })
}

fn pair(v: &Value) -> Result<[f64; 2], String> {
    let arr = v
        .as_array()
        .ok_or_else(|| err("expected a two-element array"))?;
    if arr.len() != 2 {
        return Err(err("expected a two-element array"));
    }
    Ok([number(&arr[0])?, number(&arr[1])?])
}

fn opt_pair(v: &Value) -> Result<Option<[f64; 2]>, String> {
    Ok(match v {
        Value::Null | Value::Undefined => None,
        _ => Some(pair(v)?),
    })
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null | Value::Undefined => false,
        Value::Number(bits) => {
            let n = f64::from_bits(*bits);
            n != 0.0 && !n.is_nan()
        }
        _ => true,
    }
}

fn js_string(v: &Value) -> Result<JsString, String> {
    match v {
        Value::String(s) => Ok(JsString::from_units(s.units().to_vec())),
        other => Err(err(format!("expected a string, got {}", other.kind()))),
    }
}

/// `{"log": [[method, ...args], ...]}`: `w.log[method](...args)`. Covers the
/// log writers and editors selection fixtures use.
fn run_log_step(log: &RefCell<Log>, calls: &[Value]) -> Result<(), String> {
    for call in calls {
        let parts = call
            .as_array()
            .ok_or_else(|| err("log step entries are [method, ...args]"))?;
        let (method, args) = parts
            .split_first()
            .ok_or_else(|| err("empty log step entry"))?;
        let method = method
            .as_str()
            .ok_or_else(|| err("log step method is a string"))?
            .to_string_lossy();
        let key = |i: usize| js_string(&args[i]);
        let log = &mut *log.borrow_mut();
        match method.as_str() {
            "putBoolean" => log.put_boolean(&key(0)?, number(&args[1])?, truthy(&args[2])),
            "putNumber" => log.put_number(&key(0)?, number(&args[1])?, number(&args[2])?),
            "putString" => log.put_string(&key(0)?, number(&args[1])?, js_string(&args[2])?),
            "putBooleanArray" => log.put_boolean_array(
                &key(0)?,
                number(&args[1])?,
                args[2]
                    .as_array()
                    .ok_or_else(|| err("putBooleanArray value is an array"))?
                    .iter()
                    .map(truthy)
                    .collect(),
            ),
            "putNumberArray" => log.put_number_array(
                &key(0)?,
                number(&args[1])?,
                args[2]
                    .as_array()
                    .ok_or_else(|| err("putNumberArray value is an array"))?
                    .iter()
                    .map(number)
                    .collect::<Result<_, String>>()?,
            ),
            "clearBeforeTime" => log.clear_before_time(number(&args[0])?, truthy(&args[1])),
            "deleteField" => log.delete_field(&key(0)?),
            "updateRangeWithTimestamp" => {
                log.update_range_with_timestamp(number(&args[0])?)
            }
            other => return Err(err(format!("unsupported log method {other}"))),
        }
    }
    Ok(())
}

struct Emit {
    values: Vec<(&'static str, Value)>,
}

fn selection_value(sel: &mut SelectionImpl) -> Value {
    let nullable = |t: Option<f64>| t.map_or(Value::Null, Value::number);
    let mode = sel.get_mode();
    let hovered = sel.get_hovered_time();
    let selected = sel.get_selected_time();
    let render = sel.get_render_time();
    let live = sel.get_current_live_time();
    let grab = sel.get_grab_zoom_range();
    let range = sel.get_timeline_range();
    let max_zoom = sel.get_timeline_is_max_zoom();
    Value::object([
        ("mode", Value::number(mode.number())),
        ("hoveredTime", nullable(hovered)),
        ("selectedTime", nullable(selected)),
        ("renderTime", nullable(render)),
        ("currentLiveTime", nullable(live)),
        (
            "grabZoomRange",
            grab.map_or(Value::Null, |r| {
                Value::Array(vec![Value::number(r[0]), Value::number(r[1])])
            }),
        ),
        (
            "timelineRange",
            Value::Array(vec![Value::number(range[0]), Value::number(range[1])]),
        ),
        ("timelineIsMaxZoom", Value::Bool(max_zoom)),
        (
            "buttons",
            Value::object([
                ("play", Value::Bool(sel.button_hidden(ControlButton::Play))),
                ("pause", Value::Bool(sel.button_hidden(ControlButton::Pause))),
                ("lock", Value::Bool(sel.button_hidden(ControlButton::Lock))),
                ("unlock", Value::Bool(sel.button_hidden(ControlButton::Unlock))),
                (
                    "showLockButtons",
                    Value::string(sel.show_lock_buttons()),
                ),
            ]),
        ),
    ])
}

fn button_for(selector: &str) -> Result<ControlButton, String> {
    if selector.ends_with(".play") {
        Ok(ControlButton::Play)
    } else if selector.ends_with(".pause") {
        Ok(ControlButton::Pause)
    } else if selector.ends_with(".unlock") {
        Ok(ControlButton::Unlock)
    } else if selector.ends_with(".lock") {
        Ok(ControlButton::Lock)
    } else {
        Err(err(format!("no control button for {selector}")))
    }
}

/// The rect a `contextmenu` event reports: the fixture's `layout` rules checked
/// for the event's selector (fixtures name the same selector in both), else the
/// harness's default `0x0` for unmatched elements.
fn event_rect(selector: &str, layout: &[Json]) -> Result<ElementRect, String> {
    for rule in layout {
        let s = rule
            .get("selector")
            .and_then(Json::as_str)
            .ok_or_else(|| err("layout rule without selector"))?
            .to_string_lossy();
        if s == selector {
            let rect = rule
                .get("rect")
                .and_then(Json::as_array)
                .ok_or_else(|| err("layout rule without rect"))?;
            let n = |i: usize| {
                rect[i]
                    .as_f64()
                    .ok_or_else(|| err("layout rect entries are numbers"))
            };
            return Ok(ElementRect {
                x: n(0)?,
                y: n(1)?,
                width: n(2)?,
                height: n(3)?,
            });
        }
    }
    Ok(ElementRect::default())
}

fn load_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    asrs_parity::json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The frame routine, mirroring the hub's per-frame selection reads: the
/// timeline paint calls `getTimelineRange`/`getHoveredTime`/`getSelectedTime`,
/// the sidebar reads mode/hovered/selected, and each active tab's command
/// reads `getRenderTime`/`getSelectedTime`/`getHoveredTime`/`getGrabZoomRange`.
fn frame(sel: &mut SelectionImpl) {
    sel.get_timeline_range();
    sel.get_hovered_time();
    sel.get_selected_time();
    sel.get_mode();
    sel.get_render_time();
    sel.get_grab_zoom_range();
    sel.get_current_live_time();
}

fn replay(spec_path: &Path) -> Result<Vec<Emit>, String> {
    let spec = load_json(spec_path)?;
    let spec_dir = spec_path.parent().unwrap();
    let fixture = spec
        .get("fixture")
        .ok_or_else(|| err("spec has no fixture"))?;
    let start = fixture
        .get("startTime")
        .and_then(Json::as_f64)
        .unwrap_or(1767225600000.0);
    let frame_period = fixture
        .get("window")
        .and_then(|w| w.get("framePeriod"))
        .and_then(Json::as_f64)
        .unwrap_or(16.0);
    let layout = fixture
        .get("layout")
        .and_then(Json::as_array)
        .unwrap_or(&[]);

    let clock_cell = Rc::new(Cell::new(start));
    let clock = Rc::new(ScriptedClock(clock_cell.clone()));
    let log = Rc::new(RefCell::new(Log::new(true)));
    let live_time = Rc::new(Cell::new(0.0f64));
    let mut sel = SelectionImpl::new(log.clone(), clock);
    // The boot `set-version` message (default darwin).
    let mut ctx = sel.context().clone();
    ctx.platform = fixture
        .get("version")
        .and_then(|v| v.get("platform"))
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| "darwin".to_owned());
    sel.set_context(ctx);

    let mut emitted = Vec::new();
    let drain_outbox = |sel: &mut SelectionImpl, emitted: &mut Vec<Emit>| {
        for message in sel.take_messages() {
            emitted.push(Emit {
                values: vec![
                    ("main", Value::string(message.name.name())),
                    ("data", js_json::from_js(&message.data)),
                ],
            });
        }
    };

    // hub.ts:553's touch-bar interval: setInterval at boot with `1000 / 60`,
    // dues accumulate in f64 the way `timer.due += interval` does in env.ts.
    // On darwin it reads `getCurrentLiveTime` and `getSelectedTime`.
    let touch_bar_period = 1000.0 / 60.0;
    let mut touch_bar_due = start + touch_bar_period;
    let mut last_frame = start;
    let mut advance = |ms: f64,
                      sel: &mut SelectionImpl,
                      emitted: &mut Vec<Emit>,
                      show_pending: &mut bool| {
        let end = clock_cell.get() + ms;
        loop {
            let frame_at = last_frame + frame_period;
            if touch_bar_due <= end && touch_bar_due <= frame_at {
                clock_cell.set(clock_cell.get().max(touch_bar_due));
                touch_bar_due += touch_bar_period;
                if sel.context().platform == "darwin" {
                    sel.get_current_live_time();
                    sel.get_selected_time();
                }
            } else if frame_at <= end {
                last_frame = frame_at;
                clock_cell.set(frame_at);
                frame(sel);
                if *show_pending {
                    *show_pending = false;
                    emitted.push(Emit {
                        values: vec![
                            ("main", Value::string("show")),
                            ("data", Value::Undefined),
                        ],
                    });
                }
            } else {
                clock_cell.set(end);
                break;
            }
        }
    };
    let mut show_pending = true;

    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .ok_or_else(|| err("fixture has no steps"))?;
    for (i, step) in steps.iter().enumerate() {
        let where_ = || format!("step {i}");
        if let Some(log_step) = step.get("log") {
            let calls: Vec<Value> = match log_step.as_str() {
                Some(path) => {
                    let path = spec_dir.join(path.to_string_lossy());
                    let ops = load_json(&path)?;
                    ops.as_array()
                        .ok_or_else(|| err(format!("{}: not an array", path.display())))?
                        .iter()
                        .map(Value::from_json_lenient)
                        .collect()
                }
                None => log_step
                    .as_array()
                    .ok_or_else(|| err("log step is an array or a path"))?
                    .iter()
                    .map(Value::from_json_lenient)
                    .collect(),
            };
            run_log_step(&log, &calls).map_err(|e| format!("{}: {e}", where_()))?;
            // `w.sidebar.refresh()` / `w.tabs.refresh()` read selection.
            sel.get_mode();
            sel.get_hovered_time();
            sel.get_selected_time();
            sel.get_current_live_time();
        } else if let Some(sel_step) = step.get("selection") {
            let parts = sel_step
                .as_array()
                .ok_or_else(|| err("selection step is [method, ...args]"))?;
            let (method, args) = parts
                .split_first()
                .ok_or_else(|| err("empty selection step"))?;
            let method = method
                .as_str()
                .ok_or_else(|| err("selection method is a string"))?
                .to_string_lossy();
            let arg = |i: usize| -> Result<Value, String> {
                Ok(args
                    .get(i)
                    .map(Value::from_json_lenient)
                    .unwrap_or(Value::Undefined))
            };
            match method.as_str() {
                "play" => sel.play(),
                "pause" => sel.pause(),
                "goIdle" => sel.go_idle(),
                "togglePlayback" => sel.toggle_playback(),
                "lock" => sel.lock(),
                "unlock" => sel.unlock(),
                "toggleLock" => sel.toggle_lock(),
                "stepCycle" => sel.step_cycle(truthy(&arg(0)?)),
                "setSelectedTime" => sel.set_selected_time(number(&arg(0)?)?),
                "setHoveredTime" => sel.set_hovered_time(opt_number(&arg(0)?)?),
                "setPlaybackSpeed" => sel.set_playback_speed(number(&arg(0)?)?),
                "setPlaybackLooping" => sel.set_playback_looping(truthy(&arg(0)?)),
                "setGrabZoomRange" => sel.set_grab_zoom_range(opt_pair(&arg(0)?)?),
                "finishGrabZoom" => sel.finish_grab_zoom(),
                "setTimelineRange" => {
                    sel.set_timeline_range(pair(&arg(0)?)?, truthy(&arg(1)?))
                }
                "applyTimelineScroll" => sel.apply_timeline_scroll(
                    number(&arg(0)?)?,
                    number(&arg(1)?)?,
                    number(&arg(2)?)?,
                ),
                "setLiveConnected" => {
                    let live = live_time.clone();
                    sel.set_live_connected(Rc::new(move || live.get()));
                }
                "setLiveDisconnected" => sel.set_live_disconnected(),
                other => return Err(format!("{}: unknown selection method {other}", where_())),
            }
        } else if let Some(live) = step.get("liveTime") {
            live_time.set(number(&Value::from_json_lenient(live))?);
        } else if let Some(main) = step.get("main") {
            let name = main
                .as_str()
                .ok_or_else(|| err("main step name is a string"))?
                .to_string_lossy();
            let data = step
                .get("data")
                .map(Value::from_json_lenient)
                .or_else(|| step.get("cve1").map(Value::from_json_lenient))
                .unwrap_or(Value::Undefined);
            match name.as_str() {
                "new-tab" => {
                    let mut ctx = sel.context().clone();
                    ctx.selected_tab_type = TabType::from_number(number(&data)?)
                        .ok_or_else(|| err("new-tab data is a tab type"))?;
                    sel.set_context(ctx);
                }
                "set-version" => {
                    let mut ctx = sel.context().clone();
                    ctx.platform = data
                        .get("platform")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default();
                    sel.set_context(ctx);
                }
                "set-playback-options" => {
                    sel.set_playback_speed(
                        data.get("speed").map(|v| number(v)).transpose()?.unwrap_or(f64::NAN),
                    );
                    sel.set_playback_looping(
                        data.get("looping").map(truthy).unwrap_or(false),
                    );
                }
                _ => {}
            }
        } else if let Some(event) = step.get("event") {
            let kind = event
                .get("kind")
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .unwrap_or_else(|| "Event".to_owned());
            let etype = event
                .get("type")
                .and_then(Json::as_str)
                .ok_or_else(|| err("event needs a type"))?
                .to_string_lossy();
            let target = event
                .get("target")
                .and_then(Json::as_str)
                .ok_or_else(|| err("event needs a target"))?
                .to_string_lossy();
            let init = event.get("init").unwrap_or(&Json::Null);
            let init_f64 = |k: &str| match init.get(k) {
                Some(Json::Bool(b)) => *b,
                Some(Json::Number(n)) => *n != 0.0 && !n.is_nan(),
                _ => false,
            };
            match (kind.as_str(), etype.as_str()) {
                ("MouseEvent", "click") => sel.click(button_for(&target)?),
                ("MouseEvent", "contextmenu") => {
                    sel.context_menu(button_for(&target)?, event_rect(&target, layout)?)
                }
                ("KeyboardEvent", "keydown") => {
                    let code = init
                        .get("code")
                        .and_then(Json::as_str)
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default();
                    let key_target = if target == "window" || target == "document" {
                        KeyTarget::Other
                    } else {
                        KeyTarget::Input
                    };
                    sel.key_down(&code, init_f64("metaKey"), init_f64("ctrlKey"), key_target);
                }
                other => return Err(format!("{}: unsupported event {other:?}", where_())),
            }
        } else if let Some(ms) = step.get("advance") {
            advance(
                ms.as_f64().ok_or_else(|| err("advance is a number"))?,
                &mut sel,
                &mut emitted,
                &mut show_pending,
            );
        } else if let Some(n) = step.get("frames") {
            advance(
                n.as_f64().ok_or_else(|| err("frames is a number"))? * frame_period,
                &mut sel,
                &mut emitted,
                &mut show_pending,
            );
        } else if let Some(capture) = step.get("capture") {
            let label = step
                .get("label")
                .map(Value::from_json_lenient)
                .unwrap_or_else(|| Value::number(i as f64));
            emitted.push(Emit {
                values: vec![
                    ("capture", label),
                    ("time", Value::number(clock_cell.get() - start)),
                ],
            });
            let kinds: Vec<&Json> = match capture.as_array() {
                Some(list) => list.iter().collect(),
                None => vec![capture],
            };
            for spec in kinds {
                let kind = match spec {
                    Json::String(_) => spec
                        .as_str()
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default(),
                    _ => spec
                        .get("kind")
                        .and_then(Json::as_str)
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default(),
                };
                match kind.as_str() {
                    "selection" => emitted.push(Emit {
                        values: vec![("selection", selection_value(&mut sel))],
                    }),
                    other => {
                        return Err(format!(
                            "{}: unsupported capture kind {other}",
                            where_()
                        ))
                    }
                }
            }
        } else {
            return Err(format!("{}: unknown step", where_()));
        }
        drain_outbox(&mut sel, &mut emitted);
    }
    Ok(emitted)
}

fn compare(spec: &Path, vector_id: &str) -> Result<(), String> {
    let emitted = replay(spec)?;
    let records = vectors::load(vector_id)?;
    if emitted.len() != records.len() {
        return Err(format!(
            "emitted {} records, vector has {}",
            emitted.len(),
            records.len()
        ));
    }
    for (emit, record) in emitted.iter().zip(records.iter()) {
        for (key, value) in &emit.values {
            let expected = record
                .value(key)
                .map_err(|e| format!("record {} missing {key}: {e}", record.line))?;
            if *value != expected {
                return Err(format!(
                    "line {} ({key}):\n  expected {}\n  actual   {}",
                    record.line,
                    expected.encode(),
                    value.encode()
                ));
            }
        }
    }
    Ok(())
}

fn specs() -> Vec<(PathBuf, String)> {
    let dir = vectors::workspace_root().join("parity/specs/dom/selection");
    let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .filter(|p| p.file_name().and_then(|s| s.to_str()).is_some_and(|n| n.ends_with(".entry.json")))
        .map(|p| {
            let name = p
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .trim_end_matches(".entry.json")
                .to_owned();
            (p, format!("dom/selection/{name}"))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn selection_traces_match_upstream() {
    let specs = specs();
    assert!(!specs.is_empty(), "no selection specs");
    let mut failures = Vec::new();
    for (spec, id) in &specs {
        if let Err(e) = compare(spec, id) {
            failures.push(format!("{id}: {e}"));
        }
    }
    if !failures.is_empty() {
        panic!("{}", failures.join("\n\n"));
    }
}
