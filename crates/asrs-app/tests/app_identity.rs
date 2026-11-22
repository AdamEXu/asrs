use asrs_app::menus::app_menu;
use asrs_app::menus::roles::{BuiltItem, build_from_template};
use asrs_model::app_identity::AppIdentity;
use asrs_model::preferences::Preferences;
use asrs_services::window_manager::Platform;

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
