#![allow(clippy::disallowed_methods)]

//! Replays `update/dialogs/*`: main.ts's About item, the beta block of a hub's dom-ready, the
//! welcome window, open-feedback, prompt-update, the startup update check and "Check for
//! Updates...", recorded from the real main.ts and UpdateChecker. Each op runs on the port and
//! every recorded field (boxes, quits, opened URLs, clipboard, child windows, port messages,
//! button messages, fetches, beta file writes and bytes) is compared. Boxes are also produced
//! with the release identity, which may differ only by the app name.

mod p07_support;

use std::cell::{Cell, RefCell};

use asrs_js::string::JsError;
use asrs_js::time::{Clock, NamedZone};
use asrs_js::value::JsValue;
use asrs_model::build_constants::{self, Distribution};
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_services::about::{AboutEnv, about};
use asrs_services::beta::{self, BetaConfig, BetaStartup};
use asrs_services::constants;
use asrs_services::dialog::MessageBox;
use asrs_services::fs::{DirEntry, Fs, MemFs, ReadError};
use asrs_services::path::PathStyle;
use asrs_services::update::{
    FetchReply, FetchRequest, ReleaseProfile, UpdateChecker, UpdateEnv, feedback_url,
};
use p07_support::{
    Fields, RELEASE_NAME, Tally, UPSTREAM_NAME, assert_tree, check_box, check_release_box,
    cve_object, from_js, str_value, tree_bytes,
};

struct CountingFs {
    inner: MemFs,
    watched: String,
    writes: Cell<usize>,
}

