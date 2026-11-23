//! A satellite window's root: the hub's [`HubFrame`] over a [`Satellite`]'s page, whose one
//! renderer fills the window.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_canvas::text::TextBackend;
use asrs_hub::hub::{HubModel, HubPage, HubView};
use asrs_hub::sidebar::{NoLayout, SidebarGeometry};
use asrs_model::messages::{HubToMain, MainToSatellite, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::source_list::{Rect, SourceListDom};
use asrs_model::tab_controller::TabRenderer;
use asrs_model::tab_type::TabType;
use asrs_ui::gpui::{App, AppContext, Entity, KeyDownEvent};

use super::frame::{FrameGeometry, FrameHost, HubFrame};
use super::hub_host;
use super::source_list::ListsGeometry;
use crate::satellite::Satellite;

struct SatelliteView {
    geometry: FrameGeometry,
    canvas_text: Rc<RefCell<dyn TextBackend>>,
    lists: ListsGeometry,
    sidebar: NoLayout,
    dark: bool,
}

impl HubView for SatelliteView {
    fn inner_height(&self) -> f64 {
        self.geometry.inner_height()
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        0.0
    }

    fn table_box(&self) -> Option<(Rect, f64)> {
        self.geometry.table_box()
    }

    fn dark(&self) -> bool {
        self.dark
    }

    fn sidebar_geometry(&mut self) -> &mut dyn SidebarGeometry {
        &mut self.sidebar
    }

    fn source_list_dom(&mut self) -> &mut dyn SourceListDom {
        &mut self.lists
    }

    fn canvas_text(&self) -> Option<Rc<RefCell<dyn TextBackend>>> {
        Some(self.canvas_text.clone())
    }

    fn tab_renderer(
        &self,
        kind: TabType,
        preferences: Option<&Preferences>,
        is_battery: bool,
    ) -> Option<Box<dyn TabRenderer<HubPage>>> {
        hub_host::view_renderer(kind, preferences, is_battery)
    }
}

pub struct SatelliteHost {
    satellite: Entity<Satellite>,
    geometry: FrameGeometry,
    lists: ListsGeometry,
}

impl SatelliteHost {
    /// The satellite window's root view, sent `boot` before anything draws.
    pub fn open(
        dark: bool,
        boot: &[NamedMessage<MainToSatellite>],
        scenario_seed: Option<u64>,
        cx: &mut App,
    ) -> Entity<HubFrame<SatelliteHost>> {
        let geometry = FrameGeometry::default();
        let lists = ListsGeometry::default();
        let view = Box::new(SatelliteView {
            geometry: geometry.clone(),
            canvas_text: hub_host::canvas_text(),
            lists: lists.clone(),
            sidebar: NoLayout,
            dark,
        });
        let satellite = cx.new(|cx| Satellite::new(view, scenario_seed, cx));
        satellite.update(cx, |satellite, cx| {
            for message in boot {
                satellite.handle_main_message(message, cx);
            }
        });
        cx.new(|cx| {
            cx.observe(&satellite, |_, _, cx| cx.notify()).detach();
            HubFrame::new(
                SatelliteHost {
                    satellite,
                    geometry,
                    lists,
                },
                cx,
            )
        })
    }

    pub fn satellite(&self) -> &Entity<Satellite> {
        &self.satellite
    }
}

impl FrameHost for SatelliteHost {
    fn geometry(&self) -> FrameGeometry {
        self.geometry.clone()
    }

    fn source_list_geometry(&self) -> ListsGeometry {
        self.lists.clone()
    }

    fn frame_time(&self, cx: &App) -> f64 {
        self.satellite.read(cx).frame_time(cx)
    }

    fn hub<'a>(&'a self, cx: &'a App) -> &'a HubModel {
        self.satellite.read(cx).model().hub()
    }

    fn input<R>(&mut self, cx: &mut App, f: impl FnOnce(&mut HubModel) -> R) -> R {
        self.satellite
            .update(cx, |satellite, cx| satellite.update(cx, f))
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>, cx: &mut App) {
        self.satellite
            .update(cx, |satellite, cx| satellite.send_main_message(message, cx));
    }

    fn resized(&mut self, cx: &mut App) {
        self.satellite
            .update(cx, |satellite, cx| satellite.resized(cx));
    }

    fn key_down(&mut self, _event: &KeyDownEvent, code: &str, cx: &mut App) -> bool {
        self.satellite
            .update(cx, |satellite, cx| satellite.key_down(code, cx))
    }
}
