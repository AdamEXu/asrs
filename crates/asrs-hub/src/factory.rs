//! The switch in `Tabs.addTab`: the controller and renderer upstream builds for each tab type,
//! in its `createUUID()` order. The controller's `UUID` field is drawn first, then whatever its
//! constructor draws (one per `SourceList` it owns), then the renderer's (only `TableRenderer`
//! draws one).

use std::cell::RefCell;
use std::rc::Rc;

use asrs_canvas::text::TextBackend;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{Clock, create_uuid};
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{
    NoopController, NoopRenderer, TabCommand, TabController, TabRenderer,
};
use asrs_model::tab_type::TabType;
use asrs_tabs::console::controller::{ConsoleController, ConsoleDom, ConsoleRenderer};
use asrs_tabs::documentation::DocumentationRenderer;
use asrs_tabs::field_2d::{Field2dController, Field2dDom, Field2dRenderer};
use asrs_tabs::field3d::Field3dController;
use asrs_tabs::joysticks::JoysticksController;
use asrs_tabs::joysticks_renderer::{JoysticksDom, JoysticksRenderer};
use asrs_tabs::line_graph::{LineGraphController, LineGraphDom, LineGraphRenderer};
use asrs_tabs::list_cx::SourceListCx;
use asrs_tabs::mechanism::MechanismController;
use asrs_tabs::mechanism_renderer::MechanismRenderer;
use asrs_tabs::metadata::{MetadataController, MetadataRenderer};
use asrs_tabs::points::PointsController;
use asrs_tabs::points_renderer::PointsRenderer;
use asrs_tabs::statistics::{StatisticsController, StatisticsCx};
use asrs_tabs::swerve::SwerveController;
use asrs_tabs::swerve_renderer::{SwerveDom, SwerveRenderer};
use asrs_tabs::table::{TableController, TableDom, TableRenderer};
use asrs_tabs::video::{VideoController, VideoCx};

use crate::tabs::{TabFactory, TabParts, TabsHost};

/// Everything a hub tab reads from the page.
pub trait HubTabCx: SourceListCx + StatisticsCx + VideoCx + TabsHost {}

impl<T: SourceListCx + StatisticsCx + VideoCx + TabsHost + ?Sized> HubTabCx for T {}

/// The page's `Date` and the canvas text stack a renderer measures its labels with.
#[derive(Clone)]
pub struct CanvasEnv {
    pub clock: Rc<dyn Clock>,
    pub text: Rc<RefCell<dyn TextBackend>>,
}

/// The renderer of a tab type whose renderer unit has not landed. Upstream's `saveState()`
/// returns `null` for every one of them (`Field3dRenderer` until its implementation exists),
/// so saved layouts are unaffected; it draws nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingRenderer {
    pub kind: TabType,
}

impl<Cx: ?Sized> TabRenderer<Cx> for PendingRenderer {
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

/// Sees every renderer the factory builds and may wrap or replace it.
pub trait RendererHook<Cx: ?Sized> {
    fn renderer(
        &mut self,
        cx: &mut Cx,
        kind: Option<TabType>,
        renderer: Box<dyn TabRenderer<Cx>>,
    ) -> Box<dyn TabRenderer<Cx>>;
}

pub struct HubTabFactory<Cx: ?Sized> {
    hook: Option<Box<dyn RendererHook<Cx>>>,
}

impl<Cx: ?Sized> Default for HubTabFactory<Cx> {
    fn default() -> Self {
        HubTabFactory { hook: None }
    }
}

impl<Cx: ?Sized> HubTabFactory<Cx> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_hook(hook: Box<dyn RendererHook<Cx>>) -> Self {
        HubTabFactory { hook: Some(hook) }
    }
}

type Halves<Cx> = (Box<dyn TabController<Cx>>, Box<dyn TabRenderer<Cx>>);

fn pending<Cx: ?Sized>(kind: TabType) -> Box<dyn TabRenderer<Cx>> {
    Box::new(PendingRenderer { kind })
}

