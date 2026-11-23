use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use gpui_kit::{BackgroundExecutor, ForegroundExecutor, TestDispatcher};

const CONTENTION: Duration = Duration::from_secs(2);
const LIMIT: Duration = Duration::from_secs(120);
// The scheduler scans its whole queue on every step, so an unbounded backlog stalls it.
const BACKLOG: usize = 8;

#[test]
fn a_timed_block_does_not_deadlock_with_another_threads_post() {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let dispatcher = Arc::new(TestDispatcher::new(0));
        let background = BackgroundExecutor::new(dispatcher.clone());
        let foreground = ForegroundExecutor::new(dispatcher);
        background.allow_parking();
        background.set_block_on_ticks(0..=4);

        let start = Instant::now();
        let poster = thread::spawn({
            let background = background.clone();
            move || {
                let queued = Arc::new(AtomicUsize::new(0));
                let mut posts = 0_usize;
                while start.elapsed() < CONTENTION {
                    if queued.load(Ordering::SeqCst) >= BACKLOG {
                        thread::yield_now();
                        continue;
                    }
                    queued.fetch_add(1, Ordering::SeqCst);
                    let queued = queued.clone();
                    background
                        .spawn(async move {
                            queued.fetch_sub(1, Ordering::SeqCst);
                        })
                        .detach();
                    posts += 1;
                }
                posts
            }
        });
        let mut blocks = 0_usize;
        while start.elapsed() < CONTENTION {
            assert!(
                foreground
                    .block_with_timeout(Duration::ZERO, pending::<()>())
                    .is_err()
            );
            blocks += 1;
        }
        let posts = poster.join().unwrap();
        done.send((blocks, posts)).unwrap();
    });
    let (blocks, posts) = finished
        .recv_timeout(LIMIT)
        .expect("the scheduler deadlocked or panicked");
    println!("{blocks} blocks, {posts} posts");
}
