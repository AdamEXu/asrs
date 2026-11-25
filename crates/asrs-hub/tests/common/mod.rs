#![allow(clippy::disallowed_methods)]
#![allow(dead_code)]

//! Replays a DOM-harness spec against the real `SelectionImpl`, timeline and touch
//! bar tick and compares the emitted records with the committed vector, line for
//! line.
//!
//! The replay loop stands in for two things the port gets elsewhere: the
//! oracle's fake-clock event loop (a timer due at or before the next frame runs
//! first), which GPUI's executor timers and `request_animation_frame` replace in
//! the app, and the per-frame selection reads of the sidebar and tab
//! controllers, which U08 and the tab units own. The getters only mutate as
//! a function of the clock, so what matters is that each frame reads them.

#[path = "../../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_canvas::text::FixedAdvance;
use asrs_hub::timeline::{ScrollTarget, Timeline, TimelineFrame};
use asrs_hub::touch_bar;
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::time::Clock;
use asrs_log::log::Log;
use asrs_model::preferences::Preferences;
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
            "putBoolean" => log
                .put_boolean(&key(0)?, number(&args[1])?, truthy(&args[2]))
                .unwrap(),
            "putNumber" => log
                .put_number(&key(0)?, number(&args[1])?, number(&args[2])?)
                .unwrap(),
            "putString" => log
                .put_string(&key(0)?, number(&args[1])?, js_string(&args[2])?)
                .unwrap(),
            "putBooleanArray" => log
                .put_boolean_array(
                    &key(0)?,
                    number(&args[1])?,
                    args[2]
                        .as_array()
                        .ok_or_else(|| err("putBooleanArray value is an array"))?
                        .iter()
                        .map(truthy)
                        .collect(),
                )
                .unwrap(),
            "putNumberArray" => log
                .put_number_array(
                    &key(0)?,
                    number(&args[1])?,
                    args[2]
                        .as_array()
                        .ok_or_else(|| err("putNumberArray value is an array"))?
                        .iter()
                        .map(number)
                        .collect::<Result<_, String>>()?,
                )
                .unwrap(),
            "clearBeforeTime" => log.clear_before_time(number(&args[0])?, truthy(&args[1])),
            "deleteField" => log.delete_field(&key(0)?),
            "updateRangeWithTimestamp" => log.update_range_with_timestamp(number(&args[0])?),
            other => return Err(err(format!("unsupported log method {other}"))),
        }
    }
    Ok(())
}

