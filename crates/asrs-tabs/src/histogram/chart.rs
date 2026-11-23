// Ported from Chart.js 4.4.0 (MIT, Copyright (c) 2014-2022 Chart.js Contributors). See THIRD_PARTY_NOTICES.md.

//! `Chart`, `BarController`, `BarElement`, `layouts`, the legend and the event handling of the
//! stacked bar chart `StatisticsRenderer` configures.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::rc::Rc;

use asrs_canvas::Property;
use asrs_canvas::text::TextBackend;
use asrs_js::math::{self, max2, min2};
use asrs_js::number::{self, IntlOptions};
use asrs_js::sort::array_sort;
use asrs_js::string::{JsError, JsString};

use super::animation::{self, AnimationRef, Animator, Config, Easing, Target, Value};
use super::color;
use super::scale::{Axis, ChartView, Margins, Scale};
use super::surface::Surface;
use super::ticks::limit;
use super::tooltip::{self, Bounds, Item, Tooltip};
use crate::statistics::clean_float;

pub const FONT: &str = "normal 12px 'Helvetica Neue', 'Helvetica', 'Arial', sans-serif";
pub const BOLD_FONT: &str = "normal bold 12px 'Helvetica Neue', 'Helvetica', 'Arial', sans-serif";
pub const FONT_SIZE: f64 = 12.0;
pub const LINE_HEIGHT: f64 = FONT_SIZE * 1.2;
pub const DEFAULT_BORDER: &str = "rgba(0,0,0,0.1)";

const LEGEND_BOX_WIDTH: f64 = 40.0;
const LEGEND_PADDING: f64 = 10.0;
const INFLATE: f64 = 0.33;

const HOVER: Config = Config {
    duration: 400.0,
    easing: Easing::EaseOutQuart,
};

/// What a chart call needs from outside: text metrics and `Date.now()`.
pub struct Env<'a> {
    pub text: &'a mut dyn TextBackend,
    pub now: f64,
}

pub struct ScaleColors {
    pub ticks: String,
    pub border: String,
    pub grid: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Area {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub width: f64,
    pub height: f64,
}

pub struct Dataset {
    pub label: JsString,
    pub data: Vec<f64>,
    pub background: String,
}

/// The bar options a hover changes, as the object elements share or own.
pub struct BarOptions {
    pub background: String,
    pub border: String,
    shared: bool,
    background_animation: Option<AnimationRef>,
    border_animation: Option<AnimationRef>,
}

type OptionsRef = Rc<RefCell<BarOptions>>;

fn bar_options(background: String, border: String, shared: bool) -> OptionsRef {
    Rc::new(RefCell::new(BarOptions {
        background,
        border,
        shared,
        background_animation: None,
        border_animation: None,
    }))
}

pub struct Bar {
    pub x: f64,
    pub y: f64,
    pub base: f64,
    pub width: f64,
    pub height: f64,
    options: OptionsRef,
}

pub type BarRef = Rc<RefCell<Bar>>;

impl Bar {
    fn bounds(&self) -> [f64; 4] {
        let half = self.width / 2.0;
        [
            self.x - half,
            min2(self.y, self.base),
            self.x + half,
            max2(self.y, self.base),
        ]
    }

    fn in_range(&self, x: f64, y: f64) -> bool {
        let [left, top, right, bottom] = self.bounds();
        is_between(x, left, right) && is_between(y, top, bottom)
    }

    fn draw(&self, ctx: &mut Surface) {
        let [left, top, right, bottom] = self.bounds();
        let (width, height) = (right - left, bottom - top);
        let outer = [left, top, width, height];
        let reverse = self.base < self.y;
        let vertical = limit(0.0, 0.0, height / 2.0);
        let (border_top, border_bottom) = if reverse {
            (0.0, vertical)
        } else {
            (vertical, 0.0)
        };
        let side = limit(0.0, 0.0, width / 2.0);
        let inner = [
            left + side,
            top + border_top,
            width - side - side,
            height - border_top - border_bottom,
        ];
        let options = self.options.borrow();
        ctx.save();
        if outer[2] != inner[2] || outer[3] != inner[3] {
            ctx.begin_path();
            rect_path(ctx, inflate(outer, INFLATE, Some(inner)));
            ctx.clip();
            rect_path(ctx, inflate(inner, -INFLATE, Some(outer)));
            ctx.fill_style(&options.border);
            ctx.fill_even_odd();
        }
        ctx.begin_path();
        rect_path(ctx, inflate(inner, INFLATE, None));
        ctx.fill_style(&options.background);
        ctx.fill();
        ctx.restore();
    }
}

fn rect_path(ctx: &mut Surface, [x, y, w, h]: [f64; 4]) {
    ctx.rect(x, y, w, h);
}

/// `inflateRect`; every comparison with a missing reference is with `undefined` or NaN, so true.
fn inflate([rx, ry, rw, rh]: [f64; 4], amount: f64, reference: Option<[f64; 4]>) -> [f64; 4] {
    let differs = |a: f64, b: Option<f64>| b.is_none_or(|b| a != b);
    let x = if differs(rx, reference.map(|r| r[0])) {
        -amount
    } else {
        0.0
    };
    let y = if differs(ry, reference.map(|r| r[1])) {
        -amount
    } else {
        0.0
    };
    let w = if differs(rx + rw, reference.map(|r| r[0] + r[2])) {
        amount
    } else {
        0.0
    } - x;
    let h = if differs(ry + rh, reference.map(|r| r[1] + r[3])) {
        amount
    } else {
        0.0
    } - y;
    [rx + x, ry + y, rw + w, rh + h]
}

fn is_between(value: f64, start: f64, end: f64) -> bool {
    let epsilon = 1e-6;
    value >= min2(start, end) - epsilon && value <= max2(start, end) + epsilon
}

type StackRef = Rc<RefCell<BTreeMap<usize, f64>>>;

struct Parsed {
    x: Option<f64>,
    y: f64,
    stack: StackRef,
}

/// A dataset's meta and its controller.
pub struct Meta {
    dataset: Option<Rc<Dataset>>,
    index: Cell<usize>,
    has_controller: Cell<bool>,
    sorted: Cell<bool>,
    parsed: RefCell<Vec<Parsed>>,
    data: RefCell<Vec<BarRef>>,
    shared: RefCell<Option<OptionsRef>>,
}

fn same_sign(a: f64, b: f64) -> bool {
    a == 0.0 || math::sign(a) == math::sign(b)
}

impl Meta {
    fn new(dataset: Option<Rc<Dataset>>, index: usize) -> Meta {
        Meta {
            dataset,
            index: Cell::new(index),
            has_controller: Cell::new(false),
            sorted: Cell::new(false),
            parsed: RefCell::new(Vec::new()),
            data: RefCell::new(Vec::new()),
            shared: RefCell::new(None),
        }
    }