impl Fs for CountingFs {
    fn exists(&self, path: &str) -> bool {
        self.inner.exists(path)
    }
    fn read(&self, path: &str) -> Result<Vec<u8>, JsError> {
        self.inner.read(path)
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<(), JsError> {
        if path == self.watched {
            self.writes.set(self.writes.get() + 1);
        }
        self.inner.write(path, data)
    }
    fn copy(&self, from: &str, to: &str) -> Result<(), JsError> {
        panic!("beta code copied {from} to {to}")
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>, ReadError> {
        self.inner.read_file(path)
    }
    fn remove(&self, path: &str) {
        self.inner.remove(path)
    }
    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, JsError> {
        self.inner.read_dir(path)
    }
    fn mkdir(&self, path: &str) -> Result<(), JsError> {
        self.inner.mkdir(path)
    }
}

struct FakeClock(Cell<f64>);

impl Clock for FakeClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

fn reply(fetch: &Json, ctx: &str) -> FetchReply {
    let mut f = Fields::new(&cve_object(fetch, ctx), &format!("{ctx}: fetch"));
    let out = if f.has("reject") {
        assert!(f.bool("reject"), "{ctx}: reject is false");
        FetchReply::Rejected
    } else {
        let status = f.number("status");
        let body = tree_bytes(&f.tree("body"), ctx);
        FetchReply::Response {
            status: status as u16,
            body,
        }
    };
    f.finish();
    out
}

fn request_tree(request: &FetchRequest) -> Value {
    Value::Object(vec![
        ("method".into(), str_value(request.method)),
        (
            "headers".into(),
            Value::Object(
                request
                    .headers
                    .iter()
                    .map(|(k, v)| ((*k).into(), str_value(v)))
                    .collect(),
            ),
        ),
    ])
}

#[derive(Default)]
struct Effects {
    boxes: Vec<(MessageBox, MessageBox)>,
    quits: usize,
    opened: Vec<JsValue>,
    clipboard: Vec<String>,
    created: Vec<beta::WelcomeWindow>,
    posted: Vec<bool>,
    feedback: Vec<bool>,
    update: Vec<bool>,
    fetches: Vec<FetchRequest>,
    throw: Option<&'static str>,
}

struct Process {
    platform: String,
    arch: String,
    packaged: bool,
    version: String,
    translation: bool,
}

impl Process {
    fn env<'a>(&'a self, app_name: &'a str) -> UpdateEnv<'a> {
        UpdateEnv {
            platform: &self.platform,
            arch: &self.arch,
            running_under_arm64_translation: self.translation,
            packaged: self.packaged,
            version: &self.version,
            is_beta: true,
            repository: ReleaseProfile::PARITY.repository,
            app_name,
        }
    }

    fn about_env<'a>(&'a self, app_name: &'a str) -> AboutEnv<'a> {
        AboutEnv {
            app_name,
            distribution: Distribution::Frc6328,
            platform: &self.platform,
            arch: &self.arch,
            packaged: self.packaged,
            version: &self.version,
        }
    }
}

struct World {
    zone: NamedZone,
    fs: CountingFs,
    clock: FakeClock,
    beta_file: String,
    beta: BetaConfig,
    process: Process,
    checker: UpdateChecker,
    release_checker: UpdateChecker,
    hub: Option<f64>,
    welcome: Option<f64>,
}

impl World {
    fn hub(&mut self, id: f64, ctx: &str) {
        match self.hub {
            None => self.hub = Some(id),
            Some(hub) => assert_eq!(hub, id, "{ctx}: hub window changed"),
        }
    }

    fn check_for_update(&mut self, fx: &mut Effects, scripted: &FetchReply, always: bool, response: usize) {
        let env = self.process.env(UPSTREAM_NAME);
        let mut requests = Vec::new();
        let run = self.checker.check_for_update(&env, &self.zone, always, |request| {
            requests.push(request.clone());
            scripted.clone()
        });
        let release_env = self.process.env(RELEASE_NAME);
        let release_run = self
            .release_checker
            .check_for_update(&release_env, &self.zone, always, |_| scripted.clone());
        fx.fetches.extend(requests);
        fx.update.extend(run.button);
        assert_eq!(run.button, release_run.button, "release identity button");
        match (run.prompt, release_run.prompt) {
            (None, None) => {}
            (Some(shown), Some(release_shown)) => {
                fx.boxes.push((shown, release_shown));
                self.respond_to_update(fx, response);
            }
            other => panic!("release identity prompt differs: {other:?}"),
        }
    }

    fn respond_to_update(&mut self, fx: &mut Effects, response: usize) {
        let env = self.process.env(UPSTREAM_NAME);
        let opened = self.checker.prompt_response(&env, response);
        let release_env = self.process.env(RELEASE_NAME);
        let release_opened = self.release_checker.prompt_response(&release_env, response);
        assert_eq!(
            opened.as_ref().map(|v| from_js(v).encode()),
            release_opened.as_ref().map(|v| from_js(v).encode()),
            "release identity opened URL"
        );
        fx.opened.extend(opened);
    }
}

fn run_op(world: &mut World, op: &str, args: &mut Fields, ctx: &str) -> Effects {
    let mut fx = Effects::default();
    match op {
        "ready" => {
            let scripted = reply(&args.take("fetch"), ctx);
            world.process.platform = args.string("platform");
            world.process.arch = args.string("arch");
            world.process.packaged = args.bool("packaged");
            world.clock.0.set(args.number("now"));
            world.check_for_update(&mut fx, &scripted, false, 0);
        }
        "setup" => {
            world.process.platform = args.string("platform");
            world.process.arch = args.string("arch");
            world.process.packaged = args.bool("packaged");
            world.clock.0.set(args.number("now"));
            let mut config = Fields::new(&cve_object(&args.take("config"), ctx), &format!("{ctx}: config"));
            world.beta.is_alpha = config.bool("isAlpha");
            world.beta.survey_url = match config.take("surveyUrl") {
                Json::Null => None,
                Json::String(s) => Some(Box::leak(s.to_string_lossy().into_boxed_str())),
                other => panic!("{ctx}: surveyUrl {}", other.to_compact()),
            };
            config.finish();
            match args.take("betaFile") {
                Json::Null => world.fs.inner.remove(&world.beta_file),
                Json::String(s) => world
                    .fs
                    .inner
                    .put(&world.beta_file, s.to_string_lossy().as_bytes()),
                other => panic!("{ctx}: betaFile {}", other.to_compact()),
            }
        }
        "about" => {
            let response = args.number("response") as usize;
            let shown = about(&world.process.about_env(UPSTREAM_NAME));
            let release = about(&world.process.about_env(RELEASE_NAME));
            assert_eq!(shown.copy_text, release.copy_text, "{ctx}: release identity copy text");
            fx.clipboard.extend(shown.response(response).map(str::to_owned));
            fx.boxes.push((shown.message_box, release.message_box));
        }
        "domReady" => {
            let hub = args.number("window");
            world.hub(hub, ctx);
            let response = match args.take("response") {
                Json::Null => 0,
                other => p07_support::tree_number(&Value::from_json(&other).expect("response"))
                    .expect("response number") as usize,
            };
            for (name, value) in world.checker.hub_ready_messages(Some(&world.beta)) {
                match name {
                    "show-update-button" => fx.update.push(value),
                    "show-feedback-button" => fx.feedback.push(value),
                    other => panic!("{ctx}: unexpected hub message {other}"),
                }
            }
            let scratch = MemFs::new();
            if let Ok(bytes) = world.fs.inner.read(&world.beta_file) {
                scratch.put(&world.beta_file, &bytes);
            }
            let release = beta::beta_startup(
                &scratch,
                Some(&world.beta_file),
                Some(&world.beta),
                &world.clock,
                Distribution::Frc6328,
                RELEASE_NAME,
            );
            let startup = beta::beta_startup(
                &world.fs,
                Some(&world.beta_file),
                Some(&world.beta),
                &world.clock,
                Distribution::Frc6328,
                UPSTREAM_NAME,
            );
            let release_box = |release: Result<BetaStartup, JsError>| match release {
                Ok(BetaStartup::Expired(b) | BetaStartup::Survey(b)) => b,
                other => panic!("{ctx}: release identity startup differs: {other:?}"),
            };
            match startup {
                Err(e) => {
                    assert_eq!(release.err().map(|r| r.name), Some(e.name), "{ctx}: release identity error");
                    fx.throw = Some(e.name);
                }
                Ok(BetaStartup::Nothing) => {
                    assert_eq!(release.ok(), Some(BetaStartup::Nothing), "{ctx}: release identity startup");
                }
                Ok(BetaStartup::Expired(shown)) => {
                    fx.boxes.push((shown, release_box(release)));
                    if beta::beta_expired_response(response) {
                        fx.quits += 1;
                    }
                }
                Ok(BetaStartup::Welcome) => {
                    assert_eq!(release.ok(), Some(BetaStartup::Welcome), "{ctx}: release identity startup");
                    fx.created.push(beta::beta_welcome_window(&world.process.platform));
                }
                Ok(BetaStartup::Survey(shown)) => {
                    fx.boxes.push((shown, release_box(release)));
                    let mut opened = Vec::new();
                    beta::beta_survey_response(
                        &world.fs,
                        Some(&world.beta_file),
                        Some(&world.beta),
                        &world.clock,
                        response,
                        |url| opened.push(url.to_owned()),
                    )
                    .unwrap_or_else(|e| panic!("{ctx}: survey response failed: {e:?}"));
                    fx.opened.extend(opened.into_iter().map(|u| JsValue::string(u.as_str())));
                }
            }
        }
        "welcomeReady" => {
            assert_eq!(Some(args.number("window")), world.welcome, "{ctx}: welcome window");
            fx.posted.push(beta::beta_welcome_ready(Some(&world.beta)));
        }
        "welcomeDone" => {
            assert_eq!(Some(args.number("window")), world.welcome, "{ctx}: welcome window");
            beta::beta_welcome_done(&world.fs, Some(&world.beta_file), Some(&world.beta), &world.clock)
                .unwrap_or_else(|e| panic!("{ctx}: welcome completion failed: {e:?}"));
        }
        "welcomeClose" => {
            assert_eq!(Some(args.number("window")), world.welcome, "{ctx}: welcome window");
            if beta::beta_welcome_closed() {
                fx.quits += 1;
            }
        }
        "feedback" => {
            let hub = args.number("window");
            world.hub(hub, ctx);
            fx.opened.push(JsValue::string(feedback_url(ReleaseProfile::PARITY.repository).as_str()));
        }
        "promptUpdate" => {
            let hub = args.number("window");
            world.hub(hub, ctx);
            let response = args.number("response") as usize;
            let shown = world.checker.prompt_box();
            let release_shown = world.release_checker.prompt_box();
            fx.boxes.push((shown, release_shown));
            world.respond_to_update(&mut fx, response);
        }
        "menuCheck" => {
            let scripted = reply(&args.take("fetch"), ctx);
            let response = args.number("response") as usize;
            world.check_for_update(&mut fx, &scripted, true, response);
        }
        other => panic!("{ctx}: unknown op {other:?}"),
    }
    fx
}

fn check(zone_name: &str) {
    let id = format!("update/dialogs/{zone_name}");
    let zone = NamedZone::get(zone_name).expect("zone resolves");
    let records = vectors::load(&id).unwrap_or_else(|e| panic!("{e}"));
    let mut tally = Tally::default();
    let mut records = records.iter();

    let first = records.next().expect("config record");
    let ctx = format!("{id} line {}", first.line);
    let mut f = Fields::new(&first.json, &ctx);
    assert_eq!(f.string("op"), "config", "{ctx}");
    let mut c = Fields::new(&cve_object(&f.take("result"), &ctx), &format!("{ctx}: config"));
    let user_data = c.string("userData");
    let beta_file = c.string("betaFile");
    let version = c.string("version");
    let translation = c.bool("translation");
    c.finish();
    f.finish();
    let beta = beta::beta_config(true, &zone).expect("parity builds are betas");
    assert_eq!(
        constants::beta_filename(PathStyle::Posix, &user_data, beta.year),
        beta_file,
        "{ctx}: beta file name"
    );
    assert_eq!(version, build_constants::VERSION, "{ctx}: app version");

    let mut world = World {
        zone,
        fs: CountingFs {
            inner: MemFs::new(),
            watched: beta_file.clone(),
            writes: Cell::new(0),
        },
        clock: FakeClock(Cell::new(0.0)),
        beta_file,
        beta,
        process: Process {
            platform: String::new(),
            arch: String::new(),
            packaged: false,
            version,
            translation,
        },
        checker: UpdateChecker::new(),
        release_checker: UpdateChecker::new(),
        hub: None,
        welcome: None,
    };
    let last_case = RefCell::new(-1.0);
    let mut expected_step = 0.0;

    for record in records {
        let ctx = format!("{id} line {}", record.line);
        let mut f = Fields::new(&record.json, &ctx);
        let case = f.raw_number("case");
        let step = f.raw_number("step");
        if case != *last_case.borrow() {
            assert_eq!(case, *last_case.borrow() + 1.0, "{ctx}: case order");
            *last_case.borrow_mut() = case;
            expected_step = 0.0;
        }
        assert_eq!(step, expected_step, "{ctx}: step order");
        expected_step += 1.0;
        let op = f.string("op");
        tally.record(&op);
        let mut args = if f.has("args") {
            Fields::new(&cve_object(&f.take("args"), &ctx), &format!("{ctx}: args"))
        } else {
            Fields::new(&Json::Object(Vec::new()), &format!("{ctx}: args"))
        };
        world.fs.writes.set(0);
        let fx = run_op(&mut world, &op, &mut args, &ctx);
        args.finish();

        match (f.has("throw"), fx.throw) {
            (true, Some(name)) => assert_eq!(f.string("throw"), name, "{ctx}: thrown error"),
            (false, None) => assert_tree(&format!("{ctx}: result"), &f.tree("result"), &Value::Null),
            (recorded, port) => panic!("{ctx}: upstream threw: {recorded}, port threw: {port:?}"),
        }

        let boxes = f.array("boxes");
        assert_eq!(boxes.len(), fx.boxes.len(), "{ctx}: box count");
        for (recorded, (shown, release_shown)) in boxes.iter().zip(&fx.boxes) {
            tally.record("box");
            let mut b = Fields::new(recorded, &format!("{ctx}: box"));
            let parent = b.take("parent");
            let options = b.tree("options");
            check_box(&ctx, &parent, &options, shown, world.hub);
            check_release_box(&ctx, shown, release_shown);
            b.finish();
            tally.compare("box");
        }

        assert_eq!(f.raw_number("quits") as usize, fx.quits, "{ctx}: app.quit calls");

        let opened = f.array("opened");
        assert_eq!(opened.len(), fx.opened.len(), "{ctx}: opened count");
        for (recorded, url) in opened.iter().zip(&fx.opened) {
            tally.record("open");
            let expected = Value::from_json(recorded).expect("opened url");
            assert_tree(&format!("{ctx}: opened"), &expected, &from_js(url));
            tally.compare("open");
        }

        let clipboard = f.array("clipboard");
        assert_eq!(clipboard.len(), fx.clipboard.len(), "{ctx}: clipboard writes");
        for (recorded, text) in clipboard.iter().zip(&fx.clipboard) {
            tally.record("clipboard");
            assert_eq!(recorded, &Json::string(text.as_str()), "{ctx}: clipboard text");
            tally.compare("clipboard");
        }

        let created = f.array("created");
        assert_eq!(created.len(), fx.created.len(), "{ctx}: child windows");
        for (recorded, window) in created.iter().zip(&fx.created) {
            tally.record("window");
            let mut w = Fields::new(recorded, &format!("{ctx}: window"));
            world.welcome = Some(w.raw_number("id"));
            assert_eq!(Some(w.raw_number("parent")), world.hub, "{ctx}: welcome parent");
            let expected = Value::Object(vec![
                ("width".into(), Value::number(window.width)),
                ("height".into(), Value::number(window.height)),
                ("useContentSize".into(), Value::Bool(window.use_content_size)),
                ("resizable".into(), Value::Bool(window.resizable)),
                ("modal".into(), Value::Bool(window.modal)),
                ("show".into(), Value::Bool(window.show)),
            ]);
            assert_tree(&format!("{ctx}: welcome options"), &w.tree("options"), &expected);
            w.finish();
            tally.compare("window");
        }

        for (key, values) in [("posted", &fx.posted), ("feedback", &fx.feedback), ("update", &fx.update)] {
            let recorded = f.array(key);
            assert_eq!(recorded.len(), values.len(), "{ctx}: {key} count");
            for (json, value) in recorded.iter().zip(values) {
                tally.record(key);
                assert_tree(
                    &format!("{ctx}: {key}"),
                    &Value::from_json(json).expect("bool"),
                    &Value::Bool(*value),
                );
                tally.compare(key);
            }
        }

        let fetches = f.array("fetches");
        assert_eq!(fetches.len(), fx.fetches.len(), "{ctx}: fetch count");
        for (recorded, request) in fetches.iter().zip(&fx.fetches) {
            tally.record("fetch");
            let mut r = Fields::new(recorded, &format!("{ctx}: fetch"));
            assert_tree(&format!("{ctx}: url"), &r.tree("url"), &str_value(&request.url));
            assert_tree(&format!("{ctx}: init"), &r.tree("init"), &request_tree(request));
            r.finish();
            tally.compare("fetch");
        }

        assert_eq!(f.raw_number("betaWrites") as usize, world.fs.writes.get(), "{ctx}: beta file writes");
        let actual_file = world
            .fs
            .inner
            .exists(&world.beta_file)
            .then(|| world.fs.inner.read(&world.beta_file).expect("beta file reads"));
        match f.take("betaFile") {
            Json::Null => assert_eq!(actual_file, None, "{ctx}: beta file should be absent"),
            other => {
                let expected = tree_bytes(&Value::from_json(&other).expect("bytes"), &ctx);
                assert_eq!(
                    Some(String::from_utf8_lossy(&expected).into_owned()),
                    actual_file.map(|b| String::from_utf8_lossy(&b).into_owned()),
                    "{ctx}: beta file bytes"
                );
            }
        }
        tally.compare(&op);
        f.finish();
    }
    tally.finish(&id);
}

#[test]
fn dialogs_utc() {
    check("UTC");
}

#[test]
fn dialogs_los_angeles() {
    check("America/Los_Angeles");
}

#[test]
fn dialogs_kolkata() {
    check("Asia/Kolkata");
}

#[test]
fn dialogs_auckland() {
    check("Pacific/Auckland");
}