pub struct Emit {
    pub values: Vec<(&'static str, Value)>,
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
    let controls = sel.controls();
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
                ("play", Value::Bool(controls.play_hidden)),
                ("pause", Value::Bool(controls.pause_hidden)),
                ("lock", Value::Bool(controls.lock_hidden)),
                ("unlock", Value::Bool(controls.unlock_hidden)),
                (
                    "showLockButtons",
                    Value::string(match controls.show_lock_buttons {
                        None => "",
                        Some(true) => "1",
                        Some(false) => "0",
                    }),
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

fn key_target(selector: &str) -> KeyTarget {
    if selector.starts_with("input") {
        KeyTarget::Input
    } else if selector.starts_with("textarea") {
        KeyTarget::TextArea
    } else {
        KeyTarget::Other
    }
}

fn frame(sel: &mut SelectionImpl) {
    sel.get_timeline_range();
    sel.get_hovered_time();
    sel.get_selected_time();
    sel.get_mode();
    sel.get_render_time();
    sel.get_grab_zoom_range();
    sel.get_current_live_time();
}

/// `TabController.showTimeline()` for the tab types fixtures select.
fn shows_timeline(tab: TabType) -> Result<bool, String> {
    match tab {
        TabType::Field2d
        | TabType::Field3d
        | TabType::Video
        | TabType::Joysticks
        | TabType::Swerve
        | TabType::Mechanism
        | TabType::Points => Ok(true),
        TabType::Statistics => Err(err(
            "Statistics shows the timeline only for its visible range: not modeled",
        )),
        _ => Ok(false),
    }
}

const DEFAULT_TIMELINE_RECT: [f64; 4] = [310.0, 50.0, 780.0, 32.0];

/// The `div.timeline` rect: a fixture rule naming that selector, else `layout.json`'s.
fn timeline_rect(layout: &[Json]) -> Result<[f64; 4], String> {
    for rule in layout {
        let selector = rule
            .get("selector")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        if selector.contains("timeline") && selector != "div.timeline" {
            return Err(err(format!("layout rule {selector:?} is not modeled")));
        }
        if selector == "div.timeline" {
            let rect = rule
                .get("rect")
                .and_then(Json::as_array)
                .ok_or_else(|| err("div.timeline rule without rect"))?;
            let n = |i: usize| {
                rect[i]
                    .as_f64()
                    .ok_or_else(|| err("layout rect entries are numbers"))
            };
            return Ok([n(0)?, n(1)?, n(2)?, n(3)?]);
        }
    }
    Ok(DEFAULT_TIMELINE_RECT)
}

/// `window.tabs.timeline.scrollSensor.callback(dx, dy, ...)`, the fixtures' stand-in for
/// wheel input, which jsdom cannot produce.
fn scroll_eval(expr: &str) -> Option<(f64, f64)> {
    let args = expr
        .strip_prefix("window.tabs.timeline.scrollSensor.callback(")?
        .strip_suffix(')')?;
    let mut parts = args.split(',').map(|p| p.trim().parse::<f64>());
    let dx = parts.next()?.ok()?;
    let dy = parts.next()?.ok()?;
    Some((dx, dy))
}

fn canvas_value(
    paint: &asrs_canvas::Canvas2d,
    resets: u32,
) -> Result<Vec<(&'static str, Value)>, String> {
    let calls = asrs_canvas::trace::encode(&paint.calls)
        .lines()
        .map(|line| {
            asrs_parity::json::parse(line)
                .map_err(|e| err(format!("port trace line {line}: {e}")))
                .and_then(|json| Value::from_json(&json).map_err(|e| err(e.0)))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(vec![
        ("canvas", Value::string("timeline-canvas#0")),
        ("frame", Value::number(resets as f64)),
        ("width", Value::number(paint.width as f64)),
        ("height", Value::number(paint.height as f64)),
        ("calls", Value::Array(calls)),
    ])
}

pub struct Options {
    /// Selection fixtures must not record `show`: it is hub boot, not selection.
    pub require_show_ignored: bool,
}

pub fn replay(spec_path: &Path, options: &Options) -> Result<Vec<Emit>, String> {
    let spec = load_json(spec_path)?;
    let spec_dir = spec_path.parent().unwrap();
    let fixture = spec
        .get("fixture")
        .ok_or_else(|| err("spec has no fixture"))?;
    let start = fixture
        .get("startTime")
        .and_then(Json::as_f64)
        .unwrap_or(1767225600000.0);
    let window = fixture.get("window");
    let window_number = |key: &str, default: f64| {
        window
            .and_then(|w| w.get(key))
            .and_then(Json::as_f64)
            .unwrap_or(default)
    };
    let frame_period = window_number("framePeriod", 16.0);
    let device_pixel_ratio = window_number("devicePixelRatio", 1.0);
    let light = window
        .and_then(|w| w.get("theme"))
        .and_then(Json::as_str)
        .is_none_or(|t| t.to_string_lossy() != "dark");
    let layout = fixture
        .get("layout")
        .and_then(Json::as_array)
        .unwrap_or(&[]);
    let timeline_rect = timeline_rect(layout)?;

    let clock_cell = Rc::new(Cell::new(start));
    let clock = Rc::new(ScriptedClock(clock_cell.clone()));
    let log = Rc::new(RefCell::new(Log::new(true)));
    let live_time = Rc::new(Cell::new(0.0f64));
    let mut sel = SelectionImpl::new(log.clone(), clock.clone());
    let mut timeline = Timeline::new(clock.clone());
    let mut platform = fixture
        .get("version")
        .and_then(|v| v.get("platform"))
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| "darwin".to_owned());
    let mut preferences = Some(Preferences::default());
    if let Some(prefs) = fixture.get("preferences") {
        let prefs = js_json::to_js(&Value::from_json_lenient(prefs))?;
        if let Some(p) = preferences.as_mut() {
            p.merge(&prefs)
                .map_err(|e| err(format!("fixture preferences: {e:?}")))?;
        }
    }
    if let Some(boot) = fixture.get("boot") {
        platform = String::new();
        preferences = None;
        for message in boot.as_array().ok_or_else(|| err("boot is a list"))? {
            let name = message
                .as_array()
                .and_then(|m| m.first())
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .ok_or_else(|| err("boot entries are [name, data]"))?;
            let data = message
                .as_array()
                .and_then(|m| m.get(1))
                .map(Value::from_json_lenient)
                .unwrap_or(Value::Undefined);
            match name.as_str() {
                "set-version" => {
                    platform = data
                        .get("platform")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default()
                }
                "set-preferences" => {
                    let mut p = Preferences::default();
                    p.merge(&js_json::to_js(&data)?)
                        .map_err(|e| err(format!("boot set-preferences: {e:?}")))?;
                    preferences = Some(p);
                }
                "set-assets"
                | "set-fullscreen"
                | "set-battery"
                | "show-update-button"
                | "show-feedback-button"
                | "show-when-ready"
                | "set-active-satellites" => {}
                other => return Err(err(format!("boot message {other} is not modeled"))),
            }
        }
    }
    let mut ctx = sel.context().clone();
    ctx.platform = platform;
    sel.set_context(ctx);

    let ignored: HashSet<String> = match fixture.get("ignoreMain").and_then(Json::as_array) {
        Some(names) => names
            .iter()
            .filter_map(Json::as_str)
            .map(|s| s.to_string_lossy())
            .collect(),
        None => ["save-state", "save-type-memory", "update-touch-bar-slider"]
            .map(str::to_owned)
            .into(),
    };
    if options.require_show_ignored && !ignored.contains("show") {
        return Err(err(
            "selection fixtures must list `show` in ignoreMain: it is hub boot, not selection",
        ));
    }
    if !ignored.contains("show") {
        return Err(err("the replay does not model the boot `show` message"));
    }
    let record_touch_bar = !ignored.contains("update-touch-bar-slider");

    struct Hub {
        tabs: Vec<TabType>,
        selected_tab: usize,
        timeline_hidden: bool,
        paints: u32,
        painted: Option<asrs_canvas::Canvas2d>,
    }
    let mut hub = Hub {
        tabs: vec![
            TabType::Documentation,
            TabType::LineGraph,
            TabType::Field2d,
            TabType::Field3d,
        ],
        selected_tab: 1,
        timeline_hidden: true,
        paints: 0,
        painted: None,
    };

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

    // The touch bar interval starts at boot; env.ts accumulates dues in f64.
    let mut touch_bar_due = start + touch_bar::PERIOD_MS;
    let mut last_frame = start;
    let tick_log = log.clone();
    let mut advance = |ms: f64,
                       sel: &mut SelectionImpl,
                       timeline: &mut Timeline,
                       hub: &mut Hub,
                       preferences: Option<&Preferences>,
                       emitted: &mut Vec<Emit>| {
        let end = clock_cell.get() + ms;
        loop {
            let frame_at = last_frame + frame_period;
            let help_due = timeline.next_timer();
            let next_timer = match help_due {
                Some(due) if due <= touch_bar_due => due,
                _ => touch_bar_due,
            };
            if next_timer <= end && next_timer <= frame_at {
                clock_cell.set(clock_cell.get().max(next_timer));
                if help_due == Some(next_timer) {
                    for message in timeline.run_timers() {
                        if !ignored.contains(message.name.name()) {
                            emitted.push(Emit {
                                values: vec![
                                    ("main", Value::string(message.name.name())),
                                    ("data", js_json::from_js(&message.data)),
                                ],
                            });
                        }
                    }
                    continue;
                }
                touch_bar_due += touch_bar::PERIOD_MS;
                let platform = sel.context().platform.clone();
                let position = touch_bar::tick(&platform, &tick_log.borrow(), sel);
                if let (Some(position), true) = (position, record_touch_bar) {
                    emitted.push(Emit {
                        values: vec![
                            ("main", Value::string("update-touch-bar-slider")),
                            ("data", Value::number(position)),
                        ],
                    });
                }
            } else if frame_at <= end {
                last_frame = frame_at;
                clock_cell.set(frame_at);
                let height = if hub.timeline_hidden {
                    0.0
                } else {
                    timeline_rect[3]
                };
                let frame_info = TimelineFrame {
                    width: if hub.timeline_hidden {
                        0.0
                    } else {
                        timeline_rect[2]
                    },
                    height,
                    device_pixel_ratio,
                    light,
                    preferences,
                };
                let log_ref = tick_log.clone();
                let log_ref = log_ref.borrow();
                if let Some(paint) =
                    timeline.periodic(&frame_info, sel, &log_ref, &mut FixedAdvance)
                {
                    hub.paints += 1;
                    hub.painted = Some(paint);
                }
                frame(sel);
            } else {
                clock_cell.set(end);
                break;
            }
        }
    };

    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .ok_or_else(|| err("fixture has no steps"))?;
    for (i, step) in steps.iter().enumerate() {
        let where_ = || format!("step {i}");
        let scroll_target = |hub: &Hub, preferences: Option<&Preferences>| {
            if hub.timeline_hidden {
                ScrollTarget::new(0.0, 0.0, preferences)
            } else {
                ScrollTarget::new(timeline_rect[2], timeline_rect[3], preferences)
            }
        };
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
                "setTimelineRange" => sel.set_timeline_range(pair(&arg(0)?)?, truthy(&arg(1)?)),
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
                    // A new VideoController starts unlocked.
                    ctx.unlocked_video_selected = ctx.selected_tab_type == TabType::Video;
                    hub.timeline_hidden = !shows_timeline(ctx.selected_tab_type)?;
                    hub.selected_tab += 1;
                    hub.tabs.insert(hub.selected_tab, ctx.selected_tab_type);
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
                        data.get("speed")
                            .map(number)
                            .transpose()?
                            .unwrap_or(f64::NAN),
                    );
                    sel.set_playback_looping(data.get("looping").map(truthy).unwrap_or(false));
                }
                "set-preferences" => {
                    let mut p = Preferences::default();
                    p.merge(&js_json::to_js(&data)?)
                        .map_err(|e| err(format!("{}: set-preferences: {e:?}", where_())))?;
                    preferences = Some(p);
                }
                other => return Err(format!("{}: main message {other} is not modeled", where_())),
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
            // Blink's `MouseEvent::clientX()` returns `std::floor(client_x_)`
            // (`core/events/mouse_event.h`), so a fractional init reads back floored.
            let overlay_x = math::floor(init.get("clientX").and_then(Json::as_f64).unwrap_or(0.0))
                - if hub.timeline_hidden {
                    0.0
                } else {
                    timeline_rect[0]
                };
            let on_overlay = target == "div.timeline-scroll";
            if !on_overlay && target.contains("timeline") {
                return Err(format!(
                    "{}: event target {target} is not modeled",
                    where_()
                ));
            }
            let pointer_elsewhere = matches!(target.as_str(), "window" | "document");
            match (kind.as_str(), etype.as_str()) {
                ("MouseEvent", "mousemove") if on_overlay || pointer_elsewhere => timeline
                    .mouse_move(
                        overlay_x,
                        on_overlay,
                        hub.timeline_hidden,
                        scroll_target(&hub, preferences.as_ref()),
                        &mut sel,
                    ),
                ("MouseEvent", "mouseup") if on_overlay || pointer_elsewhere => {
                    timeline.mouse_up(on_overlay, hub.timeline_hidden, &mut sel)
                }
                ("MouseEvent", "mousedown") if on_overlay => {
                    timeline.mouse_down(overlay_x, init_f64("shiftKey"), &mut sel)
                }
                ("MouseEvent", "mouseleave") if on_overlay => timeline.mouse_leave(&mut sel),
                ("MouseEvent", "click") if on_overlay => timeline.click(overlay_x, &mut sel),
                ("MouseEvent", "contextmenu") if on_overlay => timeline.context_menu(&mut sel),
                ("MouseEvent", "click") => sel.click(button_for(&target)?),
                ("MouseEvent", "contextmenu") => {
                    button_for(&target)?;
                    sel.context_menu(event_rect(&target, layout)?)
                }
                ("KeyboardEvent", "keydown") => {
                    let code = init
                        .get("code")
                        .and_then(Json::as_str)
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default();
                    sel.key_down(
                        &code,
                        init_f64("metaKey"),
                        init_f64("ctrlKey"),
                        key_target(&target),
                    );
                }
                other => return Err(format!("{}: unsupported event {other:?}", where_())),
            }
        } else if let Some(expr) = step.get("eval") {
            let expr = expr
                .as_str()
                .ok_or_else(|| err("eval is a string"))?
                .to_string_lossy();
            if let Some((dx, dy)) = scroll_eval(&expr) {
                timeline.scroll(dx, dy, scroll_target(&hub, preferences.as_ref()), &mut sel);
            } else if let Some(index) = expr
                .strip_prefix("window.tabs.setSelected(")
                .and_then(|r| r.strip_suffix(')'))
                .and_then(|i| i.parse::<usize>().ok())
            {
                let tab = *hub
                    .tabs
                    .get(index)
                    .ok_or_else(|| format!("{}: no tab {index}", where_()))?;
                if tab == TabType::Video {
                    return Err(format!(
                        "{}: selecting a video tab is not modeled",
                        where_()
                    ));
                }
                hub.selected_tab = index;
                hub.timeline_hidden = !shows_timeline(tab)?;
                let mut ctx = sel.context().clone();
                ctx.selected_tab_type = tab;
                ctx.unlocked_video_selected = false;
                sel.set_context(ctx);
            } else {
                return Err(format!("{}: unsupported eval {expr}", where_()));
            }
        } else if let Some(ms) = step.get("advance") {
            advance(
                ms.as_f64().ok_or_else(|| err("advance is a number"))?,
                &mut sel,
                &mut timeline,
                &mut hub,
                preferences.as_ref(),
                &mut emitted,
            );
        } else if let Some(n) = step.get("frames") {
            advance(
                n.as_f64().ok_or_else(|| err("frames is a number"))? * frame_period,
                &mut sel,
                &mut timeline,
                &mut hub,
                preferences.as_ref(),
                &mut emitted,
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
                    "canvas" => {
                        let selector = spec
                            .get("selector")
                            .and_then(Json::as_str)
                            .map(|s| s.to_string_lossy());
                        if selector.as_deref() != Some(".timeline-canvas") {
                            return Err(format!(
                                "{}: canvas captures must select .timeline-canvas",
                                where_()
                            ));
                        }
                        if let Some(paint) = hub.painted.take() {
                            emitted.push(Emit {
                                values: canvas_value(&paint, 2 * hub.paints)?,
                            });
                        }
                    }
                    other => return Err(format!("{}: unsupported capture kind {other}", where_())),
                }
            }
        } else {
            return Err(format!("{}: unknown step", where_()));
        }
        drain_outbox(&mut sel, &mut emitted);
    }
    Ok(emitted)
}

