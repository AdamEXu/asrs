// Ported from Chart.js 4.4.0 (MIT, Copyright (c) 2014-2022 Chart.js Contributors). See THIRD_PARTY_NOTICES.md.

//! `Animation` and the per-chart half of `Animator`.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::math::{self, max2, min2};

use super::chart::BarOptions;
use super::color;
use super::tooltip::{Prop, Props};

#[derive(Clone, Copy, PartialEq)]
pub enum Easing {
    Linear,
    EaseOutQuart,
}

impl Easing {
    fn apply(self, t: f64) -> f64 {
        match self {
            Easing::Linear => t,
            Easing::EaseOutQuart => {
                let t = t - 1.0;
                -(t * t * t * t - 1.0)
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Config {
    pub duration: f64,
    pub easing: Easing,
}

#[derive(Clone)]
pub enum Target {
    Background(Rc<RefCell<BarOptions>>),
    Border(Rc<RefCell<BarOptions>>),
    Tooltip(Rc<RefCell<Props>>, Prop),
}

#[derive(Clone, PartialEq)]
pub enum Value {
    Number(f64),
    Color(String),
}

impl Target {
    fn get(&self) -> Option<Value> {
        match self {
            Target::Background(o) => Some(Value::Color(o.borrow().background.clone())),
            Target::Border(o) => Some(Value::Color(o.borrow().border.clone())),
            Target::Tooltip(props, prop) => props.borrow().get(*prop).map(Value::Number),
        }
    }

    fn set(&self, value: Value) {
        match (self, value) {
            (Target::Background(o), Value::Color(c)) => o.borrow_mut().background = c,
            (Target::Border(o), Value::Color(c)) => o.borrow_mut().border = c,
            (Target::Tooltip(props, prop), Value::Number(v)) => props.borrow_mut().set(*prop, v),
            _ => unreachable!("a colour is animated to a colour and a number to a number"),
        }
    }
}

pub struct Animation {
    active: bool,
    easing: Easing,
    start: f64,
    duration: f64,
    total: f64,
    target: Target,
    from: Value,
    to: Value,
}

pub type AnimationRef = Rc<RefCell<Animation>>;

impl Animation {
    pub fn new(config: Config, target: Target, to: Value, now: f64) -> AnimationRef {
        let duration = math::floor(config.duration);
        Rc::new(RefCell::new(Animation {
            active: true,
            easing: config.easing,
            start: math::floor(now),
            duration,
            total: duration,
            from: target.get().unwrap_or_else(|| to.clone()),
            target,
            to,
        }))
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn to(&self) -> &Value {
        &self.to
    }

    pub fn update(&mut self, config: Config, to: Value, date: f64) {
        if !self.active {
            return;
        }
        let elapsed = date - self.start;
        let remain = self.duration - elapsed;
        self.start = date;
        self.duration = math::floor(max2(remain, config.duration));
        self.total += elapsed;
        self.from = self.target.get().unwrap_or_else(|| to.clone());
        self.to = to;
    }

    fn tick(&mut self, date: f64) {
        let elapsed = date - self.start;
        self.active = self.from != self.to && elapsed < self.duration;
        if !self.active {
            self.target.set(self.to.clone());
            return;
        }
        if elapsed < 0.0 {
            self.target.set(self.from.clone());
            return;
        }
        let factor = math::rem(elapsed / self.duration, 2.0);
        let factor = self.easing.apply(min2(1.0, max2(0.0, factor)));
        let value = match (&self.from, &self.to) {
            (Value::Number(from), Value::Number(to)) => Value::Number(from + (to - from) * factor),
            (Value::Color(from), Value::Color(to)) => {
                Value::Color(color::interpolate(from, to, factor))
            }
            _ => unreachable!("an animation's ends have one type"),
        };
        self.target.set(value);
    }
}

/// Starts or redirects the animation of one property: `_createAnimations` for one key.
pub fn animate(
    running: &mut Option<AnimationRef>,
    config: Config,
    target: impl FnOnce() -> Target,
    to: Value,
    now: f64,
    created: &mut Vec<AnimationRef>,
) {
    if let Some(animation) = running
        && animation.borrow().is_active()
    {
        animation.borrow_mut().update(config, to, now);
        return;
    }
    let animation = Animation::new(config, target(), to, now);
    *running = Some(animation.clone());
    created.push(animation);
}

#[derive(Default)]
pub struct Animator {
    items: Vec<AnimationRef>,
    chart_running: bool,
    running: bool,
    requested: bool,
}

impl Animator {
    pub fn add(&mut self, animations: Vec<AnimationRef>) {
        self.items.extend(animations);
    }

    pub fn has_items(&self) -> bool {
        !self.items.is_empty()
    }

    pub fn is_running(&self) -> bool {
        self.running && self.chart_running && !self.items.is_empty()
    }

    /// Returns whether an animation frame is to be requested.
    pub fn start(&mut self) -> bool {
        self.chart_running = true;
        self.refresh()
    }

    fn refresh(&mut self) -> bool {
        if self.requested {
            return false;
        }
        self.running = true;
        self.requested = true;
        true
    }

    /// The first half of `_update`: ticks the items and returns whether the chart is to draw.
    pub fn tick(&mut self, date: f64) -> bool {
        if !self.chart_running || self.items.is_empty() {
            return false;
        }
        let mut draw = false;
        for i in (0..self.items.len()).rev() {
            let active = self.items[i].borrow().active;
            if active {
                self.items[i].borrow_mut().tick(date);
                draw = true;
            } else {
                self.items.swap_remove(i);
            }
        }
        draw
    }

    /// The rest of `_update` and of its frame callback; returns whether another frame is to be
    /// requested.
    pub fn finish(&mut self) -> bool {
        if self.chart_running && self.items.is_empty() {
            self.chart_running = false;
        }
        if self.items.is_empty() {
            self.running = false;
        }
        self.requested = false;
        self.running && self.refresh()
    }
}
