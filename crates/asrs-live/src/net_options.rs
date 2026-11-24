//! What every tokio transport is built with.

use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::channel::Notify;

/// Where a connection to `(host, port)` really goes, so a test can put a server upstream reaches
/// on a fixed port behind an ephemeral one. URLs and headers keep the original address.
pub type Dial = Arc<dyn Fn(&str, u16) -> (String, u16) + Send + Sync>;

#[derive(Clone)]
pub struct NetOptions {
    pub handle: Handle,
    /// Runs on a runtime thread whenever an event is waiting.
    pub notify: Notify,
    pub dial: Option<Dial>,
}

impl NetOptions {
    pub fn new(handle: Handle) -> Self {
        Self {
            handle,
            notify: Arc::new(|| {}),
            dial: None,
        }
    }

    pub(crate) fn dial(&self, host: &str, port: u16) -> (String, u16) {
        match &self.dial {
            Some(dial) => dial(host, port),
            None => (host.to_owned(), port),
        }
    }
}

/// How many received bytes of one connection may wait for the foreground thread. A full buffer
/// stops reading the socket, so the server is throttled by TCP instead of growing this process.
pub(crate) const MAX_WAITING_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Backlog(Arc<Semaphore>);

/// Held by a waiting message until the foreground thread takes it.
pub(crate) struct Waiting(#[allow(dead_code)] OwnedSemaphorePermit);

impl Backlog {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Semaphore::new(MAX_WAITING_BYTES)))
    }

    pub(crate) async fn reserve(&self, bytes: usize) -> Option<Waiting> {
        let permits = bytes.clamp(1, MAX_WAITING_BYTES) as u32;
        let permit = self.0.clone().acquire_many_owned(permits).await.ok()?;
        Some(Waiting(permit))
    }
}
