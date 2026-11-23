use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use gpui_kit::{BackgroundExecutor, ForegroundExecutor, TestDispatcher};

const POSTS: usize = 100_000;

#[test]
fn a_timed_block_does_not_deadlock_with_another_threads_post() {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let dispatcher = Arc::new(TestDispatcher::new(0));
        let background = BackgroundExecutor::new(dispatcher.clone());
        let foreground = ForegroundExecutor::new(dispatcher);
        background.allow_parking();
        background.set_block_on_ticks(0..=1);

        let posted = Arc::new(AtomicBool::new(false));
        let poster = thread::spawn({
            let (background, posted) = (background.clone(), posted.clone());
            move || {
                for _ in 0..POSTS {
                    background.spawn(async {}).detach();
                }
                posted.store(true, Ordering::SeqCst);
            }
        });
        let mut blocks = 0_usize;
        while !posted.load(Ordering::SeqCst) {
            assert!(
                foreground
                    .block_with_timeout(Duration::ZERO, pending::<()>())
                    .is_err()
            );
            background.run_until_parked();
            blocks += 1;
        }
        poster.join().unwrap();
        done.send(blocks).unwrap();
    });
    let blocks = finished
        .recv_timeout(Duration::from_secs(120))
        .expect("the scheduler deadlocked or panicked");
    assert!(blocks > 0);
}
