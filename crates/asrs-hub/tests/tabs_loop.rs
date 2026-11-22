//! `Tabs`' animation-frame loop ends at the first throw from a controller's `getCommand` or a
//! renderer's `render`, as upstream's `periodic` skips its `requestAnimationFrame` then
//! (`parity/vectors/dom/line-graph/unknown-unit` records the renderer case: one
//! `requestAnimationFrame` exception, then no further command from the loop).

use std::rc::Rc;

use asrs_hub::tabs::{TabFactory, TabParts, Tabs, TabsError, TabsHost};
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{Command, NoopController, TabCommand, TabController, TabRenderer};
use asrs_model::tab_type::TabType;

#[derive(Default)]
struct Page {
    commands: usize,
    renders: usize,
    fail_render: bool,
    fail_command: bool,
}

impl TabsHost for Page {
    fn inner_height(&self) -> f64 {
        650.0
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        92.0
    }
}

struct Counting;

impl TabController<Page> for Counting {
    fn save_state(&self, _cx: &Page) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Page, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn refresh(&mut self, _cx: &mut Page) -> Result<(), JsError> {
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Page) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, _cx: &Page) -> Vec<JsString> {
        Vec::new()
    }

    fn show_timeline(&self, _cx: &Page) -> bool {
        true
    }

    fn command(&mut self, cx: &mut Page, _uuid: &JsValue) -> Result<Command, JsError> {
        cx.commands += 1;
        if cx.fail_command {
            return Err(JsError::type_error("command"));
        }
        Ok(Rc::new(JsValue::Null))
    }
}

struct Throwing;

impl TabRenderer<Page> for Throwing {
    fn save_state(&self, _cx: &Page) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Page, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Page) -> Option<f64> {
        None
    }

    fn render(&mut self, cx: &mut Page, _command: &dyn TabCommand) -> Result<(), JsError> {
        cx.renders += 1;
        if cx.fail_render {
            return Err(JsError::type_error(
                "Cannot read properties of undefined (reading 'suffix')",
            ));
        }
        Ok(())
    }
}

struct Factory;

impl TabFactory<Page> for Factory {
    fn create(
        &mut self,
        _cx: &mut Page,
        tab_type: Option<TabType>,
    ) -> Result<TabParts<Page>, JsError> {
        let controller: Box<dyn TabController<Page>> = match tab_type {
            Some(TabType::LineGraph) => Box::new(Counting),
            _ => Box::new(NoopController),
        };
        Ok(TabParts {
            uuid: JsString::from("tab"),
            controller,
            renderer: Box::new(Throwing),
        })
    }
}

fn tabs(page: &mut Page) -> Tabs<Page> {
    let tabs = Tabs::new(Box::new(Factory), page);
    assert_eq!(tabs.selected_tab_type(), Ok(&JsValue::Number(1.0)));
    tabs
}

#[test]
fn a_renderer_throw_ends_the_frame_loop() {
    let mut page = Page::default();
    let mut tabs = tabs(&mut page);
    assert!(tabs.periodic(&mut page, 0.0).error.is_none());
    assert_eq!((page.commands, page.renders), (1, 1));

    page.fail_render = true;
    let out = tabs.periodic(&mut page, 0.0);
    assert!(matches!(out.error, Some(TabsError::Thrown(ref e)) if e.name == "TypeError"));
    assert!(!tabs.frame_loop_alive());
    assert_eq!((page.commands, page.renders), (2, 2));

    page.fail_render = false;
    for _ in 0..3 {
        let out = tabs.periodic(&mut page, 0.0);
        assert!(out.error.is_none() && out.satellite_updates.is_empty());
    }
    assert_eq!((page.commands, page.renders), (2, 2));
}

#[test]
fn a_command_throw_ends_the_frame_loop_before_rendering() {
    let mut page = Page::default();
    let mut tabs = tabs(&mut page);
    page.fail_command = true;
    assert!(tabs.periodic(&mut page, 0.0).error.is_some());
    assert_eq!((page.commands, page.renders), (1, 0));
    page.fail_command = false;
    assert!(tabs.periodic(&mut page, 0.0).error.is_none());
    assert_eq!((page.commands, page.renders), (1, 0));
}
