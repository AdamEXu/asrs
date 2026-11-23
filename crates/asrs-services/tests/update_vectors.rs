#![allow(clippy::disallowed_methods)]

//! Replays `update/checker/*`: upstream's UpdateChecker against scripted releases API replies.
//! Every check, `getShouldPrompt` and `showPrompt` is run on the port in the recorded order; the
//! fetch request, every box option and every opened URL is compared, and a second checker with
//! the release identity must show the same boxes with only the app name substituted.

mod p07_support;

use asrs_js::time::NamedZone;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_services::update::{
    CheckOutcome, FetchReply, FetchRequest, ReleaseProfile, UpdateChecker, UpdateEnv,
};
use p07_support::{
    Fields, RELEASE_NAME, Tally, UPSTREAM_NAME, assert_tree, check_box, check_release_box, from_js,
    str_value, tree_bytes,
};

struct Config {
    platform: String,
    arch: String,
    translation: bool,
    packaged: bool,
    version: String,
    beta: bool,
    repository: String,
}

impl Config {
    fn env<'a>(&'a self, app_name: &'a str) -> UpdateEnv<'a> {
        UpdateEnv {
            platform: &self.platform,
            arch: &self.arch,
            running_under_arm64_translation: self.translation,
            packaged: self.packaged,
            version: &self.version,
            is_beta: self.beta,
            repository: &self.repository,
            app_name,
        }
    }
}