    fn dataset(&self) -> &Dataset {
        self.dataset
            .as_ref()
            .expect("a meta with a controller has a dataset")
    }

    /// `getMinMax` of the index scale.
    pub fn index_min_max(&self) -> (f64, f64) {
        let parsed = self.parsed.borrow();
        let finite = |p: &Parsed| p.x.filter(|x| x.is_finite());
        let mut range = (f64::INFINITY, f64::NEG_INFINITY);
        let mut extend = |x: f64| range = (min2(range.0, x), max2(range.1, x));
        if self.sorted.get() {
            if let Some(x) = parsed.iter().find_map(finite) {
                extend(x);
            }
            if let Some(x) = parsed.iter().rev().find_map(finite) {
                extend(x);
            }
        } else {
            parsed.iter().filter_map(finite).for_each(extend);
        }
        range
    }

    /// `getMinMax` of the stacked value scale.
    pub fn value_min_max(&self, below: &[Rc<Meta>]) -> (f64, f64) {
        let mut range = (f64::INFINITY, f64::NEG_INFINITY);
        for item in self.parsed.borrow().iter().filter(|p| p.y.is_finite()) {
            let value = apply_stack(item, below);
            range = (min2(range.0, value), max2(range.1, value));
        }
        range
    }

    /// `_destroy`: `clearStacks` stops at the first item whose value is already gone.
    fn destroy(&self) {
        if !self.has_controller.get() {
            return;
        }
        for item in self.parsed.borrow().iter() {
            if item.stack.borrow_mut().remove(&self.index.get()).is_none() {
                return;
            }
        }
    }

    fn default_options(&self) -> OptionsRef {
        self.shared
            .borrow_mut()
            .get_or_insert_with(|| {
                let background = self.dataset().background.clone();
                bar_options(background, DEFAULT_BORDER.to_owned(), true)
            })
            .clone()
    }

    fn hover_options(&self) -> OptionsRef {
        let hover = |css: &str| color::hover(css).unwrap_or_default();
        bar_options(
            hover(&self.dataset().background),
            hover(DEFAULT_BORDER),
            false,
        )
    }

    /// `_setStyle`: animates the element's own options to the hover or default ones.
    fn set_style(&self, element: &BarRef, active: bool, now: f64) -> Vec<AnimationRef> {
        let target = if active {
            self.hover_options()
        } else {
            self.default_options()
        };
        let mut bar = element.borrow_mut();
        if bar.options.borrow().shared {
            let own = {
                let shared = bar.options.borrow();
                bar_options(shared.background.clone(), shared.border.clone(), false)
            };
            bar.options = own;
        }
        let own = bar.options.clone();
        let mut created = Vec::new();
        let (background, border) = {
            let t = target.borrow();
            (t.background.clone(), t.border.clone())
        };
        let mut running = own.borrow().border_animation.clone();
        let on = own.clone();
        animation::animate(
            &mut running,
            HOVER,
            || Target::Border(on),
            Value::Color(border),
            now,
            &mut created,
        );
        own.borrow_mut().border_animation = running;
        let mut running = own.borrow().background_animation.clone();
        let on = own.clone();
        animation::animate(
            &mut running,
            HOVER,
            || Target::Background(on),
            Value::Color(background),
            now,
            &mut created,
        );
        own.borrow_mut().background_animation = running;
        created
    }
}

/// `applyStack` over the datasets below this one.
fn apply_stack(item: &Parsed, below: &[Rc<Meta>]) -> f64 {
    let stack = item.stack.borrow();
    let mut value = item.y;
    for meta in below {
        if let Some(&other) = stack.get(&meta.index.get())
            && other.is_finite()
            && same_sign(value, other)
        {
            value += other;
        }
    }
    value
}

#[derive(Clone)]
pub struct ActiveElement {
    pub element: BarRef,
    pub dataset_index: usize,
    pub index: usize,
}

fn same_element(a: &ActiveElement, b: &ActiveElement) -> bool {
    a.dataset_index == b.dataset_index && a.index == b.index
}

fn elements_equal(a: &[ActiveElement], b: &[ActiveElement]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_element(a, b))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    MouseMove,
    MouseOut,
    Click,
}

