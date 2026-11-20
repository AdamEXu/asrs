//! Every emoji upstream shows as a label, kept in one module so swapping them for lucide icons
//! touches nothing else (PLAN section 7, question 1).

use asrs_model::tab_type::TabType;

/// `getTabIcon`.
pub fn tab_icon(tab: TabType) -> &'static str {
    tab.icon()
}

/// A tab's title in the tab strip (`Tabs.ts`): the icon, a space, then `name`, or the default
/// title when `name` is empty.
pub fn tab_title(tab: TabType, name: &str) -> String {
    let name = if name.is_empty() {
        tab.default_title()
    } else {
        name
    };
    format!("{} {name}", tab_icon(tab))
}

/// The prefix `TableRenderer` puts before a boolean cell's text.
pub fn table_boolean(value: bool) -> &'static str {
    if value { "\u{1f7e9}" } else { "\u{1f7e5}" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_match_upstream() {
        assert_eq!(tab_title(TabType::LineGraph, ""), "\u{1f4c9} Line Graph");
        assert_eq!(tab_title(TabType::Mechanism, "Arm"), "\u{2699}\u{fe0f} Arm");
        assert_eq!(tab_title(TabType::Documentation, ""), "\u{1f4d6} ");
        assert_eq!(table_boolean(true), "\u{1f7e9}");
        assert_eq!(table_boolean(false), "\u{1f7e5}");
    }
}
