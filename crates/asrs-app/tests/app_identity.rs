use asrs_app::menus::app_menu;
use asrs_app::menus::click::{self, ClickEnv, Effect, Reply};
use asrs_app::menus::model::{Command, MenuItem};
use asrs_app::menus::roles::{BuiltItem, build_from_template};
use asrs_js::value::{JsValue, ToJs};
use asrs_model::app_identity::AppIdentity;
use asrs_model::build_constants;
use asrs_model::json_file;
use asrs_model::preferences::Preferences;
use asrs_services::fs::MemFs;
use asrs_services::lifecycle::AppPaths;
use asrs_services::path::PathStyle;
use asrs_services::update::ReleaseProfile;
use asrs_services::window_manager::{Platform, WindowManager};
use asrs_services::windows::Rect;

fn substituted(items: &[BuiltItem]) -> Vec<BuiltItem> {
    items
        .iter()
        .map(|item| BuiltItem {
            label: item
                .label
                .replace(AppIdentity::UPSTREAM.name, AppIdentity::RELEASE.name),
            submenu: item.submenu.as_deref().map(substituted),
            ..item.clone()
        })
        .collect()
}

fn built(platform: Platform, identity: AppIdentity) -> Vec<BuiltItem> {
    let template = app_menu::template(platform, &Preferences::default(), identity);
    build_from_template(&template, platform, identity.name)
}

#[test]
fn release_menus_differ_from_parity_by_the_name_only() {
    for platform in [Platform::Mac, Platform::Windows, Platform::Linux] {
        let upstream = built(platform, AppIdentity::UPSTREAM);
        let release = built(platform, AppIdentity::RELEASE);
        assert_ne!(upstream, release);
        assert_eq!(substituted(&upstream), release, "{platform:?}");
    }
}

#[test]
fn release_window_titles_differ_from_parity_by_the_name_only() {
    for (upstream, release) in [
        ("AdvantageScope", "asrs"),
        ("AdvantageScope Lite", "asrs Lite"),
        (
            "Robot_AdvantageScope.wpilog \u{2014} AdvantageScope",
            "Robot_AdvantageScope.wpilog \u{2014} asrs",
        ),
        (
            "a.wpilog (Searching) \u{2014} AdvantageScope Lite (DS)",
            "a.wpilog (Searching) \u{2014} asrs Lite (DS)",
        ),
        ("AdvantageScopeLog.wpilog", "AdvantageScopeLog.wpilog"),
        (
            "AdvantageScope x.wpilog \u{2014} AdvantageScope",
            "AdvantageScope x.wpilog \u{2014} asrs",
        ),
    ] {
        assert_eq!(AppIdentity::UPSTREAM.window_title(upstream), upstream);
        assert_eq!(AppIdentity::RELEASE.window_title(upstream), release);
    }
}

fn commands(items: &[MenuItem], out: &mut Vec<Command>) {
    for item in items {
        out.extend(item.command);
        commands(item.submenu.as_deref().unwrap_or(&[]), out);
    }
}

