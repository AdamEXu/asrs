//! `src/hub/controllers/PointsController.ts`.

use std::rc::Rc;

use asrs_js::math;
use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::geometry::{RotationUnits, grab_poses_auto, translation_3d_to_2d};
use asrs_log::log::log_util::get_or_default;
use asrs_model::source_list::SourceList;
use asrs_model::source_list::configs;
use asrs_model::source_list_config::SourceListItemState;
use asrs_model::tab_controller::{Command, TabController};

use crate::cx::TabCx;
use crate::form::{NumberInput, Select};
use crate::list_cx::{SourceListCx, js_error, source_state, with_env};

/// `WIDTH` and `HEIGHT`, in the order of upstream's `forEach` (`index` 0 and 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    Width,
    Height,
}

/// `ORIENTATION` and `ORIGIN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointsSelect {
    Orientation,
    Origin,
}

pub struct PointsController {
    source_list: SourceList,
    width: NumberInput,
    height: NumberInput,
    orientation: Select,
    origin: Select,
}

struct PointSet {
    points: Vec<[f64; 2]>,
    shape: &'static str,
    size: &'static str,
    group_size: f64,
}

impl ToJs for PointSet {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert(
            "points",
            self.points
                .iter()
                .map(|p| p.to_vec().to_js())
                .collect::<Vec<_>>()
                .to_js(),
        );
        o.insert("shape", JsValue::from(self.shape));
        o.insert("size", JsValue::from(self.size));
        o.insert("groupSize", self.group_size.to_js());
        JsValue::Object(o)
    }
}

impl PointsController {
    /// The source list draws one `createUUID()` after the controller's own.
    pub fn new<Cx: TabCx + ?Sized>(cx: &mut Cx) -> PointsController {
        let uuid = asrs_js::time::create_uuid(cx.tab_rng());
        let options = |values: &[&str]| values.iter().copied().map(JsString::from).collect();
        PointsController {
            source_list: SourceList::new(configs::points(), uuid, false),
            width: NumberInput::new("960"),
            height: NumberInput::new("720"),
            orientation: Select::with_options(options(&["xr,yd", "xr,yu", "xu,yl"])),
            origin: Select::with_options(options(&["ul", "ur", "ll", "lr", "c"])),
        }
    }

    pub fn source_list(&self) -> &SourceList {
        &self.source_list
    }

    pub fn source_list_mut(&mut self) -> &mut SourceList {
        &mut self.source_list
    }

    pub fn dimension(&self, dimension: Dimension) -> &NumberInput {
        match dimension {
            Dimension::Width => &self.width,
            Dimension::Height => &self.height,
        }
    }

    pub fn dimension_mut(&mut self, dimension: Dimension) -> &mut NumberInput {
        match dimension {
            Dimension::Width => &mut self.width,
            Dimension::Height => &mut self.height,
        }
    }

    pub fn select(&self, select: PointsSelect) -> &Select {
        match select {
            PointsSelect::Orientation => &self.orientation,
            PointsSelect::Origin => &self.origin,
        }
    }

    pub fn select_mut(&mut self, select: PointsSelect) -> &mut Select {
        match select {
            PointsSelect::Orientation => &mut self.orientation,
            PointsSelect::Origin => &mut self.origin,
        }
    }

    /// The `change` listener `WIDTH` and `HEIGHT` share. Its `index === 2` branch is
    /// unreachable upstream (the array has two inputs).
    pub fn dimension_changed(&mut self, dimension: Dimension) {
        let input = self.dimension_mut(dimension);
        if math::rem(input.number(), 1.0) != 0.0 {
            let rounded = number::to_string(math::round(input.number()));
            input.set_value(JsString::from(rounded));
        }
        if input.number() <= 0.0 {
            input.set_value(JsString::from("1"));
        }
    }

    fn is_child(&self, item: &SourceListItemState) -> bool {
        self.source_list
            .config()
            .types
            .iter()
            .find(|t| item.item_type == JsString::from(t.key.as_str()))
            .is_some_and(|t| t.child_of.is_some())
    }

    fn transform(&self, [x, y]: [f64; 2], dimensions: [f64; 2]) -> [f64; 2] {
        let orientation = self.orientation.value();
        let [x, y] = if orientation == JsString::from("xr,yu") {
            [x, -y]
        } else if orientation == JsString::from("xu,yl") {
            [-y, -x]
        } else {
            [x, y]
        };
        match self.origin.value().as_string().as_deref() {
            Some("ur") => [x + dimensions[0], y],
            Some("ll") => [x, y + dimensions[1]],
            Some("lr") => [x + dimensions[0], y + dimensions[1]],
            Some("c") => [x + dimensions[0] / 2.0, y + dimensions[1] / 2.0],
            _ => [x, y],
        }
    }
}

