//! The one tokio runtime that owns every live source's sockets (PLAN section 1.2).

use std::future::Future;
use std::io;

use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;

pub use tokio::runtime::Handle as RuntimeHandle;

pub struct LiveRuntime {
    runtime: Option<Runtime>,
}

/// A name lookup runs on a blocking thread, and dropping the runtime would wait for it.
impl Drop for LiveRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl LiveRuntime {
    pub const WORKER_THREADS: usize = 2;

    pub fn new() -> io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(Self::WORKER_THREADS)
            .thread_name("asrs-live")
            .enable_io()
            .enable_time()
            .build()?;
        Ok(Self {
            runtime: Some(runtime),
        })
    }

    fn runtime(&self) -> &Runtime {
        self.runtime.as_ref().expect("only taken on drop")
    }

    pub fn handle(&self) -> &RuntimeHandle {
        self.runtime().handle()
    }

    /// Tasks spawned on the runtime that have not finished.
    pub fn alive_tasks(&self) -> usize {
        self.handle().metrics().num_alive_tasks()
    }

    pub fn spawn<F>(&self, future: F) -> LiveTask
    where
        F: Future<Output = ()> + Send + 'static,
    {
        LiveTask(self.runtime().spawn(future))
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
