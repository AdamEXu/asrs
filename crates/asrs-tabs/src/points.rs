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
use crate::list_cx::{SourceListCx, js_error, with_env};
use crate::select::Select;

fn to_number(value: &JsString) -> f64 {
    number::to_number(value.units())
}

pub struct PointsController {
    source_list: SourceList,
    /// `WIDTH.value`: the `type="number"` input's string, `"960"` in markup.
    width: JsString,
    height: JsString,
    orientation: Select,
    origin: Select,
}

impl PointsController {
    /// The source list draws one `createUUID()` after the controller's own.
    pub fn new<Cx: TabCx + ?Sized>(cx: &mut Cx) -> PointsController {
        let uuid = asrs_js::time::create_uuid(cx.tab_rng());
        PointsController {
            source_list: SourceList::new(configs::points(), uuid, false),
            width: JsString::from("960"),
            height: JsString::from("720"),
            orientation: Select::with_options(
                ["xr,yd", "xr,yu", "xu,yl"].map(JsString::from).to_vec(),
            ),
            origin: Select::with_options(
                ["ul", "ur", "ll", "lr", "c"].map(JsString::from).to_vec(),
            ),
        }
    }

    pub fn source_list(&self) -> &SourceList {
        &self.source_list
    }

    pub fn source_list_mut(&mut self) -> &mut SourceList {
        &mut self.source_list
    }

    /// An `input.value`: `"width"`, `"height"`, `"orientation"` or `"origin"`.
    pub fn input_value(&self, input: &str) -> Option<JsString> {
        match input {
            "width" => Some(self.width.clone()),
            "height" => Some(self.height.clone()),
            "orientation" => Some(self.orientation.value()),
            "origin" => Some(self.origin.value()),
            _ => None,
        }
    }

    /// `input.value = value`.
    pub fn set_input(&mut self, input: &str, value: JsString) {
        match input {
            "width" => self.width = value,
            "height" => self.height = value,
            "orientation" => self.orientation.set_value(&value),
            "origin" => self.origin.set_value(&value),
            _ => {}
        }
    }

    /// The `change` listener `WIDTH` and `HEIGHT` share (`index` is never 2, so the
    /// `< 0` branch is unreachable upstream).
    pub fn dimension_changed(&mut self, input: &str) {
        let value = match self.input_value(input) {
            Some(v) if input == "width" || input == "height" => v,
            _ => return,
        };
        if math::rem(to_number(&value), 1.0) != 0.0 {
            self.set_input(
                input,
                JsString::from(number::to_string(math::round(to_number(&value)))),
            );
        }
        let value = self.input_value(input).unwrap_or_default();
        if to_number(&value) <= 0.0 {
            self.set_input(input, JsString::from("1"));
        }
    }
}

fn source_state(value: &JsValue) -> Result<Vec<SourceListItemState>, JsError> {
    value
        .as_array()
        .ok_or_else(|| JsError::new("", "restored sources that are not an array"))?
        .iter()
        .map(|item| {
            SourceListItemState::from_js(item)
                .ok_or_else(|| JsError::new("", "a restored source item of another shape"))
        })
        .collect()
}

fn point_to_js(point: [f64; 2]) -> JsValue {
    vec![point[0].to_js(), point[1].to_js()].to_js()
}

fn geometry_error(e: asrs_log::geometry::GeometryError) -> JsError {
    match e {
        asrs_log::geometry::GeometryError::RangeError(m) => JsError::range(m),
        asrs_log::geometry::GeometryError::TypeError(m) => JsError::type_error(m),
    }
}

