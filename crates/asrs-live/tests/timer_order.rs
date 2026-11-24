//! [`TimerQueue`] against `live/net/timers`, the order Electron's renderer ran timers due at the
//! same time in.

use std::rc::Rc;

use asrs_live::{ManualClock, TimerId, TimerQueue};
use asrs_parity::json::Json;
use asrs_parity::vectors;

struct Timer {
    name: String,
    kind: String,
    delay: f64,
    repeats: Option<usize>,
    /// Created when the named timer has fired this often, or at the start.
    after: Option<(String, usize)>,
}

fn text(json: &Json) -> String {
    json.as_str().expect("a string").to_string_lossy()
}

fn number(json: &Json) -> f64 {
    json.as_f64().expect("a number")
}

fn timer(json: &Json) -> Timer {
    let field = |key| json.get(key).expect("a timer field");
    Timer {
        name: text(field("name")),
        kind: text(field("kind")),
        delay: number(field("delay")),
        repeats: json.get("repeats").map(|n| number(n) as usize),
        after: json.get("after").map(|after| {
            let after = after.as_array().expect("a list");
            (text(&after[0]), number(&after[1]) as usize)
        }),
    }
}

fn create(
    queue: &mut TimerQueue,
    timers: &[Timer],
    after: Option<(&str, usize)>,
    live: &mut Vec<(TimerId, usize)>,
) {
    for (i, timer) in timers.iter().enumerate() {
        if timer.after.as_ref().map(|(name, n)| (name.as_str(), *n)) != after {
            continue;
        }
        let id = match timer.kind.as_str() {
            "timeout" => queue.set_timeout(timer.delay),
            "interval" => queue.set_interval(timer.delay),
            "abort" => queue.post_delayed_task(timer.delay),
            other => panic!("{other}"),
        };
        live.push((id, i));
    }
}

fn order(timers: &[Timer]) -> Vec<String> {
    let clock = ManualClock::new(0.0);
    let mut queue = TimerQueue::new(Rc::new(clock.clone()));
    let mut live = Vec::new();
    let mut fired: Vec<String> = Vec::new();
    create(&mut queue, timers, None, &mut live);
    queue.advance_to(&clock, 10000.0, |queue, id| {
        let (_, i) = *live.iter().find(|(live, _)| *live == id).expect("created");
        let timer = &timers[i];
        fired.push(timer.name.clone());
        let count = fired.iter().filter(|name| **name == timer.name).count();
        if timer.repeats == Some(count) {
            queue.clear(id);
        }
        create(queue, timers, Some((&timer.name, count)), &mut live);
    });
    fired
}

#[test]
fn timers_due_together_run_in_the_recorded_order() {
    let records = vectors::load("live/net/timers").unwrap();
    assert!(records.len() >= 30);
    let mut failures = Vec::new();
    for record in &records {
        let list = record.field("timers").and_then(Json::as_array).unwrap();
        let timers: Vec<Timer> = list.iter().map(timer).collect();
        let recorded = record.field("order").and_then(Json::as_array).unwrap();
        let recorded: Vec<String> = recorded.iter().map(text).collect();
        let got = order(&timers);
        if got != recorded {
            failures.push(format!("{}: ran {}", record.text, got.join(" ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