fn first_difference(expected: &Value, actual: &Value) -> String {
    if let (Value::Array(e), Value::Array(a)) = (expected, actual) {
        for (i, (e, a)) in e.iter().zip(a.iter()).enumerate() {
            if e != a {
                return format!(
                    "call {i}:\n  expected {}\n  actual   {}",
                    e.encode(),
                    a.encode()
                );
            }
        }
        return format!("expected {} calls, port made {}", e.len(), a.len());
    }
    format!(
        "\n  expected {}\n  actual   {}",
        expected.encode(),
        actual.encode()
    )
}

pub fn compare(spec: &Path, vector_id: &str, options: &Options) -> Result<usize, String> {
    let emitted = replay(spec, options)?;
    let records = vectors::load(vector_id)?;
    if emitted.len() != records.len() {
        return Err(format!(
            "emitted {} records, vector has {}",
            emitted.len(),
            records.len()
        ));
    }
    let mut canvases = 0;
    for (emit, record) in emitted.iter().zip(records.iter()) {
        for (key, value) in &emit.values {
            let expected = record
                .value(key)
                .map_err(|e| format!("record {} missing {key}: {e}", record.line))?;
            if !vectors::values_match(&expected, value) {
                return Err(format!(
                    "line {} ({key}): {}",
                    record.line,
                    first_difference(&expected, value)
                ));
            }
            if *key == "calls" {
                canvases += 1;
            }
        }
    }
    Ok(canvases)
}

pub fn specs(area: &str) -> Vec<(PathBuf, String)> {
    let dir = vectors::workspace_root()
        .join("parity/specs/dom")
        .join(area);
    let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.ends_with(".entry.json"))
        })
        .map(|p| {
            let name = p
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .trim_end_matches(".entry.json")
                .to_owned();
            (p, format!("dom/{area}/{name}"))
        })
        .collect();
    out.sort();
    out
}

/// Replays every spec of `dom/<area>` and panics with every mismatch; returns the number
/// of specs and the number of canvas traces compared.
pub fn check_area(area: &str, options: &Options) -> (usize, usize) {
    let specs = specs(area);
    assert!(!specs.is_empty(), "no {area} specs");
    let mut failures = Vec::new();
    let mut canvases = 0;
    for (spec, id) in &specs {
        match compare(spec, id, options) {
            Ok(n) => canvases += n,
            Err(e) => failures.push(format!("{id}: {e}")),
        }
    }
    if !failures.is_empty() {
        panic!("{}", failures.join("\n\n"));
    }
    (specs.len(), canvases)
}
