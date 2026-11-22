// Ported from AdvantageScope (BSD-3-Clause, Copyright 2021-2026 Littleton Robotics). See
// THIRD_PARTY_NOTICES.md.

//! `Field3dRenderer.ts` and `Field3dRendererImpl.ts`: the scene, cameras, coordinate frames and
//! object managers behind a 3D field tab. Everything the DOM and `window` give upstream comes in
//! through [`Field3dEnv`]; the elements it writes are [`Field3dDom`]; model and texture loads
//! leave through a queue the owner runs and answers.

mod command;
mod implementation;

use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::assets::{AdvantageScopeAssets, Config3dField};
use asrs_model::preferences::{Field3dBatteryMode, Field3dMode, Preferences};

pub use command::{AnyObj, CameraOverride, Field3dRendererCommand, ObjType, annotated_pose};
pub use implementation::Field3dRendererImpl;

use crate::glb::{LoadOptions, LoadedField, OptimizedMesh};
use crate::managers::RobotLoadRequest;
use crate::render::Texture;
use crate::three::Color;

/// What one `render` call reads from `window` and the DOM.
#[derive(Clone, Debug)]
pub struct Field3dEnv {
    /// `window.assets`.
    pub assets: Option<Arc<AdvantageScopeAssets>>,
    /// `window.preferences`.
    pub preferences: Option<Arc<Preferences>>,
    /// `window.isBattery`.
    pub is_battery: bool,
    /// The canvas `clientWidth` and `clientHeight`.
    pub client_width: f64,
    pub client_height: f64,
    /// `canvas.getBoundingClientRect().width`.
    pub rect_width: f64,
    /// The canvas parent's `clientWidth` and `clientHeight`.
    pub parent_size: Option<[f64; 2]>,
    pub device_pixel_ratio: f64,
    /// `(prefers-color-scheme: dark)`.
    pub is_dark: bool,
    /// `(prefers-reduced-motion: reduce)`.
    pub reduced_motion: bool,
    /// `performance.now()`, which drives `THREE.Clock` and the spinner timeout.
    pub performance_now: f64,
    /// `new Date().getTime()`, which drives the low-power frame cap.
    pub date_now: f64,
}

/// The elements the renderer writes: the canvas's `fixed` class and inline size, its
/// container's `fixed` class, the alert, and the spinner's classes.
#[derive(Clone, Debug, PartialEq)]
pub struct Field3dDom {
    pub canvas_fixed: bool,
    pub container_fixed: bool,
    pub canvas_width: String,
    pub canvas_height: String,
    pub alert_hidden: bool,
    pub alert_html: String,
    pub spinner_visible: bool,
    pub spinner_animating: bool,
    /// Due times (`performance.now()`) of the pending timeouts that remove `animating`.
    pub spinner_timeouts: Vec<f64>,
}

impl Default for Field3dDom {
    fn default() -> Self {
        Field3dDom {
            canvas_fixed: false,
            container_fixed: false,
            canvas_width: String::new(),
            canvas_height: String::new(),
            alert_hidden: true,
            alert_html: String::new(),
            spinner_visible: false,
            spinner_animating: false,
            spinner_timeouts: Vec::new(),
        }
    }
}

impl Field3dDom {
    /// Runs the spinner timeouts due by `now`.
    pub fn run_timers(&mut self, now: f64) {
        let before = self.spinner_timeouts.len();
        self.spinner_timeouts.retain(|&due| due > now);
        if self.spinner_timeouts.len() != before {
            self.spinner_animating = false;
        }
    }
}

/// A `WorkerManager.request` the owner runs (`glb::spawn_load_field`, `glb::spawn_load_robot`)
/// and answers with `field_loaded` or `robot_loaded`. A load that fails is never answered.
#[derive(Clone, Debug)]
pub enum LoadRequest {
    Field {
        ticket: u64,
        config: Config3dField,
        options: LoadOptions,
    },
    Robot {
        ticket: u64,
        request: RobotLoadRequest,
    },
}

/// A `TextureLoader.load(url)`, answered with `texture_loaded`; a URL that does not load is
/// never answered.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureLoad {
    pub ticket: u64,
    pub url: JsString,
}

/// One `renderer.render(scene, camera)`: the drawing buffer, pixel ratio and background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderCall {
    pub width: u32,
    pub height: u32,
    pub pixel_ratio: f64,
    pub background: Color,
}

/// `Field3dRenderer.ts`: rebuilds the implementation (on a fresh canvas) whenever the
/// preferred mode or antialiasing changes, carrying the saved state across.
pub struct Field3dRenderer {
    implementation: Option<Field3dRendererImpl>,
    last_mode: Option<Field3dMode>,
    last_use_aa: bool,
    state_restore_cache: Option<JsValue>,
    dom: Field3dDom,
    mac: bool,
}

impl Field3dRenderer {
    /// `mac` is camera-controls' `/Mac/.test(navigator.platform)`.
    pub fn new(preferences: Option<&Preferences>, is_battery: bool, mac: bool) -> Self {
        let mut renderer = Field3dRenderer {
            implementation: None,
            last_mode: None,
            last_use_aa: true,
            state_restore_cache: None,
            dom: Field3dDom::default(),
            mac,
        };
        renderer.update_implementation(preferences, is_battery);
        renderer
    }