/// Every effect of every click on a hub window, and of the layout imports that fail or warn.
fn clicks(platform: Platform, identity: AppIdentity, profile: ReleaseProfile) -> Vec<Effect> {
    let fs = MemFs::new();
    let paths = AppPaths {
        style: PathStyle::Posix,
        user_data: "/u".to_owned(),
        temp: "/t".to_owned(),
        bundled_assets: "/b".to_owned(),
        packaged: true,
    };
    let prefs = Preferences::default();
    fs.put(
        &paths.prefs(),
        &json_file::stringify(&prefs.to_js()).expect("prefs"),
    );
    fs.put("/old.json", br#"{"layout":[]}"#);
    fs.put("/unknown.json", b"{}");
    fs.put(
        "/other.json",
        br#"{"version":"0","hubs":[],"satellites":[]}"#,
    );
    let mut windows = WindowManager::for_platform(platform);
    let (hub, options) = windows.create_hub(None);
    windows.set_bounds(
        hub,
        Rect {
            x: 0.0,
            y: 0.0,
            width: options.width,
            height: options.height,
        },
    );
    let application_state = || JsValue::Null;
    let env = ClickEnv {
        platform,
        arch: "arm64",
        identity,
        profile,
        fs: &fs,
        paths: &paths,
        windows: &windows,
        asset_status: "AdvantageScope will check for updates periodically.",
        layout_default_path: "AdvantageScope date.json".to_owned(),
        application_state: &application_state,
    };
    let mut all = Vec::new();
    commands(&app_menu::template(platform, &prefs, identity), &mut all);
    let mut out = Vec::new();
    for command in all {
        click::click(command, true, Some(hub), &env, &mut out).expect("a click");
    }
    let then = out
        .iter()
        .find_map(|effect| match effect {
            Effect::OpenDialog { then, .. } if *then == click::Then::ImportLayout => {
                Some(then.clone())
            }
            _ => None,
        })
        .expect("the import dialog");
    for file in ["/old.json", "/unknown.json", "/other.json"] {
        let reply = Reply::Open(vec![file.to_owned()]);
        click::resume(then.clone(), &reply, &env, &mut out).expect("an import");
    }
    out
}

#[test]
fn release_clicks_differ_from_parity_by_the_name_version_and_help_links_only() {
    const HELP_LINKS: [(&str, &str); 3] = [
        (
            "https://github.com/Mechanical-Advantage/AdvantageScope/issues",
            "https://github.com/AdamEXu/asrs/issues",
        ),
        (
            "mailto:software@team6328.org",
            "https://github.com/AdamEXu/asrs/issues",
        ),
        (
            "https://github.com/Mechanical-Advantage/AdvantageScope",
            "https://github.com/AdamEXu/asrs",
        ),
    ];
    let version = |identity: AppIdentity| format!("Version: {}\\n", identity.full_version());
    for platform in [Platform::Mac, Platform::Windows, Platform::Linux] {
        let upstream = clicks(platform, AppIdentity::UPSTREAM, ReleaseProfile::PARITY);
        let release = clicks(platform, AppIdentity::RELEASE, ReleaseProfile::RELEASE);
        let mut differing = 0;
        let mut links = Vec::new();
        for (upstream, release) in upstream.iter().zip(&release) {
            if let (Effect::OpenExternal(upstream), Effect::OpenExternal(release)) =
                (upstream, release)
            {
                if upstream != release {
                    links.push((upstream.as_str(), release.as_str()));
                }
                continue;
            }
            let text = format!("{upstream:?}");
            assert_eq!(
                text.replace(AppIdentity::UPSTREAM.name, AppIdentity::RELEASE.name)
                    .replace(
                        &version(AppIdentity::UPSTREAM),
                        &version(AppIdentity::RELEASE)
                    ),
                format!("{release:?}"),
                "{platform:?}"
            );
            differing += usize::from(upstream != release);
        }
        assert_eq!(upstream.len(), release.len());
        assert_eq!(links, HELP_LINKS, "{platform:?}");
        assert_eq!(
            differing, 6,
            "About, custom assets, asset status, export, two imports"
        );
    }
}

#[test]
fn versions() {
    let upstream = build_constants::VERSION;
    assert_eq!(AppIdentity::UPSTREAM.full_version(), upstream);
    let own = env!("CARGO_PKG_VERSION");
    assert_ne!(own, upstream);
    assert_eq!(
        AppIdentity::RELEASE.version_line(),
        format!("asrs {own} (AdvantageScope {upstream})")
    );
    let identity = if cfg!(feature = "parity") {
        AppIdentity::UPSTREAM
    } else {
        AppIdentity::RELEASE
    };
    assert_eq!(asrs_app::IDENTITY, identity);
    let paths = AppPaths {
        style: PathStyle::Posix,
        user_data: "/u".to_owned(),
        temp: "/t".to_owned(),
        bundled_assets: "/b".to_owned(),
        packaged: true,
    };
    assert_eq!(paths.app_version(), upstream, "layouts and the state file");
}