fn option_is(item: &SourceListItemState, key: &str, value: &str) -> bool {
    item.options
        .get(key)
        .is_some_and(|v| v == &JsString::from(value))
}

impl<Cx: SourceListCx + ?Sized> TabController<Cx> for PointsController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        let mut o = JsObject::new();
        o.insert("sources", self.source_list.state().to_js());
        o.insert("width", self.width.number().to_js());
        o.insert("height", self.height.number().to_js());
        o.insert("orientation", self.orientation.value().to_js());
        o.insert("origin", self.origin.value().to_js());
        JsValue::Object(o)
    }

    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        let JsValue::Object(state) = state else {
            return Ok(());
        };
        if let Some(sources) = state.get("sources") {
            let items = source_state(sources)?;
            with_env(cx, |env| self.source_list.set_state(env, items)).map_err(js_error)?;
        }
        for (key, dimension) in [("width", Dimension::Width), ("height", Dimension::Height)] {
            if let Some(n) = state.get(key).and_then(JsValue::as_f64) {
                self.dimension_mut(dimension)
                    .set_value(JsString::from(number::to_string(n)));
            }
        }
        for (key, select) in [
            ("orientation", PointsSelect::Orientation),
            ("origin", PointsSelect::Origin),
        ] {
            if let Some(JsValue::String(value)) = state.get(key) {
                self.select_mut(select).set_value(value);
            }
        }
        Ok(())
    }

    fn refresh(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        with_env(cx, |env| self.source_list.refresh(env)).map_err(js_error)
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        self.source_list.active_fields()
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        true
    }

    fn command(&mut self, cx: &mut Cx, uuid: &JsValue) -> Result<Command, JsError> {
        let time = cx.tab_selection().get_render_time();
        let log = cx.tab_log();
        let time = time.unwrap_or_else(|| log.get_timestamp_range()[1]);
        let uuid = uuid.as_str().and_then(|s| s.as_string());
        let uuid = uuid.as_deref();

        let mut sets: Vec<PointSet> = Vec::new();
        let sources = self.source_list.displayed_state(&log);
        let mut i = 0;
        while i < sources.len() {
            let source = &sources[i];
            if self.is_child(source) {
                i += 1;
                continue;
            }
            let mut children = Vec::new();
            while sources.len() > i + 1 && self.is_child(&sources[i + 1]) {
                i += 1;
                children.push(&sources[i]);
            }
            i += 1;

            let type_key = source.item_type.as_string().unwrap_or_default();
            let shape = ["circle", "cross", "plus"]
                .into_iter()
                .find(|shape| type_key.starts_with(shape))
                .unwrap_or("plus");
            let size = match source.options.get("size") {
                None => "medium",
                Some(raw) => ["small", "medium", "large"]
                    .into_iter()
                    .find(|s| raw == &JsString::from(*s))
                    .unwrap_or("medium"),
            };
            let group_size = source
                .options
                .get("groupSize")
                .map_or(0.0, |g| number::to_number(g.units()));

            let points = match type_key.as_str() {
                "plus" | "cross" | "circle" => grab_poses_auto(
                    &log,
                    &source.log_key,
                    &source.log_type,
                    time,
                    uuid,
                    RotationUnits::Radians,
                )?
                .iter()
                .map(|p| translation_3d_to_2d(p.pose.translation))
                .collect(),
                "plusSplit" | "crossSplit" | "circleSplit" => {
                    let values: Vec<f64> =
                        get_or_default(&log, &source.log_key, time, Vec::new(), uuid);
                    let x = option_is(source, "component", "x");
                    values
                        .into_iter()
                        .map(|v| if x { [v, 0.0] } else { [0.0, v] })
                        .collect()
                }
                _ => {
                    return Err(JsError::new(
                        "Unmodeled",
                        "points: a parent type outside the switch reuses the previous set",
                    ));
                }
            };
            sets.push(PointSet {
                points,
                shape,
                size,
                group_size,
            });

            let points = &mut sets.last_mut().expect("pushed above").points;
            for child in children {
                let values: Vec<f64> = get_or_default(&log, &child.log_key, time, Vec::new(), uuid);
                let axis = if option_is(child, "component", "x") {
                    0
                } else {
                    1
                };
                for (point, value) in points.iter_mut().zip(values) {
                    point[axis] = value;
                }
            }
        }

        let dimensions = [self.width.number(), self.height.number()];
        for set in &mut sets {
            for point in &mut set.points {
                *point = self.transform(*point, dimensions);
            }
        }
        sets.reverse();

        let mut o = JsObject::new();
        o.insert("dimensions", dimensions.to_vec().to_js());
        o.insert(
            "sets",
            sets.iter().map(ToJs::to_js).collect::<Vec<_>>().to_js(),
        );
        Ok(Rc::new(JsValue::Object(o)))
    }
}