#[derive(Clone, Copy, Debug)]
struct Event {
    kind: EventKind,
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Callback {
    Resize,
    Event(EventKind),
    Animate,
}

/// What an animation frame callback did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    Resize,
    Event,
    Draw,
    /// An animation frame that had nothing to draw.
    Idle,
}

#[derive(Default)]
struct Legend {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    height: f64,
    line_widths: Vec<f64>,
}

struct LayoutArea {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    w: f64,
    h: f64,
    x: f64,
    y: f64,
    max_padding: Margins,
    outer: (f64, f64),
}

impl LayoutArea {
    fn margins(&self, horizontal: bool) -> Margins {
        if horizontal {
            Margins {
                left: max2(self.left, self.max_padding.left),
                right: max2(self.right, self.max_padding.right),
                ..Margins::default()
            }
        } else {
            Margins {
                top: max2(self.top, self.max_padding.top),
                bottom: max2(self.bottom, self.max_padding.bottom),
                ..Margins::default()
            }
        }
    }

    /// `updateDims` after the side the box sits on; returns whether the width and the height
    /// changed.
    fn update_dims(&mut self, padding: Option<Margins>) -> (bool, bool) {
        if let Some(p) = padding {
            let m = &mut self.max_padding;
            m.top = max2(m.top, p.top);
            m.left = max2(m.left, p.left);
            m.bottom = max2(m.bottom, p.bottom);
            m.right = max2(m.right, p.right);
        }
        let m = &self.max_padding;
        let horizontal = max2(m.left, self.left) + max2(m.right, self.right);
        let vertical = max2(m.top, self.top) + max2(m.bottom, self.bottom);
        let w = max2(0.0, self.outer.0 - horizontal);
        let h = max2(0.0, self.outer.1 - vertical);
        let changed = (w != self.w, h != self.h);
        (self.w, self.h) = (w, h);
        changed
    }
}

fn resize_side(side: &mut f64, size: &mut f64, box_size: f64) {
    if *size != 0.0 && !size.is_nan() {
        *side -= *size;
    }
    *size = max2(0.0, box_size) / 1.0;
    *side += *size;
}

fn truthy_or(value: f64, or: f64) -> f64 {
    if value == 0.0 || value.is_nan() {
        or
    } else {
        value
    }
}

pub struct Chart {
    pub surface: Surface,
    pub width: f64,
    pub height: f64,
    pub device_pixel_ratio: Option<f64>,
    attached: bool,
    container: (f64, f64),
    window_ratio: f64,
    last_window_ratio: f64,
    pub labels: Vec<f64>,
    pub datasets: Vec<Rc<Dataset>>,
    pub step_size: f64,
    pub legend_color: String,
    pub scale_colors: ScaleColors,
    metasets: Vec<Option<Rc<Meta>>>,
    sorted_metasets: Vec<Rc<Meta>>,
    stacks: Option<HashMap<String, StackRef>>,
    pub x: Scale,
    pub y: Scale,
    legend: Legend,
    tooltip: Tooltip,
    pub area: Option<Area>,
    active: Vec<ActiveElement>,
    last_event: Option<Event>,
    animator: Animator,
    resize_before_draw: Option<Option<(f64, f64)>>,
    callbacks: VecDeque<Callback>,
    pending_resize: Option<(f64, f64)>,
    pending_events: HashMap<&'static str, (f64, f64)>,
}

impl EventKind {
    fn name(self) -> &'static str {
        match self {
            EventKind::MouseMove => "mousemove",
            EventKind::MouseOut => "mouseout",
            EventKind::Click => "click",
        }
    }
}

macro_rules! view {
    ($chart:expr) => {
        ChartView {
            width: $chart.width,
            height: $chart.height,
            device_pixel_ratio: $chart.device_pixel_ratio.unwrap_or(f64::NAN),
            metas: &$chart.sorted_metasets,
            colors: &$chart.scale_colors,
            step_size: $chart.step_size,
        }
    };
}

impl Chart {
    /// `new Chart(canvas, ...)` on a canvas that is not in the document yet.
    pub fn new(device_pixel_ratio: f64) -> Chart {
        let mut chart = Chart {
            surface: Surface::default(),
            width: 0.0,
            height: 0.0,
            device_pixel_ratio: None,
            attached: false,
            container: (0.0, 0.0),
            window_ratio: device_pixel_ratio,
            last_window_ratio: 0.0,
            labels: Vec::new(),
            datasets: Vec::new(),
            step_size: 1.0,
            legend_color: "#666".to_owned(),
            scale_colors: ScaleColors {
                ticks: "#666".to_owned(),
                border: DEFAULT_BORDER.to_owned(),
                grid: DEFAULT_BORDER.to_owned(),
            },
            metasets: Vec::new(),
            sorted_metasets: Vec::new(),
            stacks: None,
            x: Scale::new(Axis::X),
            y: Scale::new(Axis::Y),
            legend: Legend::default(),
            tooltip: Tooltip::new(),
            area: None,
            active: Vec::new(),
            last_event: None,
            animator: Animator::default(),
            resize_before_draw: None,
            callbacks: VecDeque::new(),
            pending_resize: None,
            pending_events: HashMap::new(),
        };
        chart.retina_scale((0.0, 0.0));
        chart
    }