fn reply(fetch: &Json, ctx: &str) -> FetchReply {
    let mut f = Fields::new(fetch, &format!("{ctx}: fetch"));
    let out = if f.has("reject") {
        assert!(f.bool("reject"), "{ctx}: reject is false");
        FetchReply::Rejected
    } else {
        let status = f.raw_number("status");
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

fn check_fetch_effect(ctx: &str, effect: &Json, request: &FetchRequest) {
    let mut f = Fields::new(effect, ctx);
    assert_eq!(f.string("k"), "fetch", "{ctx}: effect kind");
    assert_tree(
        &format!("{ctx}: url"),
        &f.tree("url"),
        &str_value(&request.url),
    );
    assert_tree(
        &format!("{ctx}: init"),
        &f.tree("init"),
        &request_tree(request),
    );
    f.finish();
}

fn check(zone_name: &str) {
    let id = format!("update/checker/{zone_name}");
    let zone = NamedZone::get(zone_name).expect("zone resolves");
    let records = vectors::load(&id).unwrap_or_else(|e| panic!("{e}"));
    let mut tally = Tally::default();
    let mut state: Option<(Config, UpdateChecker, UpdateChecker)> = None;
    let mut repositories = [false, false];
    let mut expected_case = -1.0;
    let mut expected_step = 0.0;
    for record in &records {
        let ctx = format!("{id} line {}", record.line);
        let mut f = Fields::new(&record.json, &ctx);
        let case = f.raw_number("case");
        if f.has("config") {
            tally.record("case");
            expected_case += 1.0;
            assert_eq!(case, expected_case, "{ctx}: case order");
            expected_step = 0.0;
            let name = f.string("name");
            let mut c = Fields::new(&f.take("config"), &format!("{ctx} ({name}): config"));
            let config = Config {
                platform: c.string("platform"),
                arch: c.string("arch"),
                translation: c.bool("translation"),
                packaged: c.bool("packaged"),
                version: c.string("version"),
                beta: c.bool("beta"),
                repository: c.string("repository"),
            };
            c.finish();
            if config.repository == ReleaseProfile::PARITY.repository {
                repositories[0] = true;
            } else if config.repository == ReleaseProfile::RELEASE.repository {
                repositories[1] = true;
            } else {
                panic!("{ctx}: repository {}", config.repository);
            }
            state = Some((config, UpdateChecker::new(), UpdateChecker::new()));
            tally.compare("case");
            f.finish();
            continue;
        }
        let (config, checker, release) = state.as_mut().expect("config record first");
        assert_eq!(case, expected_case, "{ctx}: case");
        assert_eq!(f.raw_number("step"), expected_step, "{ctx}: step order");
        expected_step += 1.0;
        let op = f.string("op");
        tally.record(&op);
        let env = config.env(UPSTREAM_NAME);
        let release_env = config.env(RELEASE_NAME);
        match op.as_str() {
            "check" => {
                let scripted = reply(&f.take("fetch"), &ctx);
                let mut requests = Vec::new();
                let outcome = checker.check(&env, &zone, |request| {
                    requests.push(request.clone());
                    scripted.clone()
                });
                let release_outcome = release.check(&release_env, &zone, |_| scripted.clone());
                assert_eq!(outcome, release_outcome, "{ctx}: release identity outcome");
                assert_eq!(
                    f.bool("rejected"),
                    outcome == CheckOutcome::Rejected,
                    "{ctx}: rejected"
                );
                let effects = f.array("effects");
                assert_eq!(effects.len(), requests.len(), "{ctx}: fetch count");
                for (effect, request) in effects.iter().zip(&requests) {
                    tally.record("fetch");
                    check_fetch_effect(&ctx, effect, request);
                    tally.compare("fetch");
                }
            }
            "shouldPrompt" => {
                let actual = checker.should_prompt();
                assert_eq!(f.bool("result"), actual, "{ctx}: getShouldPrompt");
                assert_eq!(
                    release.should_prompt(),
                    actual,
                    "{ctx}: release getShouldPrompt"
                );
            }
            "prompt" => {
                let response = f.raw_number("response") as usize;
                let shown = checker.prompt_box();
                check_release_box(&ctx, &shown, &release.prompt_box());
                let opened = checker.prompt_response(&env, response);
                let release_opened = release.prompt_response(&release_env, response);
                assert_eq!(
                    opened.as_ref().map(|v| from_js(v).encode()),
                    release_opened.as_ref().map(|v| from_js(v).encode()),
                    "{ctx}: release identity opened URL"
                );
                let effects = f.array("effects");
                let mut effects = effects.iter();
                let effect = effects
                    .next()
                    .unwrap_or_else(|| panic!("{ctx}: no box recorded"));
                tally.record("box");
                let mut e = Fields::new(effect, &format!("{ctx}: box"));
                assert_eq!(e.string("k"), "box", "{ctx}: first effect");
                let parent = e.take("parent");
                let options = e.tree("options");
                check_box(&ctx, &parent, &options, &shown, None);
                e.finish();
                tally.compare("box");
                match (effects.next(), opened) {
                    (None, None) => {}
                    (Some(effect), Some(url)) => {
                        tally.record("open");
                        let mut e = Fields::new(effect, &format!("{ctx}: open"));
                        assert_eq!(e.string("k"), "open", "{ctx}: second effect");
                        assert_tree(&format!("{ctx}: opened"), &e.tree("url"), &from_js(&url));
                        e.finish();
                        tally.compare("open");
                    }
                    (recorded, port) => panic!(
                        "{ctx}: upstream opened {:?}, port opened {:?}",
                        recorded.map(Json::to_compact),
                        port.map(|v| from_js(&v).encode())
                    ),
                }
                assert!(effects.next().is_none(), "{ctx}: extra effects");
            }
            other => panic!("{ctx}: unknown op {other:?}"),
        }
        tally.compare(&op);
        f.finish();
    }
    assert_eq!(
        repositories,
        [true, true],
        "{id}: upstream's repository (ReleaseProfile::PARITY) and asrs's (RELEASE) must both be replayed"
    );
    tally.finish(&id);
}

#[test]
fn checker_utc() {
    check("UTC");
}

#[test]
fn checker_los_angeles() {
    check("America/Los_Angeles");
}

#[test]
fn checker_kolkata() {
    check("Asia/Kolkata");
}

#[test]
fn checker_auckland() {
    check("Pacific/Auckland");
}

#[test]
fn release_profile() {
    assert_eq!(
        ReleaseProfile::RELEASE,
        ReleaseProfile {
            repository: "AdamEXu/asrs",
            update_check: false,
            beta_survey: false,
            contact_email: None,
        }
    );
    assert_eq!(
        (
            ReleaseProfile::PARITY.update_check,
            ReleaseProfile::PARITY.beta_survey
        ),
        (true, true)
    );
}
