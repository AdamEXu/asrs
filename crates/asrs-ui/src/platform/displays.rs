//! Chromium's X11 displays (`ui/base/x/x11_display_util.cc`): the CRTCs of the connected outputs,
//! with the window manager's work area in the one it is for.

/// `[x, y, width, height]` in logical px.
pub type Area = [f64; 4];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub bounds: Area,
    pub work_area: Area,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Displays {
    pub displays: Vec<Display>,
    primary: usize,
}

fn contains(outer: Area, inner: Area) -> bool {
    (0..2).all(|i| inner[i] >= outer[i] && inner[i] + inner[i + 2] <= outer[i] + outer[i + 2])
}

/// `gfx::ScaleToEnclosingRect` of device px.
fn enclosing([x, y, width, height]: [i32; 4], scale: f64) -> Area {
    let (left, top) = (f64::from(x) / scale, f64::from(y) / scale);
    let right = f64::from(x.saturating_add(width)) / scale;
    let bottom = f64::from(y.saturating_add(height)) / scale;
    let [left, top] = [left.floor(), top.floor()];
    [left, top, right.ceil() - left, bottom.ceil() - top]
}

/// The whole logical px inside device px.
fn enclosed([x, y, width, height]: [i32; 4], scale: f64) -> Area {
    let (left, top) = (f64::from(x) / scale, f64::from(y) / scale);
    let right = f64::from(x.saturating_add(width)) / scale;
    let bottom = f64::from(y.saturating_add(height)) / scale;
    let [left, top] = [left.ceil(), top.ceil()];
    [left, top, right.floor() - left, bottom.floor() - top]
}

fn intersection(a: Area, b: Area) -> Option<Area> {
    let mut out = [0.0; 4];
    for i in 0..2 {
        out[i] = a[i].max(b[i]);
        out[i + 2] = (a[i] + a[i + 2]).min(b[i] + b[i + 2]) - out[i];
    }
    (out[2] > 0.0 && out[3] > 0.0).then_some(out)
}

impl Displays {
    /// Of CRTCs in device px, the primary's index and the root window's `_NET_WORKAREA`.
    pub fn new(
        crtcs: &[[i32; 4]],
        primary: usize,
        net_work_area: Option<[i32; 4]>,
        scale: f64,
    ) -> Displays {
        let mut displays: Vec<Display> = crtcs
            .iter()
            .map(|crtc| enclosed(*crtc, scale))
            .map(|bounds| Display {
                bounds,
                work_area: bounds,
            })
            .collect();
        if let Some(net) = net_work_area.filter(|net| net[2] > 0 && net[3] > 0) {
            clip_work_area(&mut displays, primary, enclosing(net, scale));
        }
        Displays { displays, primary }
    }

    /// The display most of a window at `bounds` is on, the primary for one on none.
    pub fn of_window(&self, bounds: Area) -> Option<&Display> {
        let mut found = (0.0, self.displays.get(self.primary)?);
        for display in &self.displays {
            let shared = intersection(display.bounds, bounds).map_or(0.0, |[_, _, w, h]| w * h);
            if shared > found.0 {
                found = (shared, display);
            }
        }
        Some(found.1)
    }

    /// Where `MenuController` keeps the menus of `anchor`.
    pub fn menu_area(&self, anchor: Area) -> Option<Area> {
        let [x, y, ..] = anchor;
        let outside = |v: f64, low: f64, length: f64| (low - v).max(v - (low + length)).max(0.0);
        let key = |display: &&Display| {
            let [left, top, width, height] = display.bounds;
            let inside = x >= left && x < left + width && y >= top && y < top + height;
            (!inside, outside(x, left, width) + outside(y, top, height))
        };
        let display = self.displays.iter().min_by(|a, b| {
            let (a, b) = (key(a), key(b));
            a.0.cmp(&b.0).then(a.1.total_cmp(&b.1))
        })?;
        let launcher = !contains(display.work_area, anchor) && contains(display.bounds, anchor);
        Some(if launcher {
            display.bounds
        } else {
            display.work_area
        })
    }
}

/// `ClipWorkArea`: the work area is one display's when that is the only one in it or the first
/// around it, and the primary's, like GTK's, when it spans several.
fn clip_work_area(displays: &mut [Display], primary: usize, net: Area) {
    let covered = displays.iter().filter(|d| contains(net, d.bounds)).count();
    if covered == 1 {
        return;
    }
    let around = displays.iter().position(|d| contains(d.bounds, net));
    let Some(display) = displays.get_mut(around.unwrap_or(primary)) else {
        return;
    };
    if let Some(area) = intersection(net, display.work_area) {
        display.work_area = area;
    }
}