    pub fn metas(&self) -> &[Rc<Meta>] {
        &self.sorted_metasets
    }

    pub fn bars(&self, meta: &Meta) -> Vec<BarRef> {
        meta.data.borrow().clone()
    }

    pub fn legend_box(&self) -> [f64; 4] {
        let l = &self.legend;
        [l.left, l.top, l.right, l.bottom]
    }

    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// The `attach` mutation: the canvas's container is in the document.
    pub fn attach(&mut self, container: (f64, f64), env: &mut Env<'_>) -> Result<(), JsError> {
        self.attached = true;
        self.container = container;
        self.resize(None, env)
    }

    /// The container's `ResizeObserver` entry.
    pub fn container_resized(&mut self, container: (f64, f64)) {
        self.container = container;
        if container == (0.0, 0.0) {
            return;
        }
        self.request_resize(Some(container));
    }

    /// The window's `resize` event.
    pub fn window_resized(&mut self, device_pixel_ratio: f64) {
        self.window_ratio = device_pixel_ratio;
        if device_pixel_ratio == self.last_window_ratio {
            return;
        }
        self.last_window_ratio = device_pixel_ratio;
        if self.device_pixel_ratio != Some(device_pixel_ratio) {
            self.request_resize(None);
        }
    }

    fn request_resize(&mut self, size: Option<(f64, f64)>) {
        self.pending_resize = size;
        self.request(Callback::Resize);
    }

    fn request(&mut self, callback: Callback) {
        if !self.callbacks.contains(&callback) {
            self.callbacks.push_back(callback);
        }
    }

    /// A mouse event on the canvas, at its `offsetX` and `offsetY`.
    pub fn pointer(&mut self, kind: EventKind, x: f64, y: f64) {
        self.pending_events.insert(kind.name(), (x, y));
        self.request(Callback::Event(kind));
    }

    /// How many animation frame callbacks are queued for the next frame.
    pub fn callbacks(&self) -> usize {
        self.callbacks.len()
    }

    /// Runs the first queued animation frame callback.
    pub fn run_callback(&mut self, env: &mut Env<'_>) -> Option<(Work, Result<(), JsError>)> {
        Some(match self.callbacks.pop_front()? {
            Callback::Resize => {
                let size = self.pending_resize;
                (Work::Resize, self.resize(size, env))
            }
            Callback::Event(kind) => {
                let (x, y) = self.pending_events.remove(kind.name()).unwrap_or_default();
                let event = self.relative(kind, x, y);
                (Work::Event, self.event_handler(event, false, env))
            }
            Callback::Animate => {
                if !self.animator.tick(env.now) {
                    self.finish_animation();
                    return Some((Work::Idle, Ok(())));
                }
                let result = self.draw(env);
                if result.is_ok() {
                    self.finish_animation();
                }
                (Work::Draw, result)
            }
        })
    }

    fn finish_animation(&mut self) {
        if self.animator.finish() {
            self.request(Callback::Animate);
        }
    }

    /// `getRelativePosition`.
    fn relative(&self, kind: EventKind, x: f64, y: f64) -> Event {
        let canvas = self.surface.canvas();
        let ratio = self.device_pixel_ratio.unwrap_or(f64::NAN);
        Event {
            kind,
            x: math::round((x - 0.0) / self.width * f64::from(canvas.width) / ratio),
            y: math::round((y - 0.0) / self.height * f64::from(canvas.height) / ratio),
        }
    }

    fn resize(&mut self, size: Option<(f64, f64)>, env: &mut Env<'_>) -> Result<(), JsError> {
        if self.animator.is_running() {
            self.resize_before_draw = Some(size);
            return Ok(());
        }
        self.resize_now(size, env)
    }

    /// `_resize` with `getMaximumSize`.
    fn resize_now(&mut self, size: Option<(f64, f64)>, env: &mut Env<'_>) -> Result<(), JsError> {
        let (width, height) = size.unwrap_or(self.container);
        let round1 = |v: f64| math::round(v * 10.0) / 10.0;
        let width = round1(max2(0.0, width));
        let mut height = round1(max2(0.0, height));
        if width != 0.0 && !width.is_nan() && (height == 0.0 || height.is_nan()) {
            height = round1(width / 2.0);
        }
        if self.retina_scale((width, height)) && self.attached {
            return self.update(env);
        }
        Ok(())
    }

    /// `retinaScale`: whether the canvas changed.
    fn retina_scale(&mut self, (width, height): (f64, f64)) -> bool {
        let ratio = self.window_ratio;
        let device_height = math::floor(height * ratio);
        let device_width = math::floor(width * ratio);
        self.width = math::floor(width);
        self.height = math::floor(height);
        let canvas = self.surface.canvas();
        if self.device_pixel_ratio == Some(ratio)
            && f64::from(canvas.height) == device_height
            && f64::from(canvas.width) == device_width
        {
            return false;
        }
        self.device_pixel_ratio = Some(ratio);
        self.surface
            .set_size(device_width as u32, device_height as u32);
        self.surface
            .set_transform([ratio, 0.0, 0.0, ratio, 0.0, 0.0]);
        true
    }

