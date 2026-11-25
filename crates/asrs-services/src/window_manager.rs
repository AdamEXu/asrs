//! `main.ts`'s window bookkeeping: the `BrowserWindow` options each window kind is created with,
//! where it is placed relative to its parent or the focused window, the singleton windows, the
//! satellite lists and the hub list in most-recently-focused order (`hubWindows`).
//!
//! Options are built for one `process.platform`. Only macOS's are complete: the hub's
//! `vibrancy` and `titleBarStyle`, and no `icon` (`WINDOW_ICON` is undefined there). The Windows
//! hub's `backgroundMaterial`, `titleBarStyle` and `titleBarOverlay` are `asrs_app::windows`'.

use asrs_js::value::JsObject;

use crate::constants::{
    HUB_DEFAULT_HEIGHT, HUB_DEFAULT_WIDTH, SATELLITE_DEFAULT_HEIGHT, SATELLITE_DEFAULT_WIDTH,
};
use crate::windows::Rect;

pub type WindowId = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Platform {
    #[default]
    Mac,
    Windows,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowKind {
    Hub,
    Satellite,
    EditRange,
    UnitConversion,
    RenameTab,
    EditFov,
    Export,
    Preferences,
    Download,
    Licenses,
    SourceListHelp,
    BetaWelcome,
}

impl WindowKind {
    pub fn is_modal(self) -> bool {
        matches!(
            self,
            WindowKind::EditRange
                | WindowKind::UnitConversion
                | WindowKind::RenameTab
                | WindowKind::EditFov
                | WindowKind::Export
                | WindowKind::BetaWelcome
        )
    }
}

/// The subset of `BrowserWindowConstructorOptions` upstream passes. `None` and `false` mean the
/// key is absent, except where Electron's default differs (`resizable`, `fullscreenable`).
#[derive(Clone, Debug, PartialEq)]
pub struct WindowOptions {
    pub kind: WindowKind,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: f64,
    pub height: f64,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    pub max_width: Option<f64>,
    pub use_content_size: bool,
    pub resizable: Option<bool>,
    pub always_on_top: bool,
    pub fullscreenable: Option<bool>,
    pub parent: Option<WindowId>,
    pub modal: bool,
    pub vibrancy: Option<&'static str>,
    pub title_bar_style: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    Undefined,
    Bool(bool),
    Number(f64),
    String(&'static str),
    Window(WindowId),
}

impl WindowOptions {
    fn new(kind: WindowKind, width: f64, height: f64) -> Self {
        Self {
            kind,
            x: None,
            y: None,
            width,
            height,
            min_width: None,
            min_height: None,
            max_width: None,
            use_content_size: false,
            resizable: None,
            always_on_top: false,
            fullscreenable: None,
            parent: None,
            modal: false,
            vibrancy: None,
            title_bar_style: None,
        }
    }

    fn modal(kind: WindowKind, parent: WindowId, width: f64, height: f64) -> Self {
        Self {
            use_content_size: true,
            resizable: Some(false),
            parent: Some(parent),
            modal: true,
            ..Self::new(kind, width, height)
        }
    }

    /// The options as `new BrowserWindow` receives them, keyed by Electron's names, with
    /// `webPreferences` left out.
    pub fn electron_entries(&self) -> Vec<(&'static str, OptionValue)> {
        let mut out = vec![
            ("width", OptionValue::Number(self.width)),
            ("height", OptionValue::Number(self.height)),
            ("icon", OptionValue::Undefined),
            ("show", OptionValue::Bool(false)),
        ];
        let numbers = [
            ("x", self.x),
            ("y", self.y),
            ("minWidth", self.min_width),
            ("minHeight", self.min_height),
            ("maxWidth", self.max_width),
        ];
        for (key, value) in numbers {
            if let Some(v) = value {
                out.push((key, OptionValue::Number(v)));
            }
        }
        let flags = [
            ("useContentSize", self.use_content_size),
            ("alwaysOnTop", self.always_on_top),
            ("modal", self.modal),
        ];
        for (key, value) in flags {
            if value {
                out.push((key, OptionValue::Bool(true)));
            }
        }
        if let Some(v) = self.resizable {
            out.push(("resizable", OptionValue::Bool(v)));
        }
        if let Some(v) = self.fullscreenable {
            out.push(("fullscreenable", OptionValue::Bool(v)));
        }
        if let Some(parent) = self.parent {
            out.push(("parent", OptionValue::Window(parent)));
        }
        if let Some(v) = self.vibrancy {
            out.push(("vibrancy", OptionValue::String(v)));
        }
        if let Some(v) = self.title_bar_style {
            out.push(("titleBarStyle", OptionValue::String(v)));
        }
        out
    }
}

/// What a request to show a singleton window did: create it, or focus the live one.
#[derive(Clone, Debug, PartialEq)]
pub enum Opened {
    New(WindowId, WindowOptions),
    Focus(WindowId),
}

#[derive(Clone, Debug)]
struct Entry {
    id: WindowId,
    kind: WindowKind,
    bounds: Option<Rect>,
}

#[derive(Debug, Default)]
pub struct WindowManager {
    platform: Platform,
    next_id: WindowId,
    windows: Vec<Entry>,
    hubs: Vec<WindowId>,
    satellites: JsObject<Vec<WindowId>>,
    preferences: Option<WindowId>,
    download: Option<WindowId>,
    licenses: Option<WindowId>,
    focused: Option<WindowId>,
}

/// `optionRows * 27 + titleRows * 34 + 54` in `openPreferences`.
pub const PREFERENCES_HEIGHT: f64 = 13.0 * 27.0 + 2.0 * 34.0 + 54.0;

fn centered(parent: Rect, width: f64, height: f64) -> (f64, f64) {
    (
        (parent.x + parent.width / 2.0 - width / 2.0).floor(),
        (parent.y + parent.height / 2.0 - height / 2.0).floor(),
    )
}

impl WindowManager {
    /// A manager for the platform this runs on.
    pub fn new() -> Self {
        Self::for_platform(Platform::current())
    }

    pub fn for_platform(platform: Platform) -> Self {
        Self {
            platform,
            ..Self::default()
        }
    }

    fn add(&mut self, kind: WindowKind) -> WindowId {
        self.next_id += 1;
        self.windows.push(Entry {
            id: self.next_id,
            kind,
            bounds: None,
        });
        self.next_id
    }

    fn live(&self, id: Option<WindowId>) -> Option<WindowId> {
        id.filter(|id| self.kind(*id).is_some())
    }

    pub fn kind(&self, id: WindowId) -> Option<WindowKind> {
        self.windows.iter().find(|e| e.id == id).map(|e| e.kind)
    }

    /// `window.getBounds()`, as last reported through [`Self::set_bounds`].
    pub fn bounds(&self, id: WindowId) -> Option<Rect> {
        self.windows
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.bounds)
    }

    pub fn set_bounds(&mut self, id: WindowId, bounds: Rect) {
        if let Some(entry) = self.windows.iter_mut().find(|e| e.id == id) {
            entry.bounds = Some(bounds);
        }
    }

    /// `BrowserWindow.getFocusedWindow()`.
    pub fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    /// A window became the one [`Self::focused`] answers with, and had its `focus` event.
    pub fn focus(&mut self, id: WindowId) {
        self.became_key(id);
        self.raised(id);
    }

    /// `window.isFocused()` turned true, which on macOS is apart from its `focus` event: it
    /// follows the key window and the event the main one.
    pub fn became_key(&mut self, id: WindowId) {
        if self.kind(id).is_some() {
            self.focused = Some(id);
        }
    }

    /// A window's `focus` event. A focused hub moves to the front of [`Self::hubs`].
    pub fn raised(&mut self, id: WindowId) {
        if let Some(i) = self.hubs.iter().position(|h| *h == id) {
            self.hubs.remove(i);
            self.hubs.insert(0, id);
        }
    }

    /// `window.isFocused()` turned false.
    pub fn blur(&mut self, id: WindowId) {
        if self.focused == Some(id) {
            self.focused = None;
        }
    }

    /// A window's `closed` event.
    pub fn closed(&mut self, id: WindowId) {
        self.windows.retain(|e| e.id != id);
        self.hubs.retain(|h| *h != id);
        for (_, list) in self.satellites.iter_mut() {
            list.retain(|s| *s != id);
        }
        self.blur(id);
    }

    /// `hubWindows`: live hubs, most recently focused first, then never-focused hubs in
    /// creation order.
    pub fn hubs(&self) -> &[WindowId] {
        &self.hubs
    }

    /// `Object.entries(satelliteWindows)`: uuids keep their entry, possibly empty, once created.
    pub fn satellites(&self) -> Vec<(String, &[WindowId])> {
        self.satellites
            .iter()
            .map(|(uuid, list)| (uuid.to_string_lossy(), list.as_slice()))
            .collect()
    }

    /// The uuids with at least one satellite, as `sendActiveSatellites` lists them.
    pub fn active_satellites(&self) -> Vec<String> {
        self.satellites()
            .into_iter()
            .filter(|(_, list)| !list.is_empty())
            .map(|(uuid, _)| uuid)
            .collect()
    }

    pub fn download(&self) -> Option<WindowId> {
        self.live(self.download)
    }

    /// The windows "Bring All to Front" moves, in order, before the focused one.
    pub fn bring_to_front_order(&self) -> Vec<WindowId> {
        let mut out = self.hubs.clone();
        out.extend(self.download());
        out.extend(self.live(self.preferences));
        out.extend(self.live(self.licenses));
        for (_, list) in self.satellites() {
            out.extend_from_slice(list);
        }
        out
    }

    /// The windows `sendAllPreferences` sends to, in order: hubs, satellites, the download
    /// window.
    pub fn preference_targets(&self) -> Vec<WindowId> {
        let mut out = self.hubs.clone();
        for (_, list) in self.satellites() {
            out.extend_from_slice(list);
        }
        out.extend(self.download());
        out
    }

    /// `createHubWindow(state)`: saved bounds if given, else the focused window's bounds moved
    /// by 30 px, else the default size with Electron's placement.
    pub fn create_hub(&mut self, state: Option<Rect>) -> (WindowId, WindowOptions) {
        let mac = self.platform == Platform::Mac;
        let mut options = WindowOptions {
            min_width: Some(800.0),
            min_height: Some(400.0),
            vibrancy: mac.then_some("sidebar"),
            title_bar_style: mac.then_some("hiddenInset"),
            ..WindowOptions::new(WindowKind::Hub, HUB_DEFAULT_WIDTH, HUB_DEFAULT_HEIGHT)
        };
        let focused = self.focused.and_then(|id| self.bounds(id));
        if let Some(state) = state {
            (options.x, options.y) = (Some(state.x), Some(state.y));
            (options.width, options.height) = (state.width, state.height);
        } else if let Some(bounds) = focused {
            (options.x, options.y) = (Some(bounds.x + 30.0), Some(bounds.y + 30.0));
            (options.width, options.height) = (bounds.width, bounds.height);
        }
        let id = self.add(WindowKind::Hub);
        self.hubs.push(id);
        (id, options)
    }

    /// `createSatellite`: centred on the parent hub, or at its saved bounds.
    pub fn create_satellite(
        &mut self,
        uuid: &str,
        placement: SatellitePlacement,
    ) -> (WindowId, WindowOptions) {
        let mut options = WindowOptions {
            min_width: Some(200.0),
            min_height: Some(100.0),
            resizable: Some(true),
            ..WindowOptions::new(
                WindowKind::Satellite,
                SATELLITE_DEFAULT_WIDTH,
                SATELLITE_DEFAULT_HEIGHT,
            )
        };
        match placement {
            SatellitePlacement::Parent(parent) => {
                if let Some(bounds) = self.bounds(parent) {
                    let (x, y) = centered(bounds, options.width, options.height);
                    (options.x, options.y) = (Some(x), Some(y));
                }
            }
            SatellitePlacement::Saved(state) => {
                (options.x, options.y) = (Some(state.x), Some(state.y));
                (options.width, options.height) = (state.width, state.height);
            }
            SatellitePlacement::Unplaced => {}
        }
        let id = self.add(WindowKind::Satellite);
        match self.satellites.get_mut(uuid) {
            Some(list) => list.push(id),
            None => {
                self.satellites.insert(uuid, vec![id]);
            }
        }
        (id, options)
    }

    /// The modal prompts: `createEditRangeWindow`, `createUnitConversionWindow`,
    /// `createRenameTabWindow`, `createEditFovWindow`, `createExportWindow` and
    /// `openBetaWelcome`.
    pub fn create_modal(
        &mut self,
        kind: WindowKind,
        parent: WindowId,
    ) -> (WindowId, WindowOptions) {
        let (width, height) = match kind {
            WindowKind::EditRange => (300.0, 108.0),
            WindowKind::UnitConversion => (300.0, 162.0),
            WindowKind::RenameTab | WindowKind::EditFov => (300.0, 81.0),
            WindowKind::Export => (300.0, 189.0),
            WindowKind::BetaWelcome => (
                450.0,
                if self.platform == Platform::Windows {
                    530.0
                } else {
                    490.0
                },
            ),
            _ => panic!("{kind:?} is not a modal prompt"),
        };
        let options = WindowOptions::modal(kind, parent, width, height);
        (self.add(kind), options)
    }

    fn singleton(
        &mut self,
        slot: fn(&mut Self) -> &mut Option<WindowId>,
        options: impl FnOnce(&Self) -> WindowOptions,
    ) -> Opened {
        let current = *slot(self);
        if let Some(id) = self.live(current) {
            self.focus(id);
            return Opened::Focus(id);
        }
        let options = options(self);
        let id = self.add(options.kind);
        *slot(self) = Some(id);
        Opened::New(id, options)
    }

    fn centered_on(&self, parent: WindowId, mut options: WindowOptions) -> WindowOptions {
        if let Some(bounds) = self.bounds(parent) {
            let (x, y) = centered(bounds, options.width, options.height);
            (options.x, options.y) = (Some(x), Some(y));
        }
        options
    }

    /// `openPreferences`.
    pub fn open_preferences(&mut self, parent: WindowId) -> Opened {
        self.singleton(
            |m| &mut m.preferences,
            |m| {
                m.centered_on(
                    parent,
                    WindowOptions {
                        use_content_size: true,
                        resizable: Some(false),
                        always_on_top: true,
                        fullscreenable: Some(false),
                        ..WindowOptions::new(WindowKind::Preferences, 400.0, PREFERENCES_HEIGHT)
                    },
                )
            },
        )
    }

    /// `openDownload`.
    pub fn open_download(&mut self, parent: WindowId) -> Opened {
        self.singleton(
            |m| &mut m.download,
            |m| {
                m.centered_on(
                    parent,
                    WindowOptions {
                        min_width: Some(500.0),
                        min_height: Some(500.0),
                        resizable: Some(true),
                        always_on_top: true,
                        fullscreenable: Some(false),
                        ..WindowOptions::new(WindowKind::Download, 500.0, 500.0)
                    },
                )
            },
        )
    }

    /// `openLicenses`.
    pub fn open_licenses(&mut self, parent: WindowId) -> Opened {
        self.singleton(
            |m| &mut m.licenses,
            |m| {
                m.centered_on(
                    parent,
                    WindowOptions {
                        min_width: Some(600.0),
                        min_height: Some(625.0),
                        resizable: Some(true),
                        fullscreenable: Some(false),
                        ..WindowOptions::new(WindowKind::Licenses, 600.0, 625.0)
                    },
                )
            },
        )
    }

    /// `openSourceListHelp`: a new window every time, 30 px in from the parent's left edge.
    pub fn open_source_list_help(&mut self, parent: WindowId) -> (WindowId, WindowOptions) {
        let (width, height) = (350.0, 500.0);
        let mut options = WindowOptions {
            min_width: Some(width - 75.0),
            max_width: Some(width + 250.0),
            min_height: Some(200.0),
            resizable: Some(true),
            fullscreenable: Some(false),
            always_on_top: true,
            ..WindowOptions::new(WindowKind::SourceListHelp, width, height)
        };
        if let Some(bounds) = self.bounds(parent) {
            options.x = Some((bounds.x + 30.0).floor());
            options.y = Some((bounds.y + bounds.height / 2.0 - height / 2.0).floor());
        }
        (self.add(WindowKind::SourceListHelp), options)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SatellitePlacement {
    Parent(WindowId),
    Saved(Rect),
    /// A saved state without numeric bounds.
    Unplaced,
}
