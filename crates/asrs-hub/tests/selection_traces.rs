mod common;

#[test]
fn selection_traces_match_upstream() {
    common::check_area(
        "selection",
        &common::Options {
            require_show_ignored: true,
        },
    );
}
