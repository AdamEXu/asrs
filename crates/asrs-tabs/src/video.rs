//! `src/hub/controllers/VideoController.ts`: the Video tab's controller. Video
//! processing itself lives in the main process (`select-video` goes out, `video-data`
//! comes back); this is the controller side of that boundary plus the frame, lock and
//! playback math.

use std::rc::Rc;

use asrs_js::math;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::SetClock;
use asrs_js::value::JsValue;
use asrs_log::log::robot_state::{get_enabled_data, get_match_info};
use asrs_log::match_info::{MatchInfo, MatchType};
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::tab_controller::{Command, TabController};
use asrs_model::video::VideoSource;

use crate::cx::TabCx;
use crate::form::RangeInput;
use crate::form::css::{Property, Style};

/// What the controller reads besides [`TabCx`].
pub trait VideoCx: TabCx {
    /// `new Date().getTime()`.
    fn now(&self) -> f64;
}

/// The `hidden` and `disabled` flags `updateButtons` writes to the controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buttons {
    pub lock_disabled: bool,
    pub unlock_disabled: bool,
    pub lock_hidden: bool,
    pub unlock_hidden: bool,
    pub play_hidden: bool,
    pub pause_hidden: bool,
    pub controls_disabled: bool,
}

pub struct VideoController {
    buttons: Buttons,
    slider: RangeInput,
    progress: Style,
    youtube_animating: bool,
    tba_animating: bool,
    img_folder: JsValue,
    last_img_folder: JsValue,
    fps: JsValue,
    total_frames: JsValue,
    completed_frames: JsValue,
    auto_end_frame: JsValue,
    locked: bool,
    locked_start_log: f64,
    playing: bool,
    play_start_frame: f64,
    play_start_real: f64,
}

