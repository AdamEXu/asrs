use std::time::Duration;

use asrs_app::main_process::MainProcess;
use asrs_ui::gpui::HeadlessAppContext;

const TIMEOUT: Duration = Duration::from_secs(120);

/// Runs the app until it has nothing left to do: `run_until_parked` cannot see the OS threads
/// the hubs decode and export on.
pub fn settle(cx: &mut HeadlessAppContext) {
    loop {
        cx.run_until_parked();
        let waited = cx.update(|cx| {
            MainProcess::hub_windows(cx)
                .iter()
                .filter_map(|(_, hub)| hub.upgrade())
                .any(|hub| {
                    let waited = hub.read(cx).wait_for_threads(TIMEOUT);
                    waited.expect("a decode or export thread stopped without finishing")
                })
        });
        if !waited {
            return;
        }
    }
}
