//! The one tokio runtime that owns every live source's sockets (PLAN section 1.2).

use std::future::Future;
use std::io;

use tokio::runtime::{Builder, Handle, Runtime};
use tokio::task::JoinHandle;

pub struct LiveRuntime {
    runtime: Runtime,
}

impl LiveRuntime {
    pub const WORKER_THREADS: usize = 2;

    pub fn new() -> io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(Self::WORKER_THREADS)
            .thread_name("asrs-live")
            .enable_time()
            .build()?;
        Ok(Self { runtime })
    }

    pub fn handle(&self) -> &Handle {
        self.runtime.handle()
    }

    pub fn spawn<F>(&self, future: F) -> LiveTask
    where
        F: Future<Output = ()> + Send + 'static,
    {
        LiveTask(self.runtime.spawn(future))
    }
}

/// A source's socket task. Dropping it aborts the task, so a stopped or replaced source
/// cannot keep posting events.
pub struct LiveTask(JoinHandle<()>);

impl LiveTask {
    pub fn abort(&self) {
        self.0.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl Drop for LiveTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
