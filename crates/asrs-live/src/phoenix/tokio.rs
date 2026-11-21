//! [`PhoenixTransport`] over real sockets on the live tokio runtime. Each `fetch` spawns a
//! task that GETs the URL and posts the settled result; `next_event` admits them one at a
//! time on the foreground thread, and `fetch_abort` cancels the task so it never settles.

use std::collections::{BTreeMap, VecDeque};

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::channel::{EventReceiver, EventSender, event_channel};
use crate::http::get_body;
use crate::nt4::RequestId;

use super::transport::{HttpEvent, PhoenixTransport};

pub struct TokioPhoenixTransport {
    handle: Handle,
    events_tx: EventSender<HttpEvent>,
    events_rx: EventReceiver<HttpEvent>,
    pending: VecDeque<HttpEvent>,
    next_id: u32,
    requests: BTreeMap<u32, JoinHandle<()>>,
}

impl TokioPhoenixTransport {
    pub fn new(handle: Handle) -> Self {
        let (events_tx, events_rx) = event_channel();
        Self {
            handle,
            events_tx,
            events_rx,
            pending: VecDeque::new(),
            next_id: 1,
            requests: BTreeMap::new(),
        }
    }

    /// The next event a browser would dispatch now, if any.
    pub fn next_event(&mut self) -> Option<HttpEvent> {
        loop {
            let event = match self.pending.pop_front() {
                Some(event) => event,
                None => self.events_rx.try_recv()?,
            };
            let id = match &event {
                HttpEvent::Response(id, ..) | HttpEvent::Failed(id) => *id,
            };
            if self.requests.remove(&id.0).is_some() {
                return Some(event);
            }
        }
    }

    /// Waits until a raw event has arrived, without consuming it; `next_event` then decides
    /// whether it is dispatched.
    pub async fn wait(&mut self) {
        if self.pending.is_empty()
            && let Some(event) = self.events_rx.recv().await
        {
            self.pending.push_back(event);
        }
    }
}

impl Drop for TokioPhoenixTransport {
    fn drop(&mut self) {
        for task in self.requests.values() {
            task.abort();
        }
    }
}

impl PhoenixTransport for TokioPhoenixTransport {
    fn fetch(&mut self, url: &str) -> RequestId {
        let id = self.next_id;
        self.next_id += 1;
        let events = self.events_tx.clone();
        let url = url.to_owned();
        let task = self.handle.spawn(async move {
            let event = match get_body(&url).await {
                Some((status, body)) => HttpEvent::Response(RequestId(id), status, body),
                None => HttpEvent::Failed(RequestId(id)),
            };
            let _ = events.send(event);
        });
        self.requests.insert(id, task);
        RequestId(id)
    }

    fn fetch_abort(&mut self, id: RequestId) {
        if let Some(task) = self.requests.remove(&id.0) {
            task.abort();
        }
    }
}