impl<Cx: HubTabCx + 'static> HubTabFactory<Cx> {
    fn halves(
        cx: &mut Cx,
        uuid: &JsString,
        tab_type: Option<TabType>,
    ) -> Result<Halves<Cx>, JsError> {
        Ok(match tab_type {
            Some(TabType::Documentation) => (
                Box::new(NoopController),
                Box::new(DocumentationRenderer::new()),
            ),
            Some(TabType::LineGraph) => {
                let controller = LineGraphController::new(cx.tab_rng());
                let renderer: Box<dyn TabRenderer<Cx>> = match cx.canvas_env() {
                    Some(env) => Box::new(LineGraphRenderer::new(
                        true,
                        env.clock,
                        Rc::new(RefCell::new(LineGraphDom::default())),
                        env.text,
                    )),
                    None => pending(TabType::LineGraph),
                };
                (Box::new(controller), renderer)
            }
            Some(TabType::Field2d) => (
                Box::new(Field2dController::new(cx)),
                Box::new(Field2dRenderer::new(Rc::new(RefCell::new(
                    Field2dDom::default(),
                )))),
            ),
            Some(TabType::Field3d) => {
                let controller = Field3dController::new(cx);
                let renderer = cx
                    .view_renderer(TabType::Field3d)
                    .unwrap_or_else(|| pending(TabType::Field3d));
                (Box::new(controller), renderer)
            }
            Some(TabType::Table) => {
                let dom = Rc::new(RefCell::new(TableDom::default()));
                let controller = TableController::new(uuid, dom.clone());
                let renderer_uuid = JsString::from(create_uuid(cx.tab_rng()));
                (
                    Box::new(controller),
                    Box::new(TableRenderer::new(renderer_uuid, dom, true)),
                )
            }
            Some(TabType::Console) => {
                let dom = Rc::new(RefCell::new(ConsoleDom::default()));
                (
                    Box::new(ConsoleController::new(dom.clone())),
                    Box::new(ConsoleRenderer::new(dom, true)),
                )
            }
            Some(TabType::Statistics) => (
                Box::new(StatisticsController::new(cx)),
                pending(TabType::Statistics),
            ),
            Some(TabType::Video) => (Box::new(VideoController::new()), pending(TabType::Video)),
            Some(TabType::Joysticks) => {
                let controller = JoysticksController::new(cx);
                let renderer = cx.view_renderer(TabType::Joysticks).unwrap_or_else(|| {
                    Box::new(JoysticksRenderer::new(Rc::new(RefCell::new(
                        JoysticksDom::default(),
                    ))))
                });
                (Box::new(controller), renderer)
            }
            Some(TabType::Swerve) => (
                Box::new(SwerveController::new(cx)),
                Box::new(SwerveRenderer::new(Rc::new(RefCell::new(
                    SwerveDom::default(),
                )))),
            ),
            Some(TabType::Mechanism) => (
                Box::new(MechanismController::new(cx)),
                Box::new(MechanismRenderer::new()),
            ),
            Some(TabType::Points) => (
                Box::new(PointsController::new(cx)),
                Box::new(PointsRenderer::new()),
            ),
            Some(TabType::Metadata) => (
                Box::new(MetadataController::new(cx)?),
                Box::new(MetadataRenderer::new()),
            ),
            None => (Box::new(NoopController), Box::new(NoopRenderer)),
        })
    }
}

impl<Cx: HubTabCx + 'static> TabFactory<Cx> for HubTabFactory<Cx> {
    fn create(&mut self, cx: &mut Cx, tab_type: Option<TabType>) -> Result<TabParts<Cx>, JsError> {
        let uuid = JsString::from(create_uuid(cx.tab_rng()));
        let (controller, renderer) = Self::halves(cx, &uuid, tab_type)?;
        let renderer = match &mut self.hook {
            Some(hook) => hook.renderer(cx, tab_type, renderer),
            None => renderer,
        };
        Ok(TabParts {
            uuid,
            controller,
            renderer,
        })
    }
}
