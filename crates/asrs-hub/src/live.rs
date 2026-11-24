//! The page's live data source and NT4 publisher with the timers and sockets they run on.
//!
//! Upstream's sources call back into hub.ts from inside their own methods. Here the callbacks
//! queue [`LiveUpdate`]s, which [`crate::hub::HubModel`] applies when the method returns: every
//! source calls back last, or reads nothing a callback changes before it returns.
//!
//! Each session owns its timers and its transport, so dropping it leaves nothing running. That
//! differs from upstream after `stop()`: `NT4Source` keeps its idle loop, which then only draws
//! subscription ids, and `FTCDashboardSource` keeps its socket, its pings and its reconnects.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use asrs_formats::worker::Abort;
use asrs_js::string::JsString;
use asrs_js::time::Clock;
use asrs_js::value::JsValue;
use asrs_live::ftc::{FtcContext, FtcSource};
use asrs_live::nt4::{
    NetEvent, Nt4Context, Nt4Mode, Nt4Publisher, Nt4PublisherStatus, Nt4Source, Nt4Transport,
    Nt4Tuner, Nt4Window, PublisherPreferences, PublisherSelection, PublisherWindow,
};
use asrs_live::phoenix::{PhoenixContext, PhoenixSource, PhoenixWindow};
use asrs_live::rlog::{MainCall, RlogContext, RlogPreferences, RlogSource};
use asrs_live::{
    LiveDataSourceStatus, LiveLog, LivePreferences, OutputCallback, StatusCallback, Thrown,
    TimeSupplier, TimerId, TimerQueue, TunedValue,
};
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::Selection;
use asrs_model::selection_impl::{SelectionImpl, SharedLog};

use crate::sidebar::{Tuner, TunerWindow};

/// The `WebSocket` and `fetch` a live source or the publisher reaches.
pub type LiveNet = Rc<RefCell<dyn Nt4Transport>>;

/// Which of the page's two connections a transport serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LiveRole {
    /// The live source. With `whole_bodies`, its `fetch` settles after the response body.
    Source {
        whole_bodies: bool,
    },
    Publisher,
}

/// `liveSource = new ...` in `startLive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveSourceKind {
    Nt4,
    Nt4AdvantageKit,
    Nt4Systemcore,
    Nt4DriverStation,
    Phoenix,
    Rlog,
    FtcDashboard,
}

impl LiveSourceKind {
    /// The transport the source needs; RLOG's socket is main's.
    pub fn role(self) -> Option<LiveRole> {
        match self {
            Self::Rlog => None,
            kind => Some(LiveRole::Source {
                whole_bodies: kind == Self::Phoenix,
            }),
        }
    }

    fn nt4_mode(self) -> Option<Nt4Mode> {
        match self {
            Self::Nt4 => Some(Nt4Mode::Default),
            Self::Nt4AdvantageKit => Some(Nt4Mode::AdvantageKit),
            Self::Nt4Systemcore => Some(Nt4Mode::Systemcore),
            Self::Nt4DriverStation => Some(Nt4Mode::DriverStation),
            Self::Phoenix | Self::Rlog | Self::FtcDashboard => None,
        }
    }
}

pub(crate) enum LiveUpdate {
    Status(LiveDataSourceStatus),
    Output(LiveLog, TimeSupplier),
    Publisher(Nt4PublisherStatus),
    Main(NamedMessage<HubToMain>),
}

type Updates = Rc<RefCell<VecDeque<LiveUpdate>>>;

/// `window` as the live sources see it.
pub(crate) struct LiveWindow<'a> {
    pub log: &'a SharedLog,
    pub selection: &'a mut SelectionImpl,
    pub preferences: Option<&'a Preferences>,
    pub realm: &'a mut JsRealm,
    pub clock: &'a dyn Clock,
    /// `process.platform`.
    pub platform: &'a str,
    /// The active fields [`LiveSession::fields_read`] asked for.
    pub fields: Vec<JsString>,
}