    /// `getDatasetMeta`.
    fn dataset_meta(&mut self, index: usize) -> Rc<Meta> {
        let dataset = self.datasets.get(index).cloned();
        let matches = |meta: &&Rc<Meta>| match (&meta.dataset, &dataset) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if let Some(meta) = self.metasets.iter().flatten().rfind(matches) {
            return meta.clone();
        }
        let meta = Rc::new(Meta::new(dataset, index));
        self.metasets.push(Some(meta.clone()));
        meta
    }

    fn remove_unreferenced_metasets(&mut self) {
        if self.metasets.len() > self.datasets.len() {
            self.stacks = None;
        }
        for slot in &mut self.metasets {
            let Some(meta) = slot else {
                continue;
            };
            let referenced = meta
                .dataset
                .as_ref()
                .is_some_and(|d| self.datasets.iter().any(|x| Rc::ptr_eq(x, d)));
            if !referenced {
                meta.destroy();
                *slot = None;
            }
        }
    }

    /// `buildOrUpdateControllers`.
    fn build_controllers(&mut self) {
        self.remove_unreferenced_metasets();
        for i in 0..self.datasets.len() {
            let meta = self.dataset_meta(i);
            meta.index.set(i);
            meta.has_controller.set(true);
        }
        let mut metasets: Vec<Rc<Meta>> = self.metasets.drain(..).flatten().collect();
        array_sort(&mut metasets, |a, b| {
            a.index.get() as f64 - b.index.get() as f64
        });
        for meta in metasets.drain(self.datasets.len().min(metasets.len())..) {
            meta.destroy();
        }
        self.metasets = metasets.iter().cloned().map(Some).collect();
        self.sorted_metasets = metasets;
    }

    /// `buildOrUpdateElements` of one controller.
    fn build_elements(&mut self, meta: &Meta) {
        let data = &meta.dataset().data;
        let mut parsed = Vec::with_capacity(data.len());
        let mut sorted = true;
        let mut prev: Option<f64> = None;
        let stacks = self.stacks.get_or_insert_with(HashMap::new);
        for (i, &y) in data.iter().enumerate() {
            let x = self.labels.get(i).copied().filter(|x| x.is_finite());
            match x {
                None => sorted = false,
                Some(x) if i > 0 && x < prev.unwrap_or(0.0) => sorted = false,
                Some(_) => {}
            }
            prev = x;
            let key = x.map_or_else(|| "null".to_owned(), number::to_string);
            let stack = stacks.entry(key).or_default().clone();
            stack.borrow_mut().insert(meta.index.get(), y);
            parsed.push(Parsed { x, y, stack });
        }
        if !data.is_empty() {
            meta.sorted.set(sorted);
        }
        let mut bars = meta.data.borrow_mut();
        for _ in bars.len()..data.len() {
            bars.push(Rc::new(RefCell::new(Bar {
                x: f64::NAN,
                y: f64::NAN,
                base: f64::NAN,
                width: f64::NAN,
                height: f64::NAN,
                options: meta.default_options(),
            })));
        }
        *meta.parsed.borrow_mut() = parsed;
    }

    pub fn update(&mut self, env: &mut Env<'_>) -> Result<(), JsError> {
        self.build_controllers();
        let metas = self.sorted_metasets.clone();
        for meta in &metas {
            self.build_elements(meta);
        }
        self.layout(env);
        self.update_elements();
        if let Some(event) = self.last_event {
            self.event_handler(event, true, env)?;
        } else if !self.active.is_empty() {
            let active = self.active.clone();
            self.update_hover_styles(&active, &active, true, env.now);
        }
        self.render(env)
    }

