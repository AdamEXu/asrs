#![allow(clippy::disallowed_methods)]

//! `select-video` in the app, headless: `main_process::start` in a fresh home directory, a
//! restored Video tab, and a stand-in for ffmpeg on the `PATH` that writes frames and reports
//! progress as ffmpeg does. What the processor decides is `video_vectors`' business; this checks
//! that its children, dialogs and files are real and that what it sends reaches the tab.

use std::path::PathBuf;
use std::process::Command as Process;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_app::main_process::{self, MainProcess};
use asrs_app::windows::{self, WindowId, Windows};
use asrs_hub::hub::{HubModel, HubPage};
use asrs_js::ops;
use asrs_js::value::JsValue;
use asrs_model::messages::MainToHub;
use asrs_tabs::video::VideoController;
use asrs_tabs::video_renderer::VideoRenderer;
use asrs_ui::gpui::{HeadlessAppContext, NoopTextSystem};
use asrs_ui::platform::dialog::{Reply, Script, Shown};

const HOME: &str = "ASRS_VIDEO_FLOW_HOME";
const TIMEOUT: Duration = Duration::from_secs(60);
const UUID: &str = "video000000000000000000000000000";
const TAB: usize = 1;

fn in_fresh_home(test: &str) {
    let home = std::env::temp_dir().join(format!("asrs-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("home");
    let mut child = Process::new(std::env::current_exe().expect("the test binary"));
    child.args(["--exact", test, "--ignored", "--nocapture"]);
    for variable in [
        "HOME",
        "APPDATA",
        "XDG_CONFIG_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        HOME,
    ] {
        child.env(variable, &home);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(home.join("bin")).chain(std::env::split_paths(&path));
    child.env("PATH", std::env::join_paths(dirs).expect("PATH"));
    let status = child.status().expect("the test binary");
    let _ = std::fs::remove_dir_all(&home);
    assert!(status.success(), "{test}: {status}");
}

#[cfg(unix)]
#[test]
fn a_local_video_plays_in_the_tab() {
    in_fresh_home("local_in_a_fresh_home");
}

#[cfg(unix)]
#[test]
fn a_failed_conversion_shows_its_error() {
    in_fresh_home("failure_in_a_fresh_home");
}

#[cfg(unix)]
#[test]
fn an_installed_ffmpeg_converts_a_clip() {
    in_fresh_home("ffmpeg_in_a_fresh_home");
}

#[test]
fn youtube_without_a_url_says_so() {
    in_fresh_home("youtube_in_a_fresh_home");
}

struct AppUnderTest {
    cx: HeadlessAppContext,
    home: PathBuf,
    hub: WindowId,
}

impl AppUnderTest {
    /// The app with one hub on a Video tab; `None` unless run by [`in_fresh_home`].
    fn start(replies: &[Reply]) -> Option<AppUnderTest> {
        let home = PathBuf::from(std::env::var_os(HOME)?);
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let (_, received) = async_channel::unbounded();
        cx.update(|cx| {
            asrs_ui::init(cx);
            windows::init(cx);
            cx.set_global(Script {
                replies: replies.to_vec().into(),
                ..Script::default()
            });
            main_process::start(cx, &[], received);
        });
        cx.run_until_parked();
        let hub = cx.update(|cx| Windows::manager(cx).hubs()[0]);
        let tab = |kind: f64, uuid: &str, height: f64| {
            ops::object([
                ("type", JsValue::Number(kind)),
                ("title", JsValue::string("")),
                ("controller", JsValue::Null),
                ("controllerUUID", JsValue::string(uuid)),
                ("renderer", JsValue::Null),
                ("controlsHeight", JsValue::Number(height)),
            ])
        };
        let state = ops::object([
            (
                "sidebar",
                ops::object([
                    ("width", JsValue::Number(300.0)),
                    ("expanded", JsValue::Array(Vec::new())),
                ]),
            ),
            (
                "tabs",
                ops::object([
                    ("selected", JsValue::Number(TAB as f64)),
                    (
                        "tabs",
                        JsValue::Array(vec![tab(0.0, "documentation", 0.0), tab(7.0, UUID, 85.0)]),
                    ),
                ]),
            ),
        ]);
        cx.update(|cx| MainProcess::send_to_hub(cx, hub, MainToHub::RestoreState, state));
        cx.run_until_parked();
        Some(AppUnderTest { cx, home, hub })
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&mut AppUnderTest) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < TIMEOUT, "waiting for {what}");
            self.frame();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn frame(&mut self) {
        self.cx.advance_clock(Duration::from_millis(10));
        let window = self.cx.update(|cx| Windows::handle(cx, self.hub));
        self.cx
            .update_window(window.expect("open"), |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .expect("open");
        self.cx.run_until_parked();
    }

    fn model<R>(&mut self, f: impl FnOnce(&mut HubModel) -> R) -> R {
        self.cx.update(|cx| {
            let hubs = MainProcess::hub_windows(cx);
            let hub = hubs.iter().find(|(id, _)| *id == self.hub).expect("a hub");
            let hub = hub.1.upgrade().expect("a live hub");
            hub.update(cx, |hub, cx| hub.update(cx, |model, _| f(model)))
        })
    }

    fn video<R>(&mut self, f: impl FnOnce(&mut VideoController, &mut HubPage, &JsValue) -> R) -> R {
        self.model(|hub| {
            let (tabs, page) = hub.tabs_mut();
            let tab = tabs.tab_mut(TAB).expect("the Video tab");
            let uuid = tab.uuid.clone();
            let controller = tab.controller.downcast_mut::<VideoController>();
            f(controller.expect("a Video controller"), page, &uuid)
        })
    }

    fn natural_size(&mut self) -> [f64; 2] {
        self.model(|hub| {
            let tab = hub.tabs().tab(TAB).expect("the Video tab");
            let renderer = tab.renderer().downcast_ref::<VideoRenderer>();
            renderer
                .expect("a Video renderer")
                .image()
                .borrow()
                .natural_size()
        })
    }

    fn shown(&mut self) -> Vec<Shown> {
        self.cx.update(|cx| {
            let shown = &cx.global::<Script>().shown;
            shown.iter().map(|(_, shown)| shown.clone()).collect()
        })
    }

    fn cache(&self) -> PathBuf {
        self.home.join("advantagescope-video-cache")
    }
}

#[cfg(unix)]
fn install_ffmpeg(script: &str) -> Option<()> {
    use std::os::unix::fs::PermissionsExt;
    let bin = PathBuf::from(std::env::var_os(HOME)?).join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let ffmpeg = bin.join("ffmpeg");
    std::fs::write(&ffmpeg, script).expect("ffmpeg");
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o755)).expect("ffmpeg");
    Some(())
}

#[cfg(unix)]
#[test]
#[ignore = "run by a_local_video_plays_in_the_tab"]
fn local_in_a_fresh_home() {
    let frame = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/assets/synthetic-frame.jpg"
    );
    let script = format!(
        r#"#!/bin/sh
[ "$1" = -version ] && exit 0
for last; do :; done
dir="${{last%/*}}"
printf '%s\n' "$@" > "$dir/argv"
printf '  Duration: 00:00:00.20, start: 0.000000, bitrate: 1 kb/s\n  Stream #0:0: Video: h264, yuv420p, 160x90, 30 fps, 30 tbr\n' >&2
for i in 1 2 3 4; do cp '{frame}' "$dir/0000000$i.jpg"; done
sleep 0.3
printf 'frame=    2 fps=0.0 q=2.0 size=N/A\r' >&2
sleep 0.3
printf 'frame=    4 fps=0.0 q=2.0 size=N/A\r' >&2
"#
    );
    if install_ffmpeg(&script).is_none() {
        return;
    }
    let clip = PathBuf::from("/clips/synthetic clip.mp4");
    let Some(mut app) = AppUnderTest::start(&[Reply::Open(vec![clip])]) else {
        return;
    };
    app.video(|video, page, uuid| video.select_local(page, uuid));
    app.until("progress", |app| {
        app.video(|video, _, _| video.data()[4] == &JsValue::Number(2.0))
    });
    app.video(|video, _, _| {
        assert_eq!(video.data()[2], &JsValue::Number(30.0));
        assert_eq!(video.data()[3], &JsValue::Number(6.0), "totalFrames");
    });
    app.until("the conversion to finish", |app| {
        app.video(|video, _, _| video.data()[3] == &JsValue::Number(4.0))
    });
    app.until("the first frame", |app| app.natural_size() == [160.0, 90.0]);
    assert_eq!(
        app.video(|video, _, _| video.progress_width().to_string_lossy()),
        "100%"
    );
    let folder = app.video(|video, _, _| video.data()[0].clone());
    let folder = folder.as_str().expect("imgFolder").to_string_lossy();
    assert!(
        folder.starts_with(&*app.cache().to_string_lossy()),
        "{folder}"
    );
    let argv = std::fs::read_to_string(format!("{folder}argv")).expect("argv");
    assert_eq!(
        argv.lines().collect::<Vec<_>>(),
        [
            "-i",
            "/clips/synthetic clip.mp4",
            "-vf",
            "scale=1920:-2,setsar=1:1",
            "-q:v",
            "2",
            format!("{folder}%08d.jpg").as_str(),
        ]
    );
    app.video(|video, _, _| video.change_frame(3.0));
    app.until("the last frame", |app| {
        app.video(|video, _, _| video.slider().number()) == 4.0
            && app.natural_size() == [160.0, 90.0]
    });
    let shown = app.shown();
    assert!(
        matches!(&shown[..], [Shown::Open(options)]
            if options.title == "Select a video to open" && options.open_file),
        "{shown:?}"
    );
    let cache = app.cache();
    assert!(cache.is_dir());
    drop(app);
    assert!(!cache.exists(), "quitting deletes the frames");
}