    pub fn implementation(&self) -> Option<&Field3dRendererImpl> {
        self.implementation.as_ref()
    }

    pub fn implementation_mut(&mut self) -> Option<&mut Field3dRendererImpl> {
        self.implementation.as_mut()
    }

    pub fn dom(&self) -> &Field3dDom {
        &self.dom
    }

    pub fn dom_mut(&mut self) -> &mut Field3dDom {
        &mut self.dom
    }

    pub fn save_state(&self) -> JsValue {
        self.implementation
            .as_ref()
            .map_or(JsValue::Null, Field3dRendererImpl::save_state)
    }

    pub fn restore_state(&mut self, state: &JsValue) {
        if state.is_null() {
            return;
        }
        match &mut self.implementation {
            None => self.state_restore_cache = Some(state.clone()),
            Some(implementation) => implementation.restore_state(state),
        }
    }

    pub fn set_3d_camera(&mut self, index: f64) {
        if let Some(implementation) = &mut self.implementation {
            implementation.set_3d_camera(index);
        }
    }

    pub fn set_fov(&mut self, fov: f64) {
        if let Some(implementation) = &mut self.implementation {
            implementation.set_fov(fov);
        }
    }

    pub fn get_aspect_ratio(&self) -> Option<f64> {
        self.implementation
            .as_ref()
            .and_then(Field3dRendererImpl::get_aspect_ratio)
    }

    /// `updateImplementation()`, which `render` runs first.
    pub fn update_implementation(&mut self, preferences: Option<&Preferences>, is_battery: bool) {
        let mode = preferences.map(|p| {
            let battery = match p.field3d_mode_battery {
                Field3dBatteryMode::SameAsAc => None,
                Field3dBatteryMode::Cinematic => Some(Field3dMode::Cinematic),
                Field3dBatteryMode::Standard => Some(Field3dMode::Standard),
                Field3dBatteryMode::LowPower => Some(Field3dMode::LowPower),
            };
            match battery {
                Some(mode) if is_battery => mode,
                _ => p.field3d_mode_ac,
            }
        });
        let use_aa = preferences.is_none_or(|p| p.field3d_antialiasing);
        let Some(mode) = mode else {
            return;
        };
        if Some(mode) == self.last_mode && use_aa == self.last_use_aa {
            return;
        }
        self.last_mode = Some(mode);
        self.last_use_aa = use_aa;
        let state = self
            .implementation
            .as_ref()
            .map_or(JsValue::Null, Field3dRendererImpl::save_state);
        self.dom.canvas_width = String::new();
        self.dom.canvas_height = String::new();
        let mut implementation = Field3dRendererImpl::new(mode, use_aa, self.mac);
        if let Some(cache) = self.state_restore_cache.take() {
            implementation.restore_state(&cache);
        } else if !state.is_null() {
            implementation.restore_state(&state);
        }
        self.implementation = Some(implementation);
    }

    pub fn render(
        &mut self,
        command: &Field3dRendererCommand,
        env: &Field3dEnv,
    ) -> Result<(), asrs_js::string::JsError> {
        self.update_implementation(env.preferences.as_deref(), env.is_battery);
        match &mut self.implementation {
            Some(implementation) => implementation.render(command, env, &mut self.dom),
            None => Ok(()),
        }
    }

    /// The window `keydown` listener; `command_modifier` is Cmd on macOS and Ctrl elsewhere.
    pub fn key_down(
        &mut self,
        code: &str,
        command_modifier: bool,
        editable: bool,
        container_height: f64,
    ) {
        if let Some(implementation) = &mut self.implementation {
            implementation.key_down(code, command_modifier, editable, container_height);
        }
    }

    pub fn key_up(&mut self, code: &str, editable: bool) {
        if let Some(implementation) = &mut self.implementation {
            implementation.key_up(code, editable);
        }
    }

    pub fn mouse_down(&mut self, button: f64, x: f64, y: f64) {
        if let Some(implementation) = &mut self.implementation {
            implementation.mouse_down(button, x, y);
        }
    }

    /// The `ask-3d-camera` message data for a right click.
    pub fn mouse_up(
        &mut self,
        x: f64,
        y: f64,
        assets: Option<&AdvantageScopeAssets>,
    ) -> Option<JsValue> {
        self.implementation
            .as_mut()
            .and_then(|implementation| implementation.mouse_up(x, y, assets))
    }

    pub fn take_load_requests(&mut self) -> Vec<LoadRequest> {
        self.implementation
            .as_mut()
            .map_or(Vec::new(), Field3dRendererImpl::take_load_requests)
    }

    pub fn take_texture_requests(&mut self) -> Vec<TextureLoad> {
        self.implementation
            .as_mut()
            .map_or(Vec::new(), Field3dRendererImpl::take_texture_requests)
    }

    pub fn field_loaded(&mut self, ticket: u64, result: &LoadedField) {
        if let Some(implementation) = &mut self.implementation {
            implementation.field_loaded(ticket, result);
        }
    }

    pub fn robot_loaded(&mut self, ticket: u64, result: &[Vec<OptimizedMesh>]) {
        if let Some(implementation) = &mut self.implementation {
            implementation.robot_loaded(ticket, result);
        }
    }

    pub fn texture_loaded(&mut self, ticket: u64, texture: Texture) {
        if let Some(implementation) = &mut self.implementation {
            implementation.texture_loaded(ticket, texture);
        }
    }
}