    /// `layouts.update`.
    fn layout(&mut self, env: &mut Env<'_>) {
        let (width, height) = (self.width, self.height);
        let (available_width, available_height) = (max2(width, 0.0), max2(height, 0.0));
        self.x.before_layout();
        self.y.before_layout();
        let v_box_max_width = available_width / 2.0 / 1.0;
        let h_box_max_height = available_height / 2.0;
        let mut area = LayoutArea {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            w: available_width,
            h: available_height,
            x: 0.0,
            y: 0.0,
            max_padding: Margins::default(),
            outer: (width, height),
        };
        let (mut legend_size, mut x_size, mut y_size) = (0.0, 0.0, 0.0);

        let mut fit_legend = |chart: &mut Chart, area: &mut LayoutArea, env: &mut Env<'_>| {
            let max_width = truthy_or(available_width, area.w);
            let max_height = truthy_or(h_box_max_height, area.h);
            chart.fit_legend(max_width, max_height, env);
            resize_side(&mut area.top, &mut legend_size, chart.legend.height);
            area.update_dims(None)
        };
        let mut fit_y = |chart: &mut Chart, area: &mut LayoutArea, env: &mut Env<'_>| {
            let max_width = truthy_or(v_box_max_width, area.w);
            let margins = area.margins(false);
            let view = view!(chart);
            chart.y.update(
                max_width,
                area.h,
                margins,
                &view,
                &mut chart.surface,
                env.text,
            );
            resize_side(&mut area.left, &mut y_size, chart.y.width);
            area.update_dims(Some(chart.y.padding()));
        };

        fit_legend(self, &mut area, env);
        fit_y(self, &mut area, env);
        let (_, legend_changed) = fit_legend(self, &mut area, env);
        let max_height = truthy_or(h_box_max_height, area.h);
        let margins = area.margins(true);
        let view = view!(self);
        self.x.update(
            area.w,
            max_height,
            margins,
            &view,
            &mut self.surface,
            env.text,
        );
        resize_side(&mut area.bottom, &mut x_size, self.x.height);
        let (_, x_changed) = area.update_dims(Some(self.x.padding()));
        if legend_changed || x_changed {
            fit_y(self, &mut area, env);
        }

        let m = area.max_padding;
        let grow = |side: &mut f64, padding: f64| {
            let change = max2(padding - *side, 0.0);
            *side += change;
            change
        };
        area.y += grow(&mut area.top, m.top);
        area.x += grow(&mut area.left, m.left);
        grow(&mut area.right, m.right);
        grow(&mut area.bottom, m.bottom);

        set_box(
            &mut self.y,
            area.x,
            area.top + 0.0,
            None,
            Some(area.h * 1.0),
        );
        area.x = self.y.right;
        let legend = &mut self.legend;
        legend.left = 0.0;
        legend.top = area.y;
        legend.right = width - 0.0 - 0.0;
        legend.bottom = area.y + legend.height;
        area.y = legend.bottom;
        area.x += area.w;
        area.y += area.h;
        set_box(
            &mut self.x,
            area.left + 0.0,
            area.y,
            Some(area.w * 1.0),
            None,
        );
        self.area = Some(Area {
            left: area.left,
            top: area.top,
            right: area.left + area.w,
            bottom: area.top + area.h,
            width: area.w,
            height: area.h,
        });
        self.x.configure();
        self.y.configure();
    }

    /// `Legend.update` and `fit`.
    fn fit_legend(&mut self, max_width: f64, max_height: f64, env: &mut Env<'_>) {
        self.surface.font(FONT);
        self.surface.text_align("left");
        self.surface.text_baseline("middle");
        let line_height = FONT_SIZE + LEGEND_PADDING;
        let mut total = 0.0;
        let mut line_widths = vec![0.0];
        for (i, meta) in self.sorted_metasets.iter().enumerate() {
            let text = self.surface.measure(env.text, &meta.dataset().label);
            let width = LEGEND_BOX_WIDTH + FONT_SIZE / 2.0 + text;
            let last = line_widths.len() - 1;
            if i == 0 || line_widths[last] + width + 2.0 * LEGEND_PADDING > max_width {
                total += line_height;
                if i > 0 {
                    line_widths.push(0.0);
                } else {
                    line_widths[last] = 0.0;
                }
            }
            let last = line_widths.len() - 1;
            line_widths[last] += width + LEGEND_PADDING;
        }
        self.legend.line_widths = line_widths;
        self.legend.height = min2(total + 10.0, max_height);
    }

    fn draw_legend(&mut self, env: &mut Env<'_>) {
        let Legend {
            left,
            top,
            right,
            bottom,
            ..
        } = self.legend;
        let ctx = &mut self.surface;
        ctx.clip_area(left, top, right, bottom);
        ctx.text_align("left");
        ctx.text_baseline("middle");
        ctx.line_width(0.5);
        ctx.font(FONT);
        let half = FONT_SIZE / 2.0;
        let widths = &self.legend.line_widths;
        let start = |line: usize| {
            let width = widths.get(line).copied().unwrap_or(f64::NAN);
            (left + LEGEND_PADDING + (right - width)) / 2.0
        };
        let (mut line, mut cursor_x, mut cursor_y) = (0, start(0), top + LEGEND_PADDING + 0.0);
        for (i, meta) in self.sorted_metasets.iter().enumerate() {
            let dataset = meta.dataset();
            ctx.stroke_style(&self.legend_color);
            ctx.fill_style(&self.legend_color);
            let text_width = ctx.measure(env.text, &dataset.label);
            let width = LEGEND_BOX_WIDTH + half + text_width;
            if i > 0 && cursor_x + width + LEGEND_PADDING > right {
                cursor_y += FONT_SIZE + LEGEND_PADDING;
                line += 1;
                cursor_x = start(line);
            }
            ctx.save();
            ctx.fill_style(&dataset.background);
            ctx.set(Property::LineCap("butt".to_owned()));
            ctx.set(Property::LineDashOffset(Some(0.0)));
            ctx.set(Property::LineJoin("miter".to_owned()));
            ctx.line_width(0.0);
            ctx.stroke_style(DEFAULT_BORDER);
            ctx.line_dash();
            ctx.begin_path();
            ctx.rect(cursor_x, cursor_y + 0.0, LEGEND_BOX_WIDTH, FONT_SIZE);
            ctx.fill();
            ctx.restore();
            ctx.save();
            ctx.font(FONT);
            ctx.fill_text(
                dataset.label.clone(),
                cursor_x + LEGEND_BOX_WIDTH + half,
                cursor_y + FONT_SIZE / 2.0,
            );
            ctx.restore();
            cursor_x += width + LEGEND_PADDING;
        }
        ctx.restore();
    }

    /// `computeMinSampleSize`.
    fn min_sample_size(&self) -> f64 {
        let mut values: Vec<f64> = self
            .sorted_metasets
            .iter()
            .flat_map(|meta| {
                let parsed = meta.parsed.borrow();
                let xs: Vec<f64> = parsed.iter().filter_map(|p| p.x).collect();
                xs
            })
            .collect();
        array_sort(&mut values, |a, b| a - b);
        values.dedup_by(|a, b| a == b);
        let mut min = self.x.length();
        let mut scan = |pixels: &mut dyn Iterator<Item = f64>| {
            let mut prev: Option<f64> = None;
            for curr in pixels {
                if curr == 32767.0 || curr == -32768.0 {
                    continue;
                }
                if let Some(prev) = prev {
                    min = min2(min, truthy_or(math::abs(curr - prev), min));
                }
                prev = Some(curr);
            }
        };
        scan(&mut values.iter().map(|&v| self.x.pixel_for_value(Some(v))));
        scan(&mut (0..self.x.ticks.len()).map(|i| self.x.pixel_for_tick(i)));
        min
    }

    /// `updateElements` of every controller.
    fn update_elements(&mut self) {
        if self.sorted_metasets.is_empty() {
            return;
        }
        let size = self.min_sample_size() * 1.0;
        let base_at_zero = self.y.pixel_for_value(Some(0.0));
        for (position, meta) in self.sorted_metasets.iter().enumerate() {
            let parsed = meta.parsed.borrow();
            for (item, bar) in parsed.iter().zip(meta.data.borrow().iter()) {
                let value = item.y;
                let mut start = 0.0;
                let mut length = apply_stack(item, &self.sorted_metasets[..position]);
                if length != value {
                    start = length - value;
                    length = value;
                }
                let mut base = self.y.pixel_for_value(Some(start));
                let head = self.y.pixel_for_value(Some(start + length));
                let mut bar_size = head - base;
                if base == base_at_zero {
                    let half_grid = math::sign(bar_size) * self.y.line_width_for_value(0.0) / 2.0;
                    base += half_grid;
                    bar_size -= half_grid;
                }
                let chunk = size / 1.0;
                let left = self.x.pixel_for_value(item.x) - size / 2.0;
                let mut bar = bar.borrow_mut();
                bar.x = left + chunk * 0.0 + chunk / 2.0;
                bar.width = min2(f64::INFINITY, chunk * 1.0);
                bar.y = head;
                bar.base = base;
                bar.height = math::abs(bar_size);
            }
        }
    }

    fn render(&mut self, env: &mut Env<'_>) -> Result<(), JsError> {
        if !self.animator.has_items() {
            return self.draw(env);
        }
        if self.attached && !self.animator.is_running() && self.animator.start() {
            self.request(Callback::Animate);
        }
        Ok(())
    }

    fn draw(&mut self, env: &mut Env<'_>) -> Result<(), JsError> {
        if let Some(size) = self.resize_before_draw {
            self.resize_now(size, env)?;
            self.resize_before_draw = None;
        }
        self.surface.clear();
        if self.width <= 0.0 || self.height <= 0.0 {
            return Ok(());
        }
        let Some(area) = self.area else {
            return Ok(());
        };
        let view = view!(self);
        self.x.draw_grid(&area, &view, &mut self.surface);
        self.y.draw_grid(&area, &view, &mut self.surface);
        self.draw_legend(env);
        let view = view!(self);
        self.x.draw_border(&view, &mut self.surface);
        self.x.draw_labels(&view, &mut self.surface, env.text);
        self.y.draw_border(&view, &mut self.surface);
        self.y.draw_labels(&view, &mut self.surface, env.text);
        for meta in self.sorted_metasets.iter().rev() {
            self.surface.clip_area(
                self.x.left - 0.0,
                self.y.top - 0.0,
                self.x.right + 0.0,
                self.y.bottom + 0.0,
            );
            for bar in meta.data.borrow().iter() {
                bar.borrow().draw(&mut self.surface);
            }
            self.surface.restore();
        }
        let bounds = Bounds {
            width: self.width,
            height: self.height,
            area: &area,
        };
        let created = self
            .tooltip
            .draw(&bounds, &mut self.surface, env.text, env.now);
        self.animator.add(created);
        Ok(())
    }

    fn point_in_area(&self, x: f64, y: f64) -> bool {
        let margin = 0.5;
        self.area.is_none_or(|a| {
            x > a.left - margin
                && x < a.right + margin
                && y > a.top - margin
                && y < a.bottom + margin
        })
    }

    /// The `nearest` interaction mode with `intersect`.
    fn nearest(&self, x: f64, y: f64) -> Vec<ActiveElement> {
        let mut items: Vec<ActiveElement> = Vec::new();
        if !self.point_in_area(x, y) {
            return items;
        }
        let mut min_distance = f64::INFINITY;
        for meta in &self.sorted_metasets {
            for (index, element) in meta.data.borrow().iter().enumerate() {
                let bar = element.borrow();
                if !bar.in_range(x, y) {
                    continue;
                }
                let center = (bar.x, (bar.y + bar.base) / 2.0);
                let dx = math::abs(x - center.0);
                let dy = math::abs(y - center.1);
                let distance = math::sqrt(math::pow(dx, 2.0) + math::pow(dy, 2.0));
                if distance < min_distance {
                    items.clear();
                    min_distance = distance;
                } else if distance != min_distance {
                    continue;
                }
                items.push(ActiveElement {
                    element: element.clone(),
                    dataset_index: meta.index.get(),
                    index,
                });
            }
        }
        items
    }

    fn active_elements(
        &self,
        event: Event,
        last: &[ActiveElement],
        in_area: bool,
    ) -> Vec<ActiveElement> {
        if event.kind == EventKind::MouseOut {
            return Vec::new();
        }
        if !in_area {
            return last.to_vec();
        }
        self.nearest(event.x, event.y)
    }

    fn event_handler(
        &mut self,
        event: Event,
        replay: bool,
        env: &mut Env<'_>,
    ) -> Result<(), JsError> {
        let in_area = self.point_in_area(event.x, event.y);
        let changed = self.handle_event(event, replay, in_area, env.now);
        let tooltip_changed = self.tooltip_event(event, replay, in_area, env)?;
        if changed || tooltip_changed {
            self.render(env)?;
        }
        Ok(())
    }

    fn handle_event(&mut self, event: Event, replay: bool, in_area: bool, now: f64) -> bool {
        let last = self.active.clone();
        let active = self.active_elements(event, &last, in_area);
        let last_event = if !in_area || event.kind == EventKind::MouseOut {
            None
        } else if event.kind == EventKind::Click {
            self.last_event
        } else {
            Some(event)
        };
        let changed = !elements_equal(&active, &last);
        if changed || replay {
            self.active.clone_from(&active);
            self.update_hover_styles(&active, &last, replay, now);
        }
        self.last_event = last_event;
        changed
    }

    fn update_hover_styles(
        &mut self,
        active: &[ActiveElement],
        last: &[ActiveElement],
        replay: bool,
        now: f64,
    ) {
        let diff = |a: &[ActiveElement], b: &[ActiveElement]| -> Vec<ActiveElement> {
            a.iter()
                .filter(|x| !b.iter().any(|y| same_element(x, y)))
                .cloned()
                .collect()
        };
        let deactivated = diff(last, active);
        let activated = if replay {
            active.to_vec()
        } else {
            diff(active, last)
        };
        for (items, enabled) in [(deactivated, false), (activated, true)] {
            for item in items {
                let meta = self.dataset_meta(item.dataset_index);
                if meta.has_controller.get() {
                    let created = meta.set_style(&item.element, enabled, now);
                    self.animator.add(created);
                }
            }
        }
    }

    /// `Tooltip.handleEvent`.
    fn tooltip_event(
        &mut self,
        event: Event,
        replay: bool,
        in_area: bool,
        env: &mut Env<'_>,
    ) -> Result<bool, JsError> {
        let last = std::mem::take(&mut self.tooltip.active);
        let active = self.active_elements(event, &last, in_area);
        let changed =
            self.tooltip.position_changed(&active) || replay || !elements_equal(&active, &last);
        if !changed {
            self.tooltip.active = last;
            return Ok(false);
        }
        self.tooltip.active.clone_from(&active);
        let Some(position) = tooltip::average(&active) else {
            let created = self.tooltip.hide(env.now);
            self.animator.add(created);
            return Ok(true);
        };
        let mut items = Vec::with_capacity(active.len());
        for item in &active {
            items.push(self.tooltip_item(item)?);
        }
        let area = self.area.unwrap_or_default();
        let bounds = Bounds {
            width: self.width,
            height: self.height,
            area: &area,
        };
        let created = self.tooltip.show(
            position,
            items,
            &bounds,
            &mut self.surface,
            env.text,
            env.now,
        );
        self.animator.add(created);
        Ok(true)
    }

    /// `createTooltipItem` and the title, label and colour callbacks.
    fn tooltip_item(&mut self, item: &ActiveElement) -> Result<Item, JsError> {
        let meta = self.dataset_meta(item.dataset_index);
        if !meta.has_controller.get() {
            return Err(JsError::type_error(
                "Cannot read properties of null (reading 'getLabelAndValue')",
            ));
        }
        let parsed = meta.parsed.borrow();
        let parsed = parsed.get(item.index).ok_or_else(|| {
            JsError::type_error("Cannot read properties of undefined (reading '_custom')")
        })?;
        let x = parsed.x.unwrap_or(0.0);
        let half = self.step_size / 2.0;
        let title = format!(
            "{} to {}",
            number::to_string(clean_float(x - half)),
            number::to_string(clean_float(x + half))
        );
        let dataset = meta.dataset();
        let mut label = dataset.label.clone();
        if !label.units().is_empty() {
            label.push_str(&JsString::from(": "));
        }
        let value = number::intl_format(parsed.y, IntlOptions::default());
        label.push_str(&JsString::from(value));
        Ok(Item {
            title: JsString::from(title),
            label,
            background: dataset.background.clone(),
        })
    }
}

fn set_box(scale: &mut Scale, left: f64, top: f64, width: Option<f64>, height: Option<f64>) {
    let width = width.unwrap_or(scale.width);
    let height = height.unwrap_or(scale.height);
    scale.left = left;
    scale.top = top;
    scale.right = left + width;
    scale.bottom = top + height;
    scale.width = width;
    scale.height = height;
}
