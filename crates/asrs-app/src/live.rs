//! Live connections: the app's one tokio runtime, how the foreground thread is woken for what
//! arrives on it, and main's half of the RLOG source.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_js::time::Clock;
use asrs_live::rlog::{MainCall, MainContext, RlogMain, TokioRlogTransport};
use asrs_live::{LiveRuntime, NetOptions, RuntimeHandle, TimerQueue};
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_ui::gpui::{
    App, AppContext, AsyncApp, BackgroundExecutor, Context, Entity, Global, Task, WeakEntity,
};

use crate::hub::{Hub, HubEvent};

struct Runtime(LiveRuntime);

impl Global for Runtime {}

/// The runtime every live connection of the app shares, started by the first of them.
pub(crate) fn runtime(cx: &mut App) -> Result<RuntimeHandle, String> {
    if !cx.has_global::<Runtime>() {
        let runtime = LiveRuntime::new().map_err(|e| format!("starting the live runtime: {e}"))?;
        cx.set_global(Runtime(runtime));
    }
    Ok(cx.global::<Runtime>().0.handle().clone())
}

/// Connections, requests and closing handshakes the app's live runtime still runs.
pub fn runtime_tasks(cx: &App) -> usize {
    cx.try_global::<Runtime>()
        .map_or(0, |r| r.0.alive_tasks())
}

/// Milliseconds since `origin` on the executor's clock, which is simulated under a test
/// dispatcher.
pub(crate) struct ExecutorClock {
    executor: BackgroundExecutor,
    origin: Instant,
}

impl ExecutorClock {
    pub(crate) fn new(origin: Instant, cx: &App) -> Self {
        ExecutorClock {
            executor: cx.background_executor().clone(),
            origin,
        }
    }
}

impl Clock for ExecutorClock {
    fn now(&self) -> f64 {
        let elapsed = self.executor.now().saturating_duration_since(self.origin);
        elapsed.as_nanos() as f64 / 1e6
    }
}

/// One executor timer, kept set to when a [`TimerQueue`]'s next timer is due.
#[derive(Default)]
pub(crate) struct Alarm {
    due: Option<f64>,
    task: Option<Task<()>>,
}

impl Alarm {
    pub(crate) fn clear(&mut self) {
        self.due = None;
    }

    pub(crate) fn arm<T: 'static>(
        &mut self,
        due: Option<f64>,
        now: f64,
        cx: &Context<'_, T>,
        fire: impl FnOnce(&mut T, &mut Context<'_, T>) + 'static,
    ) {
        if self.due == due {
            return;
        }
        self.due = due;
        self.task = due.map(|due| {
            let wait = Duration::from_nanos(((due - now) * 1e6).ceil().max(0.0) as u64);
            let timer = cx.background_executor().timer(wait);
            cx.spawn(async move |this, cx| {
                timer.await;
                let _ = this.update(cx, fire);
            })
        });
    }
}

/// How long the foreground thread handles arrivals before it lets input and frames through.
const SLICE: Duration = Duration::from_millis(4);
/// A timer, not a bare yield: on Windows posted messages run ahead of input and paint.
const YIELD: Duration = Duration::from_millis(1);

#[derive(Clone, Copy)]
pub(crate) struct Slice(Instant);

impl Slice {
    pub(crate) fn is_over(self) -> bool {
        self.0.elapsed() >= SLICE
    }
}

/// Calls `drain` whenever `ready` says something arrived, again after a pause while it reports
/// more is waiting, until it reports that the connection is gone.
pub(crate) async fn pump(
    ready: async_channel::Receiver<()>,
    cx: &mut AsyncApp,
    mut drain: impl FnMut(Slice, &mut AsyncApp) -> Option<bool>,
) {
    while ready.recv().await.is_ok() {
        loop {
            match drain(Slice(Instant::now()), cx) {
                None => return,
                Some(false) => break,
                Some(true) => cx.background_executor().timer(YIELD).await,
            }
        }
    }
}

