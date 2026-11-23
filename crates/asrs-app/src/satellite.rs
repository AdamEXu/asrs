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
use asrs_ui::gpui::{App, Context, EventEmitter, Task};

use crate::hub::{FrameClock, frame_time, interval};

#[derive(Clone, Debug, PartialEq)]
pub enum SatelliteEvent {
    /// [`SatelliteModel::title`] changed.
    Title(String),
    Main(NamedMessage<SatelliteToMain>),
}

impl EventEmitter<SatelliteEvent> for Satellite {}

pub struct Satellite {
    model: SatelliteModel,
    title: String,
    frame_origin: Instant,
    _save: Task<()>,
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
            _save: interval(cx, SAVE_PERIOD_MS, |satellite: &mut Satellite, cx| {
                satellite.task(cx, |model| {
                    model.save_state();
                    Ok(())
                });
            }),
        }
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
        self.after_task(cx);
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

    /// `render`, with the hub's command itself in place of its clone.
    pub fn render_command(&mut self, update: &SatelliteUpdate, cx: &mut Context<'_, Self>) {
        self.task(cx, |model| {
            model.render(update.command.clone(), &update.title)
        });
    }

    pub fn resized(&mut self, cx: &mut Context<'_, Self>) {
        self.task(cx, SatelliteModel::resize);
    }

    pub fn key_down(&mut self, code: &str, cx: &mut Context<'_, Self>) -> bool {
        let handled = self.model.key_down(code);
        self.after_task(cx);
        handled
    }

    pub fn before_unload(&mut self, cx: &mut Context<'_, Self>) {
        self.model.before_unload();
        self.after_task(cx);
    }

    /// `window.sendMainMessage` from a view.
    pub fn send_main_message(
        &mut self,
        message: NamedMessage<HubToMain>,
        cx: &mut Context<'_, Self>,
    ) {
        self.model.forward(message);
        self.after_task(cx);
    }

    fn task(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut SatelliteModel) -> Result<(), TabsError>,
    ) {
        if let Err(e) = f(&mut self.model) {
            eprintln!("asrs: satellite: {e}");
        }
        self.after_task(cx);
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
        cx.notify();
    }
}
