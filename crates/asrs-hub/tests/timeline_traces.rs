mod common;

#[test]
fn timeline_traces_match_upstream() {
    let (specs, canvases) = common::check_area(
        "timeline",
        &common::Options {
            require_show_ignored: false,
        },
    );
    eprintln!("{specs} timeline specs, {canvases} canvas traces equal");
}