#[cfg(unix)]
fn running(folder: &str) -> bool {
    let ps = Process::new("ps").args(["-axww", "-o", "args="]).output();
    let ps = ps.expect("ps").stdout;
    String::from_utf8_lossy(&ps).contains(folder)
}

#[cfg(unix)]
#[test]
#[ignore = "run by an_installed_ffmpeg_converts_a_clip"]
fn ffmpeg_in_a_fresh_home() {
    let Some(home) = std::env::var_os(HOME).map(PathBuf::from) else {
        return;
    };
    let clip = |name: &str, seconds: u32| {
        let file = home.join(name);
        let source = format!("testsrc2=size=320x180:rate=25:duration={seconds}");
        let status = Process::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", &source])
            .args(["-c:v", "mpeg4", "-q:v", "5", "-f", "mp4"])
            .arg(format!("file:{}", file.display()))
            .status();
        status.ok().map(|status| {
            assert!(status.success(), "{name}: {status}");
            file
        })
    };
    let Some(short) = clip("-y 100% 'two' \u{fc}.mp4", 2) else {
        println!("skipped: no ffmpeg on the PATH");
        return;
    };
    let long = clip("long.mp4", 120).expect("ffmpeg");
    let replies = [long.clone(), short, long].map(|file| Reply::Open(vec![file]));
    let Some(mut app) = AppUnderTest::start(&replies) else {
        return;
    };
    let folder = |app: &mut AppUnderTest| {
        let folder = app.video(|video, _, _| video.data()[0].clone());
        folder.as_str().map(|folder| folder.to_string_lossy())
    };
    let start = |app: &mut AppUnderTest| {
        let before = folder(app);
        app.video(|video, page, uuid| video.select_local(page, uuid));
        app.until("progress", |app| {
            let now = folder(app);
            now.is_some() && now != before
        });
        folder(app).expect("imgFolder")
    };
    let first = start(&mut app);
    assert!(running(&first));

    app.video(|video, page, uuid| video.select_local(page, uuid));
    app.until("the short clip", |app| {
        app.video(|video, _, _| {
            video.data()[3] == &JsValue::Number(50.0) && video.data()[4] == &JsValue::Number(50.0)
        })
    });
    assert!(!running(&first), "the replaced conversion still runs");
    assert_eq!(
        app.video(|video, _, _| video.data()[2].clone()),
        JsValue::Number(25.0)
    );
    for frame in 1..=50 {
        app.until("the frame", |app| app.natural_size() == [1920.0, 1080.0]);
        assert_eq!(
            app.video(|video, _, _| video.slider().number()),
            f64::from(frame)
        );
        app.video(|video, _, _| video.change_frame(1.0));
        app.frame();
    }

    let last = start(&mut app);
    assert!(running(&last));
    let cache = app.cache();
    drop(app);
    let start = Instant::now();
    while running(&last) {
        assert!(start.elapsed() < TIMEOUT, "ffmpeg outlives the app");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!PathBuf::from(&last).exists(), "{last}");
    assert!(!cache.exists(), "quitting deletes the frames");
}

