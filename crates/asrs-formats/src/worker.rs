//! The decode-worker protocol of `HistoricalDataSource.ts` (`HistoricalDataSource_WorkerRequest`,
//! `HistoricalDataSource_WorkerResponse`) and the thread each worker runs on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use asrs_js::string::JsString;
use asrs_log::log::{SerializedLog, SerializedLogField};

pub enum Request {
    /// `request.data`: one slot per file main read; `None` is a file that could not be read.
    Start(Vec<Option<Arc<[u8]>>>),
    ParseField(JsString),
}

/// `HistoricalDataSource_WorkerFieldResponse`.
pub struct FieldResponse {
    pub key: JsString,
    pub data: SerializedLogField,
    pub generated_parent: bool,
}

pub enum Response {
    Progress(f64),
    Initial {
        log: Box<SerializedLog>,
        is_partial: bool,
    },
    Failed,
    Fields(Vec<FieldResponse>),
}

/// How a worker's `onmessage` ends without returning normally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abort {
    /// Any JS throw out of the handler: a DataView RangeError, a thrown string, a TypeError on
    /// an `undefined` slot. The worker stays alive and handles later requests.
    Threw,
    /// Upstream loops forever or its realm dies. The port detects it and stops instead; the
    /// worker never handles another request.
    NeverReturns,
    /// Upstream continues in a way the port does not model yet; see the text. The worker's state
    /// is then neither upstream's nor modeled, so it never handles another request.
    Unmodeled(&'static str),
}

/// One upstream worker realm: its module state lives in the implementing struct, and
/// `handle` is one `onmessage` call. Responses go to `post` in the order upstream posts them.
pub trait DecodeWorker: Send {
    fn handle(&mut self, request: Request, post: &mut dyn FnMut(Response)) -> Result<(), Abort>;
}

/// What a decode thread sends back: each posted response, then `Done` when the request's
/// handler has finished (the oracle's `__oracleDone`).
pub enum WorkerMessage {
    Response(Response),
    Done(Result<(), Abort>),
}

/// A worker on its own OS thread, named `decode-<id>`. Requests are handled in order. After a
/// `NeverReturns` or `Unmodeled` abort the thread drops every later request without answering,
/// as the stuck upstream worker would.
pub struct WorkerThread {
    requests: Option<Sender<Request>>,
    messages: Receiver<WorkerMessage>,
    terminated: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WorkerThread {
    pub fn spawn(id: &str, mut worker: Box<dyn DecodeWorker>) -> std::io::Result<Self> {
        let (request_tx, request_rx) = channel::<Request>();
        let (message_tx, message_rx) = channel();
        let terminated = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&terminated);
        let thread = std::thread::Builder::new()
            .name(format!("decode-{id}"))
            .spawn(move || {
                let mut stuck = false;
                for request in request_rx {
                    if stuck || stop.load(Ordering::Acquire) {
                        continue;
                    }
                    let end = worker.handle(request, &mut |response| {
                        if !stop.load(Ordering::Acquire) {
                            let _ = message_tx.send(WorkerMessage::Response(response));
                        }
                    });
                    stuck = matches!(end, Err(Abort::NeverReturns | Abort::Unmodeled(_)));
                    if !stop.load(Ordering::Acquire) {
                        let _ = message_tx.send(WorkerMessage::Done(end));
                    }
                }
            })?;
        Ok(WorkerThread {
            requests: Some(request_tx),
            messages: message_rx,
            terminated,
            thread: Some(thread),
        })
    }

    pub fn post(&self, request: Request) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(request);
        }
    }

    /// The next message, or `None` once the worker is terminated or its thread has ended.
    pub fn recv(&self) -> Option<WorkerMessage> {
        if self.terminated.load(Ordering::Acquire) {
            return None;
        }
        self.messages.recv().ok()
    }

    pub fn try_recv(&self) -> Option<WorkerMessage> {
        if self.terminated.load(Ordering::Acquire) {
            return None;
        }
        self.messages.try_recv().ok()
    }

    /// `terminate()`: nothing more is delivered and no further request is handled. A request
    /// already running finishes on the detached thread with its messages discarded.
    pub fn terminate(&mut self) {
        self.terminated.store(true, Ordering::Release);
        self.requests = None;
        self.thread.take();
    }
}

impl Drop for WorkerThread {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub fn run_request(
    worker: &mut dyn DecodeWorker,
    request: Request,
) -> (Vec<Response>, Result<(), Abort>) {
    let mut responses = Vec::new();
    let end = worker.handle(request, &mut |response| responses.push(response));
    (responses, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl DecodeWorker for Echo {
        fn handle(
            &mut self,
            request: Request,
            post: &mut dyn FnMut(Response),
        ) -> Result<(), Abort> {
            match request {
                Request::Start(_) => {
                    post(Response::Progress(0.5));
                    Err(Abort::NeverReturns)
                }
                Request::ParseField(_) => {
                    post(Response::Failed);
                    Ok(())
                }
            }
        }
    }

    #[test]
    fn a_thread_answers_in_order_and_goes_silent_after_never_returns() {
        let mut worker = WorkerThread::spawn("test", Box::new(Echo)).unwrap();
        worker.post(Request::ParseField(JsString::from("a")));
        worker.post(Request::Start(vec![]));
        worker.post(Request::ParseField(JsString::from("b")));
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(match worker.recv().unwrap() {
                WorkerMessage::Response(Response::Progress(_)) => "progress",
                WorkerMessage::Response(Response::Failed) => "failed",
                WorkerMessage::Response(_) => "other",
                WorkerMessage::Done(Ok(())) => "done",
                WorkerMessage::Done(Err(_)) => "aborted",
            });
        }
        assert_eq!(seen, ["failed", "done", "progress", "aborted"]);
        assert!(
            worker
                .messages
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        worker.terminate();
        assert!(worker.recv().is_none());
    }
}
