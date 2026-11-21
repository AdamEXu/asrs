//! `src/hub/controllers/TabController.ts` and `src/shared/renderers/TabRenderer.ts`: the two
//! halves of every tab. A controller owns the tab's controls and produces one command per frame;
//! a renderer draws that command, in the hub or in a satellite window.
//!
//! `Cx` is whatever the hub hands every tab in place of upstream's `window.*` globals (log,
//! selection, preferences, assets). The traits only fix how `Tabs` calls into a tab; each
//! controller or renderer reads what it needs from `Cx` itself.
//!
//! An `Err` is an exception upstream code would throw: `Tabs` stops where upstream stops and
//! reports it as the hub would.

use std::any::Any;
use std::rc::Rc;

use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsValue, ToJs};

/// The concrete type behind a tab trait object. Use the `downcast_ref` and `downcast_mut`
/// methods on the trait objects: called on a `Box` or `Rc`, `as_any` would see the pointer.
pub trait AsAny {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// What `getCommand` returns. Renderers downcast it to their controller's command type;
/// `to_js` is the value upstream passes to `render` and posts to satellites.
pub trait TabCommand: AsAny {
    fn to_js(&self) -> JsValue;
}

impl<T: ToJs + Any> TabCommand for T {
    fn to_js(&self) -> JsValue {
        ToJs::to_js(self)
    }
}

/// Shared so one frame's command can go to the local renderer and every satellite.
pub type Command = Rc<dyn TabCommand>;

pub trait TabController<Cx: ?Sized>: AsAny {
    /// `saveState()`: the `controller` member of the tab's saved state.
    fn save_state(&self, cx: &Cx) -> JsValue;

    /// `restoreState(state)`, with the raw saved value.
    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError>;

    /// `refresh()`, after log fields change.
    fn refresh(&mut self, cx: &mut Cx) -> Result<(), JsError>;

    fn new_assets(&mut self, cx: &mut Cx) -> Result<(), JsError>;

    /// `getActiveFields()`: keys, and key prefixes, live sources should provide.
    fn active_fields(&self, cx: &Cx) -> Vec<JsString>;

    fn show_timeline(&self, cx: &Cx) -> bool;

    /// `getCommand()`, once per frame while the tab is shown anywhere. `uuid` is the tab's
    /// `controller.UUID`, which `Tabs` owns because a restore may set it to any value.
    fn command(&mut self, cx: &mut Cx, uuid: &JsValue) -> Result<Command, JsError>;

    /// `TableController.addRendererRange`. `Tabs` calls it only on Table tabs.
    fn add_renderer_range(
        &mut self,
        _cx: &mut Cx,
        _renderer_uuid: &JsString,
        _range: Option<[f64; 2]>,
    ) -> Result<(), JsError> {
        Ok(())
    }
}

pub trait TabRenderer<Cx: ?Sized>: AsAny {
    /// `saveState()`: the `renderer` member of the tab's saved state.
    fn save_state(&self, cx: &Cx) -> JsValue;

    /// `restoreState(state)`, with the raw saved value.
    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError>;

    /// `getAspectRatio()`, used to size satellite windows.
    fn aspect_ratio(&self, cx: &Cx) -> Option<f64>;

    /// `render(command)`, once per frame while the tab is shown.
    fn render(&mut self, cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError>;

    /// `TableRenderer.UUID`. `Tabs` reads it only on Table tabs.
    fn renderer_uuid(&self) -> Option<&JsString> {
        None
    }

    /// `TableRenderer.getTimestampRange()`. `Tabs` reads it only on Table tabs.
    fn timestamp_range(&self, _cx: &Cx) -> Option<[f64; 2]> {
        None
    }
}

macro_rules! downcasts {
    ($($object:ty),*) => {$(
        impl<'a, Cx: ?Sized> $object {
            pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
                self.as_any().downcast_ref()
            }

            pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
                self.as_any_mut().downcast_mut()
            }
        }
    )*};
}

downcasts!(dyn TabController<Cx> + 'a, dyn TabRenderer<Cx> + 'a);

impl<'a> dyn TabCommand + 'a {
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref()
    }
}

/// `setupKeyboardControls`: whether a `keydown` with this `key` clicks the element.
pub fn keyboard_clicks(key: &JsString) -> bool {
    *key == JsString::from("Enter") || *key == JsString::from(" ")
}

/// `NoopController`: Documentation tabs, and tabs restored with a type the switch in
/// `Tabs.addTab` does not know.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopController;

impl<Cx: ?Sized> TabController<Cx> for NoopController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn refresh(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        Vec::new()
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        false
    }

    fn command(&mut self, _cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        Ok(Rc::new(JsValue::Null))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopRenderer;

impl<Cx: ?Sized> TabRenderer<Cx> for NoopRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, _cx: &mut Cx, _command: &dyn TabCommand) -> Result<(), JsError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_halves_match_upstream() {
        let mut c = NoopController;
        let cx = &mut ();
        assert_eq!(TabController::<()>::save_state(&c, cx), JsValue::Null);
        assert!(!TabController::<()>::show_timeline(&c, cx));
        assert!(TabController::<()>::active_fields(&c, cx).is_empty());
        let command = TabController::<()>::command(&mut c, cx, &JsValue::Undefined).unwrap();
        assert_eq!(command.to_js(), JsValue::Null);
        let r = NoopRenderer;
        assert_eq!(TabRenderer::<()>::save_state(&r, cx), JsValue::Null);
        assert_eq!(TabRenderer::<()>::aspect_ratio(&r, cx), None);
    }

    #[test]
    fn commands_downcast_to_their_type() {
        let command: Command = Rc::new(JsValue::Number(3.0));
        assert_eq!(
            command.downcast_ref::<JsValue>(),
            Some(&JsValue::Number(3.0))
        );
        let boxed: Box<dyn TabController<()>> = Box::new(NoopController);
        assert!(boxed.downcast_ref::<NoopController>().is_some());
    }
}
