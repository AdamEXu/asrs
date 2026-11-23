//! The X11 side of a GPUI window, over GPUI's own connection: pointer grabs only bring events to
//! the client that takes them.

use gpui_kit::{Bounds, Pixels, Point, Window, point, px, size};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use x11rb::CURRENT_TIME;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::properties::{AspectRatio, WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ClientMessageEvent, ConfigureWindowAux,
    ConnectionExt as _, EventMask, GrabMode, GrabStatus, Gravity, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT, MapState, PropMode,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub struct X11Window {
    connection: XCBConnection,
    window: u32,
    root: u32,
    scale: f32,
}

impl X11Window {
    pub fn of(window: &Window) -> Option<X11Window> {
        let RawWindowHandle::Xcb(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        let RawDisplayHandle::Xcb(display) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: GPUI's connection lives for as long as its client, which outlives every window,
        // and `X11Window` is only used while a window is updated or kept beside it; `false` leaves
        // closing the connection to GPUI.
        let connection = unsafe {
            XCBConnection::from_raw_xcb_connection(display.connection?.as_ptr(), false).ok()?
        };
        let root = connection
            .setup()
            .roots
            .get(usize::try_from(display.screen).ok()?)?
            .root;
        Some(X11Window {
            connection,
            window: handle.window.get(),
            root,
            scale: window.scale_factor(),
        })
    }

    pub fn id(&self) -> u32 {
        self.window
    }

    /// Another window of the same display.
    pub fn with_id(mut self, window: u32) -> X11Window {
        self.window = window;
        self
    }

    fn atom(&self, name: &str) -> Result<u32> {
        Ok(self
            .connection
            .intern_atom(false, name.as_bytes())?
            .reply()?
            .atom)
    }

    fn logical(&self, v: i32) -> Pixels {
        px(v as f32 / self.scale)
    }

    fn device(&self, v: f64) -> i32 {
        (v * f64::from(self.scale)).round() as i32
    }

    /// The content's top-left corner on screen. GPUI's own is relative to the window manager's
    /// frame after a resize.
    pub fn origin(&self) -> Result<Point<Pixels>> {
        let at = self
            .connection
            .translate_coordinates(self.window, self.root, 0, 0)?
            .reply()?;
        Ok(point(
            self.logical(at.dst_x.into()),
            self.logical(at.dst_y.into()),
        ))
    }

    /// The monitor at a position on screen, or the nearest to it. GPUI's X11 display is the
    /// screen, which spans every monitor.
    pub fn monitor_at(&self, at: Point<Pixels>) -> Result<Option<Bounds<Pixels>>> {
        let reply = self
            .connection
            .randr_get_monitors(self.root, true)?
            .reply()?;
        let monitors = reply.monitors.into_iter().map(|m| {
            Bounds::new(
                point(self.logical(m.x.into()), self.logical(m.y.into())),
                size(self.logical(m.width.into()), self.logical(m.height.into())),
            )
        });
        let outside =
            |v: Pixels, low: Pixels, high: Pixels| f32::from((low - v).max(v - high).max(px(0.0)));
        let distance = |m: &Bounds<Pixels>| {
            let dx = outside(at.x, m.left(), m.right());
            let dy = outside(at.y, m.top(), m.bottom());
            dx * dx + dy * dy
        };
        Ok(monitors.min_by(|a, b| {
            let inside = |m: &Bounds<Pixels>| !m.contains(&at);
            (distance(a), inside(a))
                .partial_cmp(&(distance(b), inside(b)))
                .unwrap_or(std::cmp::Ordering::Equal)
        }))
    }

    /// The content's size in logical px.
    pub fn size(&self) -> Result<(f64, f64)> {
        let geometry = self.connection.get_geometry(self.window)?.reply()?;
        let css = |v: u16| f64::from(v) / f64::from(self.scale);
        Ok((css(geometry.width), css(geometry.height)))
    }

    pub fn resize(&self, width: f64, height: f64) -> Result<()> {
        let aux = ConfigureWindowAux::new()
            .width(self.device(width).max(1) as u32)
            .height(self.device(height).max(1) as u32);
        self.connection.configure_window(self.window, &aux)?;
        Ok(self.connection.flush()?)
    }

    pub fn move_to(&self, origin: Point<Pixels>) -> Result<()> {
        let aux = ConfigureWindowAux::new()
            .x(self.device(f64::from(f32::from(origin.x))))
            .y(self.device(f64::from(f32::from(origin.y))));
        self.connection.configure_window(self.window, &aux)?;
        Ok(self.connection.flush()?)
    }

    /// Puts the content at a screen position with the static gravity Chromium sets, under which
    /// the window manager's frame goes around it.
    pub fn place(&self, x: f64, y: f64) -> Result<()> {
        let mut hints = self.size_hints()?;
        let (device_x, device_y) = (self.device(x), self.device(y));
        hints.position = Some((
            WmSizeHintsSpecification::ProgramSpecified,
            device_x,
            device_y,
        ));
        hints.win_gravity = Some(Gravity::STATIC);
        self.set_size_hints(&hints)?;
        self.move_to(point(px(x as f32), px(y as f32)))
    }

    /// Mapped, which a minimized window is not.
    pub fn is_mapped(&self) -> Result<bool> {
        let attributes = self
            .connection
            .get_window_attributes(self.window)?
            .reply()?;
        Ok(attributes.map_state != MapState::UNMAPPED)
    }

    fn size_hints(&self) -> Result<WmSizeHints> {
        Ok(
            WmSizeHints::get_normal_hints(&self.connection, self.window)?
                .reply()?
                .unwrap_or_default(),
        )
    }

    fn set_size_hints(&self, hints: &WmSizeHints) -> Result<()> {
        hints.set_normal_hints(&self.connection, self.window)?;
        Ok(self.connection.flush()?)
    }

    /// `X11Window::SetAspectRatio` of the ratio Electron scales by 100, since the hint is in
    /// whole numbers.
    pub fn set_aspect_ratio(&self, ratio: f64) -> Result<()> {
        let mut hints = self.size_hints()?;
        hints.aspect = (ratio > 0.0).then(|| {
            let aspect = AspectRatio::new((ratio as f32 * 100.0) as i32, 100);
            (aspect, aspect)
        });
        self.set_size_hints(&hints)
    }

    pub fn aspect_ratio(&self) -> Result<Option<[i32; 4]>> {
        Ok(self.size_hints()?.aspect.map(|(min, max)| {
            [
                min.numerator,
                min.denominator,
                max.numerator,
                max.denominator,
            ]
        }))
    }

    pub fn set_size_limits(&self, min: Option<(f64, f64)>, max: Option<(f64, f64)>) -> Result<()> {
        let mut hints = self.size_hints()?;
        let device = |(w, h): (f64, f64)| (self.device(w), self.device(h));
        if let Some(min) = min {
            hints.min_size = Some(device(min));
        }
        if let Some(max) = max {
            hints.max_size = Some(device(max));
        }
        self.set_size_hints(&hints)
    }

    /// The minimum and maximum sizes of the window's hints.
    pub fn size_limits(&self) -> Result<[Option<(i32, i32)>; 2]> {
        let hints = self.size_hints()?;
        Ok([hints.min_size, hints.max_size])
    }

    pub fn set_transient_for(&self, parent: &X11Window) -> Result<()> {
        self.connection.change_property32(
            PropMode::REPLACE,
            self.window,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[parent.window],
        )?;
        Ok(self.connection.flush()?)
    }

    pub fn keep_above(&self) -> Result<()> {
        const ADD: u32 = 1;
        let state = self.atom("_NET_WM_STATE")?;
        let above = self.atom("_NET_WM_STATE_ABOVE")?;
        let message = ClientMessageEvent::new(32, self.window, state, [ADD, above, 0, 1, 0]);
        self.connection.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            message,
        )?;
        Ok(self.connection.flush()?)
    }

    pub fn states(&self) -> Result<Vec<u32>> {
        let property = self
            .connection
            .get_property(
                false,
                self.window,
                self.atom("_NET_WM_STATE")?,
                AtomEnum::ATOM,
                0,
                64,
            )?
            .reply()?;
        Ok(property
            .value32()
            .map(Iterator::collect)
            .unwrap_or_default())
    }

    pub fn is_above(&self) -> Result<bool> {
        Ok(self.states()?.contains(&self.atom("_NET_WM_STATE_ABOVE")?))
    }

    /// Every pointer event goes to this window until it closes.
    pub fn grab_pointer(&self) -> Result<bool> {
        if self
            .connection
            .extension_information(xinput::X11_EXTENSION_NAME)?
            .is_none()
        {
            return Ok(true);
        }
        let pointer = self
            .connection
            .xinput_xi_get_client_pointer(self.window)?
            .reply()?
            .deviceid;
        let mask = xinput::XIEventMask::MOTION
            | xinput::XIEventMask::BUTTON_PRESS
            | xinput::XIEventMask::BUTTON_RELEASE
            | xinput::XIEventMask::ENTER
            | xinput::XIEventMask::LEAVE;
        let reply = self
            .connection
            .xinput_xi_grab_device(
                self.window,
                CURRENT_TIME,
                x11rb::NONE,
                pointer,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                xinput::GrabOwner::NO_OWNER,
                &[u32::from(mask)],
            )?
            .reply()?;
        Ok(reply.status == GrabStatus::SUCCESS)
    }
}

/// Moves the pointer and presses or releases a button as a device would, for live checks.
pub fn fake_pointer(position: Option<(f64, f64)>, button: Option<(u8, bool)>) -> Result<()> {
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    if let Some((x, y)) = position {
        let (x, y) = (x.round() as i16, y.round() as i16);
        connection
            .xtest_fake_input(MOTION_NOTIFY_EVENT, 0, CURRENT_TIME, root, x, y, 0)?
            .check()?;
    }
    if let Some((button, down)) = button {
        let event = if down {
            BUTTON_PRESS_EVENT
        } else {
            BUTTON_RELEASE_EVENT
        };
        connection
            .xtest_fake_input(event, button, CURRENT_TIME, root, 0, 0, 0)?
            .check()?;
    }
    Ok(())
}

/// Presses the keys the keysyms are on in turn, then releases them.
pub fn fake_keys(keysyms: &[u32]) -> Result<()> {
    let (connection, screen) = x11rb::connect(None)?;
    let setup = connection.setup();
    let root = setup.roots[screen].root;
    let count = setup.max_keycode - setup.min_keycode + 1;
    let keys = connection
        .get_keyboard_mapping(setup.min_keycode, count)?
        .reply()?;
    let mut codes = Vec::new();
    for keysym in keysyms {
        let index = keys
            .keysyms
            .chunks(usize::from(keys.keysyms_per_keycode))
            .position(|syms| syms.contains(keysym))
            .ok_or("no key for the keysym")?;
        codes.push(setup.min_keycode + index as u8);
    }
    let presses = codes.iter().map(|code| (KEY_PRESS_EVENT, *code));
    let releases = codes.iter().rev().map(|code| (KEY_RELEASE_EVENT, *code));
    for (event, code) in presses.chain(releases) {
        connection
            .xtest_fake_input(event, code, CURRENT_TIME, root, 0, 0, 0)?
            .check()?;
    }
    Ok(())
}

/// Divides the screen into two monitors at `x`, or undoes that.
pub fn split_monitors(x: Option<u16>) -> Result<()> {
    const NAMES: [&[u8]; 2] = [b"ASRS-LEFT", b"ASRS-RIGHT"];
    let (connection, number) = x11rb::connect(None)?;
    let screen = &connection.setup().roots[number];
    let (root, width, height) = (screen.root, screen.width_in_pixels, screen.height_in_pixels);
    let outputs = connection
        .randr_get_screen_resources_current(root)?
        .reply()?
        .outputs;
    let mut names = Vec::new();
    for name in NAMES {
        names.push(connection.intern_atom(false, name)?.reply()?.atom);
    }
    let Some(x) = x else {
        for name in names {
            connection.randr_delete_monitor(root, name)?.check()?;
        }
        return Ok(());
    };
    let boxes = [(0, x, outputs), (x, width - x, Vec::new())];
    for (name, (x, width, outputs)) in names.into_iter().zip(boxes) {
        let monitor = randr::MonitorInfo {
            name,
            primary: false,
            automatic: false,
            x: x as i16,
            y: 0,
            width,
            height,
            width_in_millimeters: 300,
            height_in_millimeters: 300,
            outputs,
        };
        connection.randr_set_monitor(root, monitor)?.check()?;
    }
    Ok(())
}
