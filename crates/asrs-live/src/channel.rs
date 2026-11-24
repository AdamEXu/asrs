//! Events from a source's tokio task to its foreground half, delivered in send order.
//!
//! The receiver needs no tokio context: the foreground drains it with [`EventReceiver::drain`]
//! or awaits [`EventReceiver::recv`] on any executor, such as GPUI's.

use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Runs on the sending thread after each event is queued.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

pub fn event_channel<E: Send>() -> (EventSender<E>, EventReceiver<E>) {
    event_channel_notifying(Arc::new(|| {}))
}

pub fn event_channel_notifying<E: Send>(notify: Notify) -> (EventSender<E>, EventReceiver<E>) {
    let (tx, rx) = unbounded_channel();
    (EventSender { tx, notify }, EventReceiver(rx))
}

pub struct EventSender<E> {
    tx: UnboundedSender<E>,
    notify: Notify,
}

impl<E> Clone for EventSender<E> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            notify: self.notify.clone(),
        }
    }
}

impl<E> EventSender<E> {
    /// Returns the event back when the receiver is gone.
    pub fn send(&self, event: E) -> Result<(), E> {
        self.tx.send(event).map_err(|e| e.0)?;
        (self.notify)();
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

pub struct EventReceiver<E>(UnboundedReceiver<E>);

impl<E> EventReceiver<E> {
    pub fn try_recv(&mut self) -> Option<E> {
        self.0.try_recv().ok()
    }

    /// Every event sent so far, oldest first.
    pub fn drain(&mut self) -> Vec<E> {
        let mut out = Vec::new();
        while let Ok(event) = self.0.try_recv() {
            out.push(event);
        }
        out
    }

    /// The next event, or `None` once every sender is gone and the queue is empty.
    pub async fn recv(&mut self) -> Option<E> {
        self.0.recv().await
    }

    pub fn close(&mut self) {
        self.0.close();
    }
}