impl<Cx: SourceListCx + ?Sized> TabController<Cx> for PointsController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        let mut o = JsObject::new();
        o.insert("sources", self.source_list.state().to_js());
        o.insert("width", to_number(&self.width).to_js());
        o.insert("height", to_number(&self.height).to_js());
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
        if let Some(w) = state.get("width").and_then(JsValue::as_f64) {
            self.width = JsString::from(number::to_string(w));
        }
        if let Some(h) = state.get("height").and_then(JsValue::as_f64) {
            self.height = JsString::from(number::to_string(h));
        }
        if let Some(JsValue::String(o)) = state.get("orientation") {
            self.orientation.set_value(o);
        }
        if let Some(JsValue::String(o)) = state.get("origin") {
            self.origin.set_value(o);
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
        let mut time = cx.tab_selection().get_render_time();
        let log = cx.tab_log();
        if time.is_none() {
            time = Some(log.get_timestamp_range()[1]);
        }
        let time = time.unwrap();
        let uuid = uuid.as_str().and_then(|s| s.as_string());
        let uuid = uuid.as_deref();
        let config = configs::points();
        let child_of = |key: &JsString| -> Option<()> {
            config
                .types
                .iter()
                .find(|t| t.key == key.as_string().unwrap_or_default())
                .and_then(|t| t.child_of.as_ref())
                .map(|_| ())
        };

        let mut sets: Vec<JsValue> = Vec::new();
        let sources = self.source_list.displayed_state(&log);
        let mut i = 0;
        while i < sources.len() {
            let source = &sources[i];
            if child_of(&source.item_type).is_some() {
                i += 1;
                continue;
            }
            let mut children: Vec<&SourceListItemState> = Vec::new();
            while sources.len() > i + 1 && child_of(&sources[i + 1].item_type).is_some() {
                i += 1;
                children.push(&sources[i]);
            }

            let type_key = source.item_type.as_string().unwrap_or_default();
            let shape = if type_key.starts_with("plus") {
                "plus"
            } else if type_key.starts_with("cross") {
                "cross"
            } else {
                "circle"
            };
            let size = match source
                .options
                .get("size")
                .and_then(|s| s.as_string())
                .unwrap_or_default()
                .as_str()
            {
                s @ ("small" | "medium" | "large") => JsString::from(s),
                _ => JsString::from("medium"),
            };
            let group_size = source.options.get("groupSize").map_or(0.0, to_number);

            let mut points: Vec<[f64; 2]> = Vec::new();
            match type_key.as_str() {
                "plus" | "cross" | "circle" => {
                    let poses = grab_poses_auto(
                        &log,
                        &source.log_key,
                        &source.log_type,
                        time,
                        uuid,
                        RotationUnits::Radians,
                    )
                    .map_err(geometry_error)?;
                    points = poses
                        .iter()
                        .map(|p| translation_3d_to_2d(p.pose.translation))
                        .collect();
                }
                "plusSplit" | "crossSplit" | "circleSplit" => {
                    let values: Vec<f64> =
                        get_or_default(&log, &source.log_key, time, Vec::new(), uuid);
                    let x_component = source
                        .options
                        .get("component")
                        .is_some_and(|c| c == &JsString::from("x"));
                    points = values
                        .iter()
                        .map(|&v| if x_component { [v, 0.0] } else { [0.0, v] })
                        .collect();
                }
                _ => {}
            }

            for child in children {
                let values: Vec<f64> = get_or_default(&log, &child.log_key, time, Vec::new(), uuid);
                for (index, value) in values.iter().enumerate() {
                    if let Some(point) = points.get_mut(index) {
                        if child
                            .options
                            .get("component")
                            .is_some_and(|c| c == &JsString::from("x"))
                        {
                            point[0] = *value;
                        } else {
                            point[1] = *value;
                        }
                    }
                }
            }

            let mut o = JsObject::new();
            o.insert(
                "points",
                points
                    .iter()
                    .map(|p| point_to_js(*p))
                    .collect::<Vec<_>>()
                    .to_js(),
            );
            o.insert("shape", JsString::from(shape).to_js());
            o.insert("size", size.to_js());
            o.insert("groupSize", group_size.to_js());
            sets.push(JsValue::Object(o));
            i += 1;
        }

        let dimensions = [to_number(&self.width), to_number(&self.height)];
        for set in &mut sets {
            let JsValue::Object(o) = set else { continue };
            let Some(JsValue::Array(points)) = o.get_mut("points") else {
                continue;
            };
            for point in points.iter_mut() {
                let JsValue::Array(coords) = point else {
                    continue;
                };
                let (x, y) = (
                    coords.first().and_then(JsValue::as_f64).unwrap_or(f64::NAN),
                    coords.get(1).and_then(JsValue::as_f64).unwrap_or(f64::NAN),
                );
                let (mut x, mut y) = match self.orientation.value().as_string().as_deref() {
                    Some("xr,yu") => (x, -y),
                    Some("xu,yl") => (-y, -x),
                    _ => (x, y),
                };
                match self.origin.value().as_string().as_deref() {
                    Some("ur") => x += dimensions[0],
                    Some("ll") => y += dimensions[1],
                    Some("lr") => {
                        x += dimensions[0];
                        y += dimensions[1];
                    }
                    Some("c") => {
                        x += dimensions[0] / 2.0;
                        y += dimensions[1] / 2.0;
                    }
                    _ => {}
                }
                if coords.len() >= 2 {
                    coords[0] = x.to_js();
                    coords[1] = y.to_js();
                }
            }
        }
        sets.reverse();

        let mut o = JsObject::new();
        o.insert("dimensions", dimensions.to_vec().to_js());
        o.insert("sets", sets.to_js());
        Ok(Rc::new(JsValue::Object(o)))
    }
}
