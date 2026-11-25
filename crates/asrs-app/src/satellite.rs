//! A satellite window's page as a GPUI entity: `asrs_hub`'s headless [`SatelliteModel`] and the
//! one interval it runs.

use std::rc::Rc;
use std::time::Instant;

use asrs_hub::hub::HubModel;
use asrs_hub::hub::HubView;
use asrs_hub::satellite::{SAVE_PERIOD_MS, SatelliteModel};
use asrs_hub::tabs::{SatelliteUpdate, TabsError};
use asrs_js::time::{Clock, SystemClock, V8MathRandom};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToSatellite, NamedMessage, SatelliteToMain};
use asrs_ui::gpui::{AnyWindowHandle, App, Context, EventEmitter, Task, Window};

use crate::hub::{FrameClock, frame_time, interval};
use crate::repaint::Repaint;

#[derive(Clone, Debug, PartialEq)]
pub enum SatelliteEvent {
    /// [`SatelliteModel::title`] changed.
    Title(String),
    Main(NamedMessage<SatelliteToMain>),
}

impl EventEmitter<SatelliteEvent> for Satellite {}

/// Blink runs a hidden page's timers once a second, and satellites keep `backgroundThrottling`.
const HIDDEN_PERIOD_MS: f64 = 1000.0;

pub struct Satellite {
    model: SatelliteModel,
    title: String,
    frame_origin: Instant,
    window: Option<AnyWindowHandle>,
    repaint: Repaint,
    _save: Task<()>,
}

fn save_interval(cx: &Context<'_, Satellite>, period_ms: f64) -> Task<()> {
    interval(cx, period_ms, |satellite: &mut Satellite, cx| {
        satellite.model.save_state();
        satellite.after_task(cx);
    })
}

impl Satellite {
    /// With `scenario_seed`, the page runs as a parity scenario's, as the hub's does.
    pub fn new(
        view: Box<dyn HubView>,
        scenario_seed: Option<u64>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let (random, clock): (_, Rc<dyn Clock>) = match scenario_seed {
            Some(seed) => (V8MathRandom::from_seed(seed), Rc::new(FrameClock::new(cx))),
            None => (V8MathRandom::from_entropy(), Rc::new(SystemClock)),
        };
        let model = SatelliteModel::new(JsRealm::new(Box::new(random)), clock, view);
        Satellite {
            title: model.title().to_string_lossy(),
            model,
            frame_origin: cx.background_executor().now(),
            window: None,
            repaint: Repaint::default(),
            _save: save_interval(cx, SAVE_PERIOD_MS),
        }
    }

    /// Throttles the page while `window` is hidden, once it has been shown.
    pub fn throttle_with(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.window = Some(window.window_handle());
        cx.observe_window_visibility(window, |satellite, visibility, _, cx| {
            let period = if visibility.is_visible() {
                SAVE_PERIOD_MS
            } else {
                HIDDEN_PERIOD_MS
            };
            satellite._save = save_interval(cx, period);
            if visibility.is_visible() {
                satellite.repaint.changed();
            }
        })
        .detach();
    }

    pub fn model(&self) -> &SatelliteModel {
        &self.model
    }

    /// Milliseconds on the frame clock, which the page's scroll sensors animate on.
    pub fn frame_time(&self, cx: &App) -> f64 {
        frame_time(self.frame_origin, cx)
    }

    /// Runs an input handler a view dispatches into the page.
    pub fn update<R>(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut HubModel) -> R,
    ) -> R {
        let result = f(self.model.hub_mut());
        self.shown_task(cx);
        result
    }

    /// One message main posts on the port.
    pub fn handle_main_message(
        &mut self,
        message: &NamedMessage<MainToSatellite>,
        cx: &mut Context<'_, Self>,
    ) {
        self.task(cx, |model| model.handle_main_message(message));
    }

    /// `render`, with the hub's command itself in place of its clone, after a frame of the hub's
    /// that showed something new if it was `moving`.
    pub fn render_command(
        &mut self,
        update: &SatelliteUpdate,
        moving: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if let Err(e) = self.model.render(update.command.clone(), &update.title) {
            eprintln!("asrs: satellite: {e}");
        }
        self.after_task(cx);
        let changed = moving || self.model.needs_draw();
        let Some(window) = self.window else {
            cx.notify();
            return;
        };
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let dirty = window.update(cx, |_, window, _| window.is_dirty());
            let _ = this.update(cx, |satellite, cx| {
                if satellite.repaint.frame(changed || dirty.unwrap_or(false)) {
                    cx.notify();
                }
            });
        });
    }

    pub(crate) fn begin_draw(&mut self) {
        self.repaint.begin_draw();
    }

    pub(crate) fn end_draw(&mut self, window: &Window) {
        self.repaint.end_draw();
        if window.has_next_frame_callbacks() {
            self.repaint.changed();
        }
    }

    /// Input whose effect is drawn rather than run as a task.
    pub(crate) fn touched(&mut self) {
        self.repaint.task();
    }

    /// The renderer's own `requestAnimationFrame` callbacks.
    pub fn animation_frame(&mut self, cx: &mut Context<'_, Self>) {
        for e in self.model.animation_frame() {
            eprintln!("asrs: satellite: {}: {}", e.name, e.message);
        }
        self.shown_task(cx);
    }

    pub fn resized(&mut self, cx: &mut Context<'_, Self>) {
        self.task(cx, SatelliteModel::resize);
    }

    pub fn key_down(&mut self, code: &str, cx: &mut Context<'_, Self>) -> bool {
        let handled = self.model.key_down(code);
        self.shown_task(cx);
        handled
    }

    pub fn before_unload(&mut self, cx: &mut Context<'_, Self>) {
        self.model.before_unload();
        self.shown_task(cx);
    }

    /// `window.sendMainMessage` from a view.
    pub fn send_main_message(
        &mut self,
        message: NamedMessage<HubToMain>,
        cx: &mut Context<'_, Self>,
    ) {
        self.model.forward(message);
        self.shown_task(cx);
    }

    fn task(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut SatelliteModel) -> Result<(), TabsError>,
    ) {
        if let Err(e) = f(&mut self.model) {
            eprintln!("asrs: satellite: {e}");
        }
        self.shown_task(cx);
    }

    fn shown_task(&mut self, cx: &mut Context<'_, Self>) {
        self.after_task(cx);
        self.repaint.task();
        cx.notify();
    }

    fn after_task(&mut self, cx: &mut Context<'_, Self>) {
        for message in self.model.take_messages() {
            cx.emit(SatelliteEvent::Main(message));
        }
        let title = self.model.title().to_string_lossy();
        if title != self.title {
            self.title = title.clone();
            cx.emit(SatelliteEvent::Title(title));
        }
    }
}
