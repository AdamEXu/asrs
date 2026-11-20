use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::time::{Clock, SetRng};
use asrs_live::{
    LiveContext, LiveDataSource, LiveDataSourceBase, LiveDataSourceStatus, LivePreferences,
    ManualClock, OutputCallback, StatusCallback, TimeSupplier, TimerQueue,
};
use asrs_log::log::Log;
use asrs_parity::{Response, Value};

use crate::functions::{self, arg};
use crate::log_field::{array, js_string, number, numbers};
use crate::log_run::dump;

/// The oracle's `ScriptedSource`: a subclass that only exposes the base.
struct ScriptedSource {
    base: LiveDataSourceBase,
    enable_clear_data: bool,
}

impl LiveDataSource for ScriptedSource {
    fn base(&self) -> &LiveDataSourceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut LiveDataSourceBase {
        &mut self.base
    }

    fn connect(
        &mut self,
        address: &str,
        status_callback: StatusCallback,
        output_callback: OutputCallback,
        cx: &mut LiveContext<'_>,
    ) {
        self.base.connect(
            address,
            status_callback,
            output_callback,
            self.enable_clear_data,
            cx.timers,
        );
    }
}

fn status_value(status: LiveDataSourceStatus) -> Value {
    Value::number(f64::from(status as u8))
}

fn supplier(spec: &Value, clock: &ManualClock) -> Result<TimeSupplier, String> {
    let spec = spec.as_array().ok_or("supplier must be an array")?;
    let kind = js_string(&arg(spec, 0))?;
    let value = number(&arg(spec, 1))?;
    match kind.to_string_lossy().as_str() {
        "wall" => {
            let clock = clock.clone();
            Ok(Rc::new(move || clock.now() / 1000.0 - value))
        }
        "const" => Ok(Rc::new(move || value)),
        other => Err(format!("liveBaseRun: unknown supplier {other:?}")),
    }
}

fn run(args: &[Value]) -> Result<Value, String> {
    let random = array(&arg(args, 0), number)?;
    let clock = ManualClock::new(number(&arg(args, 1))?);
    let mut timers = TimerQueue::new(Rc::new(clock.clone()));
    let mut preferences: Option<LivePreferences> = None;
    let records = Rc::new(RefCell::new(Vec::new()));

    let mut source = ScriptedSource {
        base: LiveDataSourceBase::new(&mut SetRng::new(random)),
        enable_clear_data: true,
    };
    records.borrow_mut().push(Value::Array(vec![
        Value::string("uuid"),
        Value::string(source.base().uuid()),
    ]));

    let ops = arg(args, 2);
    for op in ops.as_array().ok_or("ops must be an array")? {
        let op = op.as_array().ok_or("an op must be an array")?;
        let name = js_string(&arg(op, 0))?.to_string_lossy();
        match name.as_str() {
            "connect" => {
                let status_records = records.clone();
                let status_clock = clock.clone();
                let status_callback: StatusCallback = Box::new(move |status| {
                    status_records.borrow_mut().push(Value::Array(vec![
                        Value::string("status"),
                        status_value(status),
                        Value::number(status_clock.now()),
                    ]));
                });
                let output_records = records.clone();
                let output_clock = clock.clone();
                let output_callback: OutputCallback = Box::new(move |log, time_supplier| {
                    output_records.borrow_mut().push(Value::Array(vec![
                        Value::string("output"),
                        Value::number(output_clock.now()),
                        Value::Bool(log.is_none()),
                        Value::number(time_supplier()),
                    ]));
                });
                let address = js_string(&arg(op, 1))?.to_string_lossy();
                source.enable_clear_data = match arg(op, 2) {
                    Value::Undefined => true,
                    other => other.as_bool().ok_or("enableClearData must be a boolean")?,
                };
                let mut cx = LiveContext {
                    timers: &mut timers,
                    preferences: preferences.as_ref(),
                };
                source.connect(&address, status_callback, output_callback, &mut cx);
            }
            "setStatus" => {
                let status = LiveDataSourceStatus::from_index(number(&arg(op, 1))? as u8)
                    .ok_or("unknown status")?;
                source.base_mut().set_status(status);
            }
            "newLog" => source
                .base_mut()
                .set_log(Some(Rc::new(RefCell::new(Log::new(true))))),
            "dropLog" => source.base_mut().set_log(None),
            "put" => {
                if let Some(log) = source.base().log() {
                    let key = js_string(&arg(op, 1))?;
                    log.borrow_mut()
                        .put_number(&key, number(&arg(op, 2))?, number(&arg(op, 3))?);
                }
            }
            "output" => {
                let time_supplier = supplier(&arg(op, 1), &clock)?;
                source.base_mut().output(time_supplier);
            }
            "prefs" => {
                preferences = match number(&arg(op, 1))? as u8 {
                    0 => None,
                    1 => Some(LivePreferences::default()),
                    _ => Some(LivePreferences {
                        live_discard: Some(number(&arg(op, 2))?),
                    }),
                };
            }
            "advance" => {
                let target = clock.now() + number(&arg(op, 1))?;
                timers.advance_to(&clock, target, |timers, id| {
                    let mut cx = LiveContext {
                        timers,
                        preferences: preferences.as_ref(),
                    };
                    source.handle_timer(id, &mut cx);
                });
            }
            "stop" => {
                let mut cx = LiveContext {
                    timers: &mut timers,
                    preferences: preferences.as_ref(),
                };
                source.stop(&mut cx);
            }
            "state" => records.borrow_mut().push(Value::Array(vec![
                Value::string("state"),
                status_value(source.base().status()),
                source.base().address().map_or(Value::Null, Value::string),
            ])),
            "range" => {
                let range = source.base().log().map_or(Value::Null, |log| {
                    numbers(&log.borrow().get_timestamp_range())
                });
                records
                    .borrow_mut()
                    .push(Value::Array(vec![Value::string("range"), range]));
            }
            "dump" => {
                let state = source
                    .base()
                    .log()
                    .map_or(Value::Null, |log| dump(&log.borrow()));
                records
                    .borrow_mut()
                    .push(Value::Array(vec![Value::string("dump"), state]));
            }
            other => return Err(format!("liveBaseRun: unknown op {other:?}")),
        }
    }
    Ok(Value::Array(records.take()))
}

fn live_base_run(args: &[Value]) -> Response {
    match run(args) {
        Ok(v) => Response::Ok(v),
        Err(e) => Response::Error(e),
    }
}

pub const FUNCTIONS: &[(&str, functions::Function)] = &[("oracle:liveBaseRun", live_base_run)];

#[cfg(test)]
mod tests {
    use asrs_parity::vectors::check_fn_vectors;

    #[test]
    fn live_base_vectors() {
        check_fn_vectors("live/base", |case| super::live_base_run(&case.request.args));
    }
}