impl LiveWindow<'_> {
    fn live_preferences(&self) -> Option<LivePreferences> {
        self.preferences.map(|p| LivePreferences {
            live_discard: Some(p.live_discard),
            live_subscribe_mode: Some(JsString::from(p.live_subscribe_mode.as_str())),
        })
    }

    fn nt4(&mut self, updates: &Updates) -> Nt4Window {
        let updates = updates.clone();
        Nt4Window {
            log: self.log.clone(),
            preferences: self.live_preferences(),
            tabs_fields: std::mem::take(&mut self.fields),
            sidebar_fields: Vec::new(),
            send_main_message: Box::new(move |name, data| {
                if let Some(name) = HubToMain::from_name(name) {
                    let message = NamedMessage::new(name, data);
                    updates.borrow_mut().push_back(LiveUpdate::Main(message));
                }
            }),
        }
    }
}

/// Which of the page's active fields handling a timer reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldsRead {
    None,
    Tabs,
    TabsAndSidebar,
}

enum Source {
    Nt4(Nt4Source, LiveNet),
    Phoenix(PhoenixSource, LiveNet),
    Rlog(RlogSource),
    Ftc(FtcSource, LiveNet),
}

pub(crate) struct LiveSession {
    timers: TimerQueue,
    source: Source,
    updates: Updates,
}

fn callbacks(updates: &Updates) -> (StatusCallback, OutputCallback) {
    let status = updates.clone();
    let output = updates.clone();
    (
        Box::new(move |s| status.borrow_mut().push_back(LiveUpdate::Status(s))),
        Box::new(move |log, supplier| {
            if let Some(log) = log {
                let mut output = output.borrow_mut();
                output.push_back(LiveUpdate::Output(log, supplier));
            }
        }),
    )
}

fn rlog_preferences(window: &LiveWindow<'_>) -> Option<RlogPreferences> {
    window.preferences.map(|p| RlogPreferences {
        rlog_port: p.rlog_port,
    })
}