#[cfg(unix)]
#[test]
#[ignore = "run by a_failed_conversion_shows_its_error"]
fn failure_in_a_fresh_home() {
    let script = "#!/bin/sh\n[ \"$1\" = -version ] && exit 0\necho broken >&2\nexit 1\n";
    if install_ffmpeg(script).is_none() {
        return;
    }
    let clip = PathBuf::from("/clips/broken.mp4");
    let Some(mut app) = AppUnderTest::start(&[Reply::Open(vec![clip]), Reply::Button(0)]) else {
        return;
    };
    app.video(|video, page, uuid| video.select_local(page, uuid));
    app.until("the error", |app| app.shown().len() == 2);
    let shown = app.shown();
    assert!(
        matches!(&shown[1], Shown::Message(m) if m.message == "Failed to process video"),
        "{shown:?}"
    );
    assert!(!app.video(|video, _, _| video.has_data()));
}

#[test]
#[ignore = "run by youtube_without_a_url_says_so"]
fn youtube_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start(&[Reply::Button(0)]) else {
        return;
    };
    app.video(|video, page, uuid| {
        video.select_youtube(page, uuid);
        assert!(video.youtube_animating());
    });
    app.until("the message", |app| app.shown().len() == 1);
    let shown = app.shown();
    assert!(
        matches!(&shown[0], Shown::Message(m) if m.message == "Copy URL to clipboard"),
        "{shown:?}"
    );
    app.until("the animation to stop", |app| {
        !app.video(|video, _, _| video.youtube_animating())
    });
}
