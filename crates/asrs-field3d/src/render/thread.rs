use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::frame::Frame;
use super::gpu::{RenderError, Renderer, Timings};

/// One finished frame: tightly packed BGRA rows, top row first.
pub struct RenderedFrame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub render_ms: f64,
    pub timings: Timings,
}

#[derive(Default)]
struct State {
    #[cfg(feature = "parity")]
    lose_device: bool,
    pending: Option<Frame>,
    latest: Option<RenderedFrame>,
    pool: Vec<Vec<u8>>,
    release_at: Option<Instant>,
    holds_targets: bool,
    shutdown: bool,
    failure: Option<String>,
}

/// Devices opened for one frame before the thread gives up on it.
const DEVICE_ATTEMPTS: u32 = 3;

struct Shared {
    state: Mutex<State>,
    wake_thread: Condvar,
}

/// The dedicated thread that owns the wgpu device. The foreground submits immutable frames into
/// a queue of depth one where a newer frame replaces one not yet started, and is woken through
/// `waker` (in the app, a channel whose receiver calls `cx.notify`) when a frame is ready.
/// A lost device is reported on stderr and replaced, as a lost WebGL context is restored.
/// Pixel buffers circulate through a pool: give each presented buffer back with `recycle`.
pub struct RenderThread {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
    adapter: String,
}

impl RenderThread {
    pub fn spawn(waker: impl Fn() + Send + 'static) -> Result<Self, RenderError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake_thread: Condvar::new(),
        });
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_shared = shared.clone();
        let handle = std::thread::Builder::new()
            .name("field3d-render".into())
            .spawn(move || {
                let mut renderer = match Renderer::new() {
                    Ok(r) => {
                        let info = r.adapter_info();
                        let _ = ready_tx.send(Ok(format!("{} ({:?})", info.name, info.backend)));
                        r
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                run(&thread_shared, &mut renderer, &waker);
            })
            .map_err(|e| RenderError::Device(e.to_string()))?;
        let adapter = ready_rx
            .recv()
            .map_err(|e| RenderError::Device(e.to_string()))??;
        Ok(RenderThread {
            shared,
            handle: Some(handle),
            adapter,
        })
    }

    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// Latest wins: a frame still waiting is dropped in favour of this one. It also cancels a
    /// release the thread has not done yet.
    pub fn submit(&self, frame: Frame) {
        let mut state = self.shared.state.lock().expect("render state");
        state.pending = Some(frame);
        state.release_at = None;
        self.shared.wake_thread.notify_one();
    }

    /// The newest finished frame, if one arrived since the last call. An older finished frame
    /// that was never taken has already gone back to the pool.
    pub fn take(&self) -> Option<RenderedFrame> {
        self.shared
            .state
            .lock()
            .expect("render state")
            .latest
            .take()
    }

    /// Loses the device before the next frame, as a driver reset would.
    #[cfg(feature = "parity")]
    pub fn lose_device(&self) {
        self.shared.state.lock().expect("render state").lose_device = true;
    }

    /// Why the thread ended, if it did: the device was lost and no other could be opened.
    pub fn failure(&self) -> Option<String> {
        self.shared
            .state
            .lock()
            .expect("render state")
            .failure
            .clone()
    }

    /// Once no frame came for `delay`, frees the render targets, the shadow maps and the pooled
    /// pixel buffers. The next frame makes them again.
    pub fn release_after(&self, delay: Duration) {
        let mut state = self.shared.state.lock().expect("render state");
        state.release_at = Some(Instant::now() + delay);
        self.shared.wake_thread.notify_one();
    }

    pub fn cancel_release(&self) {
        self.shared.state.lock().expect("render state").release_at = None;
    }

    /// Whether the last frame's targets are still there.
    pub fn holds_targets(&self) -> bool {
        self.shared
            .state
            .lock()
            .expect("render state")
            .holds_targets
    }

    pub fn recycle(&self, buffer: Vec<u8>) {
        let mut state = self.shared.state.lock().expect("render state");
        if state.pool.len() < 4 {
            state.pool.push(buffer);
        }
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.shutdown = true;
        }
        self.shared.wake_thread.notify_one();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run(shared: &Shared, renderer: &mut Renderer, waker: &dyn Fn()) {
    let mut seq = 0;
    loop {
        let (frame, mut buffer) = {
            let mut state = shared.state.lock().expect("render state");
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(frame) = state.pending.take() {
                    #[cfg(feature = "parity")]
                    if std::mem::take(&mut state.lose_device) {
                        renderer.lose_device();
                    }
                    let buffer = state.pool.pop().unwrap_or_default();
                    state.holds_targets = true;
                    break (frame, buffer);
                }
                match state.release_at {
                    Some(at) if at <= Instant::now() => {
                        state.release_at = None;
                        state.pool = Vec::new();
                        drop(state);
                        renderer.release_targets();
                        state = shared.state.lock().expect("render state");
                        state.holds_targets = renderer.holds_targets();
                    }
                    Some(at) => {
                        let wait = at.saturating_duration_since(Instant::now());
                        state = shared
                            .wake_thread
                            .wait_timeout(state, wait)
                            .expect("render state")
                            .0;
                    }
                    None => state = shared.wake_thread.wait(state).expect("render state"),
                }
            }
        };
        let start = Instant::now();
        let mut attempts = 1;
        let timings = loop {
            let error = match renderer.render(&frame, &mut buffer) {
                Ok(timings) => break timings,
                Err(e) if attempts == DEVICE_ATTEMPTS => e,
                Err(e) => {
                    eprintln!("asrs: 3D renderer: {e}; opening another device");
                    attempts += 1;
                    match Renderer::new() {
                        Ok(r) => {
                            *renderer = r;
                            continue;
                        }
                        Err(e) => e,
                    }
                }
            };
            eprintln!("asrs: 3D renderer stopped: {error}");
            shared.state.lock().expect("render state").failure = Some(error.to_string());
            waker();
            return;
        };
        let render_ms = start.elapsed().as_secs_f64() * 1e3;
        seq += 1;
        {
            let mut state = shared.state.lock().expect("render state");
            let previous = state.latest.replace(RenderedFrame {
                seq,
                width: frame.options.width.max(1),
                height: frame.options.height.max(1),
                bgra: buffer,
                render_ms,
                timings,
            });
            if let Some(previous) = previous
                && state.pool.len() < 4
            {
                state.pool.push(previous.bgra);
            }
        }
        waker();
    }
}
