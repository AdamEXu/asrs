use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use asrs_live::{LiveRuntime, event_channel};

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on_plain_thread<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        thread::park();
    }
}

#[test]
fn events_arrive_in_send_order_on_a_non_tokio_thread() {
    let runtime = LiveRuntime::new().unwrap();
    let (tx, mut rx) = event_channel::<u32>();
    let _task = runtime.spawn(async move {
        for i in 0..2000 {
            if i % 500 == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            if tx.send(i).is_err() {
                return;
            }
        }
    });
    let received: Vec<u32> = block_on_plain_thread(async {
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    });
    assert_eq!(received, (0..2000).collect::<Vec<_>>());
}

#[test]
fn dropping_the_task_aborts_it_and_closes_the_channel() {
    let runtime = LiveRuntime::new().unwrap();
    let (tx, mut rx) = event_channel::<u32>();
    let task = runtime.spawn(async move {
        let _ = tx.send(1);
        loop {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    assert_eq!(block_on_plain_thread(rx.recv()), Some(1));
    drop(task);
    assert_eq!(block_on_plain_thread(rx.recv()), None);
}

#[test]
fn drain_returns_everything_sent_so_far() {
    let (tx, mut rx) = event_channel::<&'static str>();
    tx.send("a").unwrap();
    tx.clone().send("b").unwrap();
    assert_eq!(rx.drain(), vec!["a", "b"]);
    assert!(rx.drain().is_empty());
    rx.close();
    assert_eq!(tx.send("c"), Err("c"));
}

#[test]
fn runtime_uses_two_named_workers() {
    let runtime = LiveRuntime::new().unwrap();
    let (tx, mut rx) = event_channel::<String>();
    let _task = runtime.spawn(async move {
        let _ = tx.send(thread::current().name().unwrap_or_default().to_owned());
    });
    assert_eq!(
        block_on_plain_thread(rx.recv()).as_deref(),
        Some("asrs-live")
    );
    assert_eq!(
        runtime.handle().metrics().num_workers(),
        LiveRuntime::WORKER_THREADS
    );
}

#[test]
fn tasks_can_open_sockets() {
    let runtime = LiveRuntime::new().unwrap();
    let (tx, mut rx) = event_channel::<std::io::Result<()>>();
    let _task = runtime.spawn(async move {
        let result = async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;
            let client = tokio::spawn(tokio::net::TcpStream::connect(addr));
            listener.accept().await?;
            client.await.map_err(std::io::Error::other)??;
            Ok(())
        }
        .await;
        let _ = tx.send(result);
    });
    assert!(block_on_plain_thread(rx.recv()).unwrap().is_ok());
}