/// Main's RLOG sockets for one hub window: `live-rlog-start` and `live-rlog-stop` from the hub,
/// `live-data` to it.
pub struct RlogBridge {
    hub: WeakEntity<Hub>,
    main: RlogMain,
    timers: TimerQueue,
    clock: Rc<ExecutorClock>,
    net: TokioRlogTransport,
    alarm: Alarm,
    _pump: Task<()>,
}

/// Answers `hub`'s RLOG messages as main does, until the hub is released.
pub fn attach(hub: &Entity<Hub>, cx: &mut App) {
    let mut bridge: Option<Entity<RlogBridge>> = None;
    cx.subscribe(hub, move |hub, event, cx| {
        let HubEvent::Main(message) = event else {
            return;
        };
        let Some(call) = MainCall::parse(message.name.name(), &message.data) else {
            return;
        };
        if bridge.is_none() && call != MainCall::Stop {
            match RlogBridge::open(&hub, cx) {
                Ok(opened) => bridge = Some(opened),
                Err(e) => eprintln!("asrs: live-rlog-start: {e}"),
            }
        }
        if let Some(bridge) = &bridge {
            bridge.update(cx, |bridge, cx| bridge.call(&call, cx));
        }
    })
    .detach();
}

impl RlogBridge {
    fn open(hub: &Entity<Hub>, cx: &mut App) -> Result<Entity<Self>, String> {
        let handle = runtime(cx)?;
        let (wake, ready) = async_channel::bounded::<()>(1);
        let posted = hub.read(cx).live_posted();
        let options = NetOptions {
            handle,
            notify: Arc::new(move || {
                let _ = wake.try_send(());
                let _ = posted.try_send(());
            }),
            dial: hub.read(cx).dial(),
        };
        let hub = hub.downgrade();
        Ok(cx.new(|cx| {
            let clock = Rc::new(ExecutorClock::new(cx.background_executor().now(), cx));
            let mut timers = TimerQueue::new(clock.clone());
            let pump = cx.spawn(async move |this, cx| {
                pump(ready, cx, |slice, cx| {
                    while this.update(cx, Self::step).ok()? {
                        if slice.is_over() {
                            return Some(true);
                        }
                    }
                    Some(false)
                })
                .await;
            });
            RlogBridge {
                hub,
                main: RlogMain::new(&mut timers),
                timers,
                clock,
                net: TokioRlogTransport::new(options),
                alarm: Alarm::default(),
                _pump: pump,
            }
        }))
    }

    fn call(&mut self, call: &MainCall, cx: &mut Context<'_, Self>) {
        self.fire_timers();
        self.timers.begin_task();
        self.main.handle(
            call,
            &mut MainContext {
                timers: &mut self.timers,
                net: &mut self.net,
            },
        );
        self.arm(cx);
    }

    /// Delivers one waiting `live-data`, or else handles one socket event. Each runs in its own
    /// task, so a message the hub sends back is handled before the next.
    fn step(&mut self, cx: &mut Context<'_, Self>) -> bool {
        if let Some(message) = self.main.outbox.pop_front() {
            let message = NamedMessage::new(MainToHub::LiveData, message.into_value());
            let _ = self
                .hub
                .update(cx, |hub, cx| hub.handle_main_message(message, cx));
            return true;
        }
        let Some(event) = self.net.next_event() else {
            return false;
        };
        self.timers.begin_task();
        self.main.handle_net_event(
            &event,
            &mut MainContext {
                timers: &mut self.timers,
                net: &mut self.net,
            },
        );
        self.arm(cx);
        true
    }

    fn fire_timers(&mut self) {
        while let Some(id) = self.timers.fire_next() {
            self.main.handle_timer(
                id,
                &mut MainContext {
                    timers: &mut self.timers,
                    net: &mut self.net,
                },
            );
        }
    }

    /// Without a socket the heartbeat and the watchdog do nothing, so the thread stays asleep;
    /// [`Self::call`] first runs what came due in the meantime.
    fn arm(&mut self, cx: &mut Context<'_, Self>) {
        let due = self.main.has_socket().then(|| self.timers.next_due());
        self.alarm
            .arm(due.flatten(), self.clock.now(), cx, |bridge: &mut Self, cx| {
                bridge.alarm.clear();
                bridge.fire_timers();
                bridge.arm(cx);
            });
    }
}