impl VideoController {
    /// `<input class="video-timeline-slider" type="range" value="0" min="1" max="1"
    /// step="1">` and the starting field values.
    pub fn new() -> VideoController {
        let mut controller = VideoController {
            buttons: Buttons::default(),
            slider: RangeInput::new("0", "1", "1", "1"),
            progress: Style::new(),
            youtube_animating: false,
            tba_animating: false,
            img_folder: JsValue::Null,
            last_img_folder: JsValue::Null,
            fps: JsValue::Null,
            total_frames: JsValue::Null,
            completed_frames: JsValue::Null,
            auto_end_frame: JsValue::Number(-1.0),
            locked: false,
            locked_start_log: 0.0,
            playing: false,
            play_start_frame: 0.0,
            play_start_real: 0.0,
        };
        controller.update_buttons();
        controller
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// `hasData()`.
    pub fn has_data(&self) -> bool {
        !self.img_folder.is_null()
            && !self.fps.is_null()
            && !self.total_frames.is_null()
            && !self.completed_frames.is_null()
    }

    /// The flags `updateButtons` last wrote. They go stale where upstream skips it: a
    /// `video-data` without `totalFrames` or `completedFrames` drops the data but leaves
    /// the controls enabled.
    pub fn buttons(&self) -> Buttons {
        self.buttons
    }

    fn update_buttons(&mut self) {
        let has_data = self.has_data();
        self.buttons = Buttons {
            lock_disabled: !has_data,
            unlock_disabled: !has_data,
            lock_hidden: self.locked,
            unlock_hidden: !self.locked,
            play_hidden: self.playing,
            pause_hidden: !self.playing,
            controls_disabled: self.locked || !has_data,
        };
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn locked_start_log(&self) -> f64 {
        self.locked_start_log
    }

    pub fn play_start(&self) -> (f64, f64) {
        (self.play_start_frame, self.play_start_real)
    }

    /// `imgFolder`, `lastImgFolder`, `fps`, `totalFrames`, `completedFrames` and
    /// `autoEndFrame` as `video-data` left them.
    pub fn data(&self) -> [&JsValue; 6] {
        [
            &self.img_folder,
            &self.last_img_folder,
            &self.fps,
            &self.total_frames,
            &self.completed_frames,
            &self.auto_end_frame,
        ]
    }

    pub fn slider(&self) -> &RangeInput {
        &self.slider
    }

    /// `VIDEO_TIMELINE_PROGRESS.style.width`.
    pub fn progress_width(&self) -> JsString {
        self.progress.get(Property::Width)
    }

    pub fn youtube_animating(&self) -> bool {
        self.youtube_animating
    }

    pub fn tba_animating(&self) -> bool {
        self.tba_animating
    }

    /// `LOCAL_SOURCE` click.
    pub fn select_local(&mut self, cx: &mut (impl VideoCx + ?Sized), uuid: &JsValue) {
        self.youtube_animating = false;
        self.tba_animating = false;
        send_select_video(cx, uuid, VideoSource::Local, JsValue::Null, JsValue::Null);
    }

    /// `YOUTUBE_SOURCE` click.
    pub fn select_youtube(&mut self, cx: &mut (impl VideoCx + ?Sized), uuid: &JsValue) {
        self.youtube_animating = true;
        self.tba_animating = false;
        send_select_video(cx, uuid, VideoSource::YouTube, JsValue::Null, JsValue::Null);
    }

    /// `TBA_SOURCE` click: the API key, match info and practice-match checks send `error`
    /// and return early. `menu` is `[rect.right, rect.top]` off the button.
    pub fn select_tba(&mut self, cx: &mut (impl VideoCx + ?Sized), uuid: &JsValue, menu: [f64; 2]) {
        let has_key = cx
            .tab_preferences()
            .map(|p| !p.tba_api_key.is_empty())
            .unwrap_or(false);
        if !has_key {
            send_error(
                cx,
                "No API key",
                "Please enter an API key for The Blue Alliance in the AdvantageScope \
                 preferences. An API key can be obtained from the Account page on The \
                 Blue Alliance website.",
            );
            return;
        }
        let clock = SetClock(cx.now());
        let info = get_match_info(&cx.tab_log(), &clock);
        let Some(info) = info else {
            send_error(
                cx,
                "No match info",
                "Failed to read event and match info from the log. Please load the \
                 video using a YouTube URL or local file instead.",
            );
            return;
        };
        if info.match_type == MatchType::Practice {
            send_error(
                cx,
                "No videos for practice match",
                "This is a practice match. No data is available on The Blue Alliance \
                 for practice matches, please load the video using a YouTube URL or \
                 local file instead.",
            );
            return;
        }
        self.youtube_animating = false;
        self.tba_animating = true;
        send_select_video(
            cx,
            uuid,
            VideoSource::TheBlueAlliance,
            match_info_to_js(&info),
            JsValue::Array(menu.map(JsValue::Number).to_vec()),
        );
    }

    /// `toggleLock`: no-op without data.
    pub fn toggle_lock(&mut self, cx: &mut (impl VideoCx + ?Sized)) {
        if !self.has_data() {
            return;
        }
        self.locked = !self.locked;
        if self.locked {
            self.playing = false;
            let selected = cx.tab_selection().get_selected_time().unwrap_or(0.0);
            self.locked_start_log = selected - (self.slider.number() - 1.0) / num(&self.fps);
        }
        self.update_buttons();
    }

    /// `togglePlayPause`: no-op while locked or without data.
    pub fn toggle_play_pause(&mut self, cx: &mut (impl VideoCx + ?Sized)) {
        if self.locked || !self.has_data() {
            return;
        }
        self.playing = !self.playing;
        if self.playing {
            self.play_start_frame = self.slider.number();
            self.play_start_real = cx.now() / 1000.0;
        }
        self.update_buttons();
    }

    /// The slider's `input` listener: re-anchors playback at the dragged frame.
    pub fn slider_input(&mut self, cx: &mut (impl VideoCx + ?Sized)) {
        if self.playing {
            self.play_start_frame = self.slider.number();
            self.play_start_real = cx.now() / 1000.0;
        }
    }

    /// `input.value = v`, sanitized.
    pub fn set_slider(&mut self, value: JsString) {
        self.slider.set_value(value);
    }

    /// `changeFrame(delta)`: no-op while locked, playing or without data.
    pub fn change_frame(&mut self, delta: f64) {
        if self.locked || !self.has_data() || self.playing {
            return;
        }
        self.slider.set_value(JsString::from(number::to_string(
            self.slider.number() + delta,
        )));
    }

    /// `skipTime(delta)`: `delta` seconds of frames; re-anchors playback like `input`.
    pub fn skip_time(&mut self, cx: &mut (impl VideoCx + ?Sized), delta: f64) {
        if self.locked || !self.has_data() {
            return;
        }
        if ops::to_boolean(&self.fps) {
            let moved = self.slider.number() + delta * num(&self.fps);
            self.slider
                .set_value(JsString::from(number::to_string(moved)));
            if self.playing {
                self.play_start_frame = self.slider.number();
                self.play_start_real = cx.now() / 1000.0;
            }
        }
    }

    /// The `keydown` listener's switch; the caller has already applied the target,
    /// visibility and modifier guards.
    pub fn key_down(&mut self, cx: &mut (impl VideoCx + ?Sized), code: &str) {
        match code {
            "ArrowUp" | "ArrowDown" => self.toggle_lock(cx),
            "Slash" => self.toggle_play_pause(cx),
            "Comma" => self.skip_time(cx, -5.0),
            "Period" => self.skip_time(cx, 5.0),
            "ArrowLeft" => self.change_frame(-1.0),
            "ArrowRight" => self.change_frame(1.0),
            _ => {}
        }
    }

    /// `processVideoData(data)`, dispatched by `Tabs` to every Video tab.
    pub fn process_video_data(
        &mut self,
        cx: &mut (impl VideoCx + ?Sized),
        data: &JsValue,
        uuid: &JsValue,
    ) -> Result<(), JsError> {
        if !ops::strict_equals(&ops::get(data, "uuid")?, uuid) {
            return Ok(());
        }
        if ops::has(data, "error")? {
            self.youtube_animating = false;
            self.tba_animating = false;
        } else if ops::has(data, "fps")? {
            self.img_folder = ops::get(data, "imgFolder")?;
            self.fps = ops::get(data, "fps")?;
            self.total_frames = ops::get(data, "totalFrames")?;
            self.completed_frames = ops::get(data, "completedFrames")?;
            if self.total_frames.is_null() || self.completed_frames.is_null() {
                return Ok(());
            }
            let percent = num(&self.completed_frames) / num(&self.total_frames) * 100.0;
            self.progress.set(
                Property::Width,
                &JsString::from(format!("{}%", number::to_string(percent))),
            );
            self.slider.set_max(member_to_string(&self.total_frames)?);
            if !ops::strict_equals(&self.img_folder, &self.last_img_folder) {
                self.last_img_folder = self.img_folder.clone();
                self.youtube_animating = false;
                self.tba_animating = false;
            }
            let auto_end_frame = ops::get(data, "autoEndFrame")?;
            if num(&self.auto_end_frame) <= 0.0
                && num(&auto_end_frame) > 0.0
                && !self.locked
                && let Some(auto_end_time) = first_disable(cx)
            {
                self.playing = false;
                self.locked = true;
                self.locked_start_log =
                    auto_end_time - (num(&auto_end_frame) - 1.0) / num(&self.fps);
            }
            self.auto_end_frame = auto_end_frame;
        } else {
            self.locked = false;
            self.playing = false;
        }
        self.update_buttons();
        Ok(())
    }
}

impl Default for VideoController {
    fn default() -> Self {
        Self::new()
    }
}

impl<Cx: VideoCx + ?Sized> TabController<Cx> for VideoController {
    /// `saveState()`: always `null`.
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
        true
    }

    /// `getCommand()`: the frame's image path, or `""`.
    fn command(&mut self, cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        let mut path = JsString::new();
        if self.has_data() {
            let render_time = cx.tab_selection().get_render_time();
            if self.locked && render_time.is_some() {
                let frame = math::floor(
                    (render_time.unwrap_or_default() - self.locked_start_log) * num(&self.fps),
                ) + 1.0;
                self.slider
                    .set_value(JsString::from(number::to_string(frame)));
            }
            if self.playing {
                let frame = (cx.now() / 1000.0 - self.play_start_real) * num(&self.fps)
                    + self.play_start_frame;
                self.slider
                    .set_value(JsString::from(number::to_string(frame)));
            }
            let frame = self.slider.number();
            if frame >= 1.0 && frame <= num(&self.completed_frames) {
                let mut filename = number::to_string(frame);
                while filename.len() < 8 {
                    filename = format!("0{filename}");
                }
                path = ops::to_string(&self.img_folder);
                path.push_str(&JsString::from(filename));
                path.push_str(&JsString::from(".jpg"));
            }
        }
        Ok(Rc::new(path))
    }
}

/// `window.sendMainMessage("select-video", {...})`.
fn send_select_video(
    cx: &mut (impl VideoCx + ?Sized),
    uuid: &JsValue,
    source: VideoSource,
    match_info: JsValue,
    menu_coordinates: JsValue,
) {
    cx.send_main_message(NamedMessage::new(
        HubToMain::SelectVideo,
        ops::object([
            ("uuid", uuid.clone()),
            ("source", JsValue::Number(source.number())),
            ("matchInfo", match_info),
            ("menuCoordinates", menu_coordinates),
        ]),
    ));
}

/// `window.sendMainMessage("error", {title, content})`.
fn send_error(cx: &mut (impl VideoCx + ?Sized), title: &str, content: &str) {
    cx.send_main_message(NamedMessage::new(
        HubToMain::Error,
        ops::object([
            ("title", JsValue::String(JsString::from(title))),
            ("content", JsValue::String(JsString::from(content))),
        ]),
    ));
}

/// The `{year, event, matchType, matchNumber}` literal `getMatchInfo` returns, sent on as
/// `matchInfo`.
fn match_info_to_js(info: &MatchInfo) -> JsValue {
    ops::object([
        ("year", JsValue::Number(info.year)),
        ("event", JsValue::String(info.event.clone())),
        ("matchType", JsValue::Number(info.match_type.as_number())),
        ("matchNumber", JsValue::Number(info.match_number)),
    ])
}

/// The enabled-to-disabled transition `processVideoData` locks to: the timestamp of the
/// first disabled sample after an enabled one.
fn first_disable<Cx: VideoCx + ?Sized>(cx: &Cx) -> Option<f64> {
    let data = get_enabled_data(&cx.tab_log())?;
    let mut was_enabled = false;
    for (i, &timestamp) in data.timestamps.iter().enumerate() {
        if was_enabled && !data.values[i] {
            return Some(timestamp);
        }
        was_enabled = data.values[i];
    }
    None
}

/// `value.toString()`: a member access, so null and undefined throw instead of
/// stringifying.
fn member_to_string(value: &JsValue) -> Result<JsString, JsError> {
    match value {
        JsValue::Undefined => Err(JsError::type_error(
            "Cannot read properties of undefined (reading 'toString')",
        )),
        JsValue::Null => Err(JsError::type_error(
            "Cannot read properties of null (reading 'toString')",
        )),
        other => Ok(ops::to_string(other)),
    }
}

/// `ToNumber(value)` where JS only compares or does arithmetic.
fn num(value: &JsValue) -> f64 {
    ops::value_to_number(value).unwrap_or(f64::NAN)
}
