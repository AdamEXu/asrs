//! The windows without a hub renderer behind them: `main.ts`'s `openLicenses`,
//! `openSourceListHelp` and `openBetaWelcome`.

pub mod beta_welcome;
pub mod layout;
pub mod licenses;
pub mod page;
pub mod source_list_help;

pub use beta_welcome::{Acknowledged, BetaWelcomePage};
pub use licenses::LicensesPage;
pub use source_list_help::SourceListHelpPage;

use asrs_model::source_list_config::SourceListConfig;
use asrs_tabs::info::{License, SourceListHelp};
use asrs_ui::gpui::{App, AppContext};

use crate::windows::{self, Opening, SingletonOpening, WindowId, WindowKind};

fn title(page: &str) -> String {
    format!("{page} \u{2014} {}", crate::IDENTITY.name)
}

/// The window of `openLicenses`, or `None` when it was open and now has the focus.
pub fn open_licenses(
    cx: &mut App,
    parent: WindowId,
    licenses: Vec<License>,
) -> SingletonOpening<LicensesPage> {
    windows::open_licenses(cx, parent, |window, cx| {
        window.set_window_title(&title("Licenses"));
        cx.new(|cx| LicensesPage::new(licenses, window, cx))
    })
}

/// The window of `openSourceListHelp`.
pub fn open_source_list_help(
    cx: &mut App,
    parent: WindowId,
    config: &SourceListConfig,
) -> Opening<SourceListHelpPage> {
    let mut help = SourceListHelp::default();
    if let Err(e) = help.set_config(config) {
        eprintln!("asrs: source list help: {}: {}", e.name, e.message);
    }
    windows::open_source_list_help(cx, parent, |window, cx| {
        window.set_window_title(&title(&help.title()));
        cx.new(|cx| SourceListHelpPage::new(help, window, cx))
    })
}

/// The window of `openBetaWelcome`, with main's `isAlpha()` posted to its page.
pub fn open_beta_welcome(
    cx: &mut App,
    parent: WindowId,
    is_alpha: bool,
) -> Opening<BetaWelcomePage> {
    windows::open_modal(cx, WindowKind::BetaWelcome, parent, |window, cx| {
        window.set_window_title("Welcome");
        cx.new(|cx| {
            let mut page = BetaWelcomePage::new(crate::IDENTITY.name, window, cx);
            page.message(is_alpha, cx);
            page
        })
    })
}