/// Runs `f` as an RLOG source method and queues the messages it sends main.
fn rlog<R>(
    source: &mut RlogSource,
    timers: &mut TimerQueue,
    updates: &Updates,
    window: &mut LiveWindow<'_>,
    f: impl FnOnce(&mut RlogSource, &mut RlogContext<'_>) -> R,
) -> R {
    let preferences = rlog_preferences(window);
    let window_log = Rc::new(RefCell::new(window.log.clone()));
    let mut calls = VecDeque::new();
    let result = f(
        source,
        &mut RlogContext {
            timers,
            send_main: &mut calls,
            preferences: preferences.as_ref(),
            window_log: &window_log,
            realm: window.realm,
            clock: window.clock,
        },
    );
    let mut updates = updates.borrow_mut();
    updates.extend(
        calls
            .into_iter()
            .map(|call| LiveUpdate::Main(rlog_message(call))),
    );
    result
}

pub(crate) fn rlog_message(call: MainCall) -> NamedMessage<HubToMain> {
    match call {
        call @ MainCall::Start { .. } => {
            NamedMessage::new(HubToMain::LiveRlogStart, call.into_data())
        }
        MainCall::Stop => NamedMessage::without_data(HubToMain::LiveRlogStop),
    }
}

impl LiveSession {
    /// `liveSource = new ...; liveSource.connect(...)`. `net` serves [`LiveSourceKind::role`];
    /// without one the kind is RLOG.
    pub(crate) fn connect(
        kind: LiveSourceKind,
        address: &JsString,
        net: Option<LiveNet>,
        timer_clock: Rc<dyn Clock>,
        date: Rc<dyn Clock>,
        window: &mut LiveWindow<'_>,
    ) -> Self {
        let mut timers = TimerQueue::new(timer_clock);
        let updates = Updates::default();
        let (status, output) = callbacks(&updates);
        let address = address.to_string_lossy();
        let preferences = window.live_preferences();
        let source = match (net, kind.nt4_mode()) {
            (Some(net), Some(mode)) => {
                let mut source = Nt4Source::new(mode, &mut *window.realm.random, date, &mut timers);
                source.connect(
                    &address,
                    status,
                    output,
                    preferences.as_ref(),
                    &mut Nt4Context {
                        timers: &mut timers,
                        net: &mut *net.borrow_mut(),
                        clock: window.clock,
                        realm: window.realm,
                    },
                );
                Source::Nt4(source, net)
            }
            (Some(net), None) if kind == LiveSourceKind::Phoenix => {
                let mut source = PhoenixSource::new(&mut *window.realm.random, date);
                source.connect(
                    &address,
                    status,
                    output,
                    &mut PhoenixContext {
                        timers: &mut timers,
                        net: &mut *net.borrow_mut(),
                    },
                );
                Source::Phoenix(source, net)
            }
            (Some(net), None) => {
                let mut source = FtcSource::new(&mut *window.realm.random, date);
                source.connect(
                    &address,
                    status,
                    output,
                    preferences.as_ref(),
                    &mut FtcContext {
                        timers: &mut timers,
                        net: &mut *net.borrow_mut(),
                        clock: window.clock,
                    },
                );
                Source::Ftc(source, net)
            }
            (None, _) => {
                let mut source = RlogSource::new(&mut *window.realm.random, date);
                rlog(&mut source, &mut timers, &updates, window, |source, cx| {
                    source.connect(&address, status, output, cx);
                });
                Source::Rlog(source)
            }
        };
        LiveSession {
            timers,
            source,
            updates,
        }
    }

    pub(crate) fn is_rlog(&self) -> bool {
        matches!(self.source, Source::Rlog(_))
    }

    pub(crate) fn take_updates(&self) -> VecDeque<LiveUpdate> {
        self.updates.take()
    }

    pub(crate) fn next_due(&self) -> Option<f64> {
        self.timers.next_due()
    }

    pub(crate) fn fire_next(&mut self) -> Option<TimerId> {
        self.timers.fire_next()
    }

    pub(crate) fn fields_read(&self, id: TimerId) -> FieldsRead {
        match &self.source {
            Source::Nt4(source, _) if source.reads_fields(id) => FieldsRead::TabsAndSidebar,
            Source::Phoenix(source, _) if source.reads_fields(id) => FieldsRead::Tabs,
            _ => FieldsRead::None,
        }
    }

    pub(crate) fn handle_timer(&mut self, id: TimerId, window: &mut LiveWindow<'_>) {
        let timers = &mut self.timers;
        match &mut self.source {
            Source::Nt4(source, net) => {
                let mut nt4 = window.nt4(&self.updates);
                let mut cx = Nt4Context {
                    timers,
                    net: &mut *net.borrow_mut(),
                    clock: window.clock,
                    realm: window.realm,
                };
                source.handle_timer(id, &mut nt4, &mut cx);
            }
            Source::Phoenix(source, net) => {
                let phoenix = PhoenixWindow {
                    tabs_fields: std::mem::take(&mut window.fields),
                    preferences: window.live_preferences(),
                };
                let net = &mut *net.borrow_mut();
                source.handle_timer(id, &phoenix, &mut PhoenixContext { timers, net });
            }
            Source::Rlog(source) => {
                rlog(source, timers, &self.updates, window, |source, cx| {
                    source.handle_timer(id, cx);
                });
            }
            Source::Ftc(source, net) => {
                let preferences = window.live_preferences();
                let mut cx = FtcContext {
                    timers,
                    net: &mut *net.borrow_mut(),
                    clock: window.clock,
                };
                source.handle_timer(id, preferences.as_ref(), &mut cx);
            }
        }
    }

    /// A throw out of a socket listener is the browser's to report; nothing on the page reads it.
    pub(crate) fn handle_net_event(&mut self, event: &NetEvent, window: &mut LiveWindow<'_>) {
        let timers = &mut self.timers;
        timers.begin_task();
        match &mut self.source {
            Source::Nt4(source, net) => {
                let mut cx = Nt4Context {
                    timers,
                    net: &mut *net.borrow_mut(),
                    clock: window.clock,
                    realm: window.realm,
                };
                let _ = source.handle_net_event(event, &mut cx);
            }
            Source::Phoenix(source, net) => {
                if let NetEvent::Fetch(event) = event {
                    let net = &mut *net.borrow_mut();
                    source.on_fetch_event(event, &mut PhoenixContext { timers, net });
                }
            }
            Source::Rlog(_) => {}
            Source::Ftc(source, net) => {
                let mut cx = FtcContext {
                    timers,
                    net: &mut *net.borrow_mut(),
                    clock: window.clock,
                };
                source.handle_net_event(event, &mut cx);
            }
        }
    }

    /// `liveSource.handleMainMessage(data)`, which only RLOG overrides.
    pub(crate) fn handle_main_message(
        &mut self,
        data: &JsValue,
        window: &mut LiveWindow<'_>,
    ) -> Result<(), Abort> {
        self.timers.begin_task();
        match &mut self.source {
            Source::Rlog(source) => rlog(
                source,
                &mut self.timers,
                &self.updates,
                window,
                |source, cx| source.handle_main_message(data, cx),
            ),
            _ => Ok(()),
        }
    }

    pub(crate) fn stop(&mut self, window: &mut LiveWindow<'_>) {
        let timers = &mut self.timers;
        match &mut self.source {
            Source::Nt4(source, net) => {
                let _ = source.stop(&mut Nt4Context {
                    timers,
                    net: &mut *net.borrow_mut(),
                    clock: window.clock,
                    realm: window.realm,
                });
            }
            Source::Phoenix(source, net) => {
                let net = &mut *net.borrow_mut();
                source.stop(&mut PhoenixContext { timers, net });
            }
            Source::Rlog(source) => {
                rlog(source, timers, &self.updates, window, RlogSource::stop);
            }
            Source::Ftc(source, net) => source.stop(&mut FtcContext {
                timers,
                net: &mut *net.borrow_mut(),
                clock: window.clock,
            }),
        }
    }

    /// `liveSource.getTuner()`.
    pub(crate) fn tuner(session: &Rc<RefCell<LiveSession>>) -> Option<Box<dyn Tuner>> {
        let nt4 = match &session.borrow().source {
            Source::Nt4(source, _) => Some(source.tuner().ok()?),
            Source::Ftc(..) => None,
            Source::Phoenix(..) | Source::Rlog(_) => return None,
        };
        Some(Box::new(LiveTuner {
            session: session.clone(),
            nt4,
        }))
    }
}

struct LiveTuner {
    session: Rc<RefCell<LiveSession>>,
    nt4: Option<Nt4Tuner>,
}

impl LiveTuner {
    fn ftc<R>(&self, f: impl FnOnce(&FtcSource) -> R) -> Option<R> {
        match &self.session.borrow().source {
            Source::Ftc(source, _) => Some(f(source)),
            _ => None,
        }
    }
}

impl Tuner for LiveTuner {
    fn has_tunable_fields(&self, log: &Log) -> bool {
        match &self.nt4 {
            Some(tuner) => tuner.has_tunable_fields(log),
            None => self.ftc(FtcSource::has_tunable_fields).unwrap_or(false),
        }
    }

    fn is_tunable(&self, log: &Log, key: &JsString) -> bool {
        match &self.nt4 {
            Some(tuner) => tuner.is_tunable(log, key),
            None => self.ftc(|s| s.is_tunable(key)).unwrap_or(false),
        }
    }

    fn publish(
        &self,
        window: &mut TunerWindow<'_>,
        key: &JsString,
        value: TunedValue,
    ) -> Result<(), Thrown> {
        let LiveSession { timers, source, .. } = &mut *self.session.borrow_mut();
        match (&self.nt4, source) {
            (Some(tuner), Source::Nt4(_, net)) => tuner.publish(
                window.log,
                key,
                value,
                &mut Nt4Context {
                    timers,
                    net: &mut *net.borrow_mut(),
                    clock: window.clock,
                    realm: window.realm,
                },
            ),
            (None, Source::Ftc(source, net)) => source.publish(
                key,
                value,
                &mut FtcContext {
                    timers,
                    net: &mut *net.borrow_mut(),
                    clock: window.clock,
                },
            ),
            _ => Ok(()),
        }
    }

    fn unpublish(&self, window: &mut TunerWindow<'_>, key: &JsString) -> Result<(), Thrown> {
        let LiveSession { timers, source, .. } = &mut *self.session.borrow_mut();
        match (&self.nt4, source) {
            (Some(tuner), Source::Nt4(_, net)) => tuner.unpublish(
                key,
                &mut Nt4Context {
                    timers,
                    net: &mut *net.borrow_mut(),
                    clock: window.clock,
                    realm: window.realm,
                },
            ),
            (None, Source::Ftc(source, _)) => {
                source.unpublish(key);
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

struct WindowSelection<'a>(&'a mut SelectionImpl);

impl PublisherSelection for WindowSelection<'_> {
    fn get_hovered_time(&self) -> Option<f64> {
        self.0.get_hovered_time()
    }

    fn get_selected_time(&mut self) -> Option<f64> {
        self.0.get_selected_time()
    }
}

pub(crate) struct PublisherSession {
    timers: TimerQueue,
    net: LiveNet,
    publisher: Nt4Publisher,
    updates: Updates,
}

/// Runs `f` with the publisher's `window`.
fn publisher_window<R>(
    window: &mut LiveWindow<'_>,
    f: impl FnOnce(&mut PublisherWindow<'_>, &mut JsRealm, &dyn Clock) -> R,
) -> R {
    let preferences = window.preferences.map(|p| PublisherPreferences {
        publish_filter: p.publish_filter.clone(),
        robot_address: p.robot_address.clone(),
        systemcore_static_address: JsString::from(p.systemcore_static_address.as_str()),
    });
    let mut selection = WindowSelection(window.selection);
    f(
        &mut PublisherWindow {
            log: window.log,
            preferences: preferences.as_ref(),
            platform: window.platform,
            selection: &mut selection,
        },
        window.realm,
        window.clock,
    )
}

impl PublisherSession {
    /// `new NT4Publisher(isSim, ...)`.
    pub(crate) fn start(
        is_sim: bool,
        net: LiveNet,
        timer_clock: Rc<dyn Clock>,
        window: &mut LiveWindow<'_>,
    ) -> Self {
        let mut timers = TimerQueue::new(timer_clock);
        let updates = Updates::default();
        let status = updates.clone();
        let publisher = publisher_window(window, |window, realm, clock| {
            Nt4Publisher::new(
                is_sim,
                Box::new(move |s| status.borrow_mut().push_back(LiveUpdate::Publisher(s))),
                window,
                &mut Nt4Context {
                    timers: &mut timers,
                    net: &mut *net.borrow_mut(),
                    clock,
                    realm,
                },
            )
        });
        PublisherSession {
            timers,
            net,
            publisher,
            updates,
        }
    }

    pub(crate) fn take_updates(&self) -> VecDeque<LiveUpdate> {
        self.updates.take()
    }

    pub(crate) fn next_due(&self) -> Option<f64> {
        self.timers.next_due()
    }

    pub(crate) fn fire_next(&mut self) -> Option<TimerId> {
        self.timers.fire_next()
    }

    pub(crate) fn handle_timer(&mut self, id: TimerId, window: &mut LiveWindow<'_>) {
        let (publisher, net, timers) = (&mut self.publisher, &self.net, &mut self.timers);
        publisher_window(window, |window, realm, clock| {
            let mut cx = Nt4Context {
                timers,
                net: &mut *net.borrow_mut(),
                clock,
                realm,
            };
            publisher.handle_timer(id, window, &mut cx);
        });
    }

    pub(crate) fn handle_net_event(&mut self, event: &NetEvent, window: &mut LiveWindow<'_>) {
        self.timers.begin_task();
        let mut cx = Nt4Context {
            timers: &mut self.timers,
            net: &mut *self.net.borrow_mut(),
            clock: window.clock,
            realm: window.realm,
        };
        let _ = self.publisher.handle_net_event(event, &mut cx);
    }

    pub(crate) fn stop(&mut self, window: &mut LiveWindow<'_>) {
        let mut cx = Nt4Context {
            timers: &mut self.timers,
            net: &mut *self.net.borrow_mut(),
            clock: window.clock,
            realm: window.realm,
        };
        let _ = self.publisher.stop(&mut cx);
    }
}
