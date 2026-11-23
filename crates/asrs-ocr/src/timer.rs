use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use crate::{OcrError, Worker};

const NUM_TESSERACT_WORKERS: usize = 8;

/// A rectangle as fractions of the video's width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimerRect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl TimerRect {
    const fn of_1080p(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self {
            left: left / 1920.0,
            top: top / 1080.0,
            width: width / 1920.0,
            height: height / 1080.0,
        }
    }

    /// The `[left, top, width, height]` passed to tesseract for a video of this size.
    pub fn rectangle(&self, width: f64, height: f64) -> [f64; 4] {
        [
            self.left * width,
            self.top * height,
            self.width * width,
            self.height * height,
        ]
    }
}

pub const TIMER_RECTS: [TimerRect; 8] = [
    TimerRect::of_1080p(1040.0, 760.0, 80.0, 40.0),
    TimerRect::of_1080p(895.0, 905.0, 130.0, 80.0),
    TimerRect::of_1080p(895.0, 80.0, 130.0, 80.0),
    TimerRect::of_1080p(925.0, 1005.0, 75.0, 30.0),
    TimerRect::of_1080p(865.0, 980.0, 200.0, 90.0),
    TimerRect::of_1080p(865.0, 160.0, 200.0, 90.0),
    TimerRect::of_1080p(900.0, 960.0, 120.0, 68.0),
    TimerRect::of_1080p(900.0, 65.0, 120.0, 68.0),
];

/// Why `readTimerText` rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimerError {
    /// `fs.promises.readFile`.
    Read(String),
    /// A worker's job, whose message tesseract.js also throws uncaught.
    Job(OcrError),
    /// No worker started, so no job runs.
    NoWorker(OcrError),
}

type Done = Box<dyn FnOnce(Result<String, TimerError>) + Send>;

enum Image {
    Path(PathBuf),
    Data(Vec<u8>),
}

struct Job {
    image: Image,
    width: f64,
    height: f64,
    next_rect: usize,
    text: String,
    done: Done,
}

impl Job {
    /// One `addJob("recognize")`. Returns the job while rectangles are left to read.
    fn step(mut self, worker: &mut Worker) -> Option<Self> {
        if let Image::Path(path) = &self.image {
            match std::fs::read(path) {
                Ok(data) => self.image = Image::Data(data),
                Err(error) => {
                    let error = format!("{}: {error}", path.display());
                    (self.done)(Err(TimerError::Read(error)));
                    return None;
                }
            }
        }
        let Image::Data(data) = &self.image else {
            unreachable!("the image was read above");
        };
        let rectangle = TIMER_RECTS[self.next_rect].rectangle(self.width, self.height);
        match worker.recognize(data, rectangle) {
            Ok(text) => self.text.push_str(&text),
            Err(error) => {
                (self.done)(Err(TimerError::Job(error)));
                return None;
            }
        }
        self.next_rect += 1;
        if self.next_rect == TIMER_RECTS.len() {
            (self.done)(Ok(self.text));
            return None;
        }
        Some(self)
    }
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    live_workers: usize,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Shared {
    fn push(&self, job: Job) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.jobs.push_back(job);
        drop(queue);
        self.wake.notify_one();
    }

    fn pop(&self) -> Option<Job> {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if queue.closed {
                return None;
            }
            if let Some(job) = queue.jobs.pop_front() {
                return Some(job);
            }
            queue = self
                .wake
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn lose_worker(&self) -> bool {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.live_workers -= 1;
        queue.live_workers == 0
    }

    fn work(&self, worker: Result<Worker, OcrError>) {
        match worker {
            Ok(mut worker) => {
                while let Some(job) = self.pop() {
                    if let Some(rest) = job.step(&mut worker) {
                        self.push(rest);
                    }
                }
            }
            Err(error) => {
                if self.lose_worker() {
                    while let Some(job) = self.pop() {
                        (job.done)(Err(TimerError::NoWorker(error.clone())));
                    }
                }
            }
        }
    }
}

/// `VideoProcessor.tesseractScheduler` with its eight workers. Jobs queue until the workers are
/// ready; upstream starts its workers with the app and fails jobs added before one is.
pub struct TimerReader {
    shared: Arc<Shared>,
}

impl Default for TimerReader {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerReader {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .live_workers = NUM_TESSERACT_WORKERS;
        for _ in 0..NUM_TESSERACT_WORKERS {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("timer-ocr".to_owned())
                .stack_size(8 << 20)
                .spawn(move || shared.work(Worker::new()))
                .expect("a worker thread starts");
        }
        Self { shared }
    }

    /// `readTimerText(imagePath, width, height)`. `done` runs on a worker thread; an error is a
    /// rejected promise, after which upstream never sees the frame's text.
    pub fn read_timer_text(
        &self,
        path: impl Into<PathBuf>,
        width: f64,
        height: f64,
        done: impl FnOnce(Result<String, TimerError>) + Send + 'static,
    ) {
        self.shared.push(Job {
            image: Image::Path(path.into()),
            width,
            height,
            next_rect: 0,
            text: String::new(),
            done: Box::new(done),
        });
    }
}

impl Drop for TimerReader {
    fn drop(&mut self) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.closed = true;
        queue.jobs.clear();
        drop(queue);
        self.shared.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn jobs_fail_once_no_worker_is_left() {
        let reader = TimerReader {
            shared: Arc::default(),
        };
        reader.shared.queue.lock().expect("queue").live_workers = 2;
        let (sender, receiver) = mpsc::channel();
        for _ in 0..3 {
            let sender = sender.clone();
            reader.read_timer_text("/nonexistent.jpg", 1920.0, 1080.0, move |result| {
                sender.send(result).expect("receiver is alive");
            });
        }
        let error = OcrError("Error: no memory".to_owned());
        reader.shared.work(Err(error.clone()));
        assert!(receiver.try_recv().is_err(), "a worker is left");
        let shared = Arc::clone(&reader.shared);
        let last = error.clone();
        let worker = thread::spawn(move || shared.work(Err(last)));
        for _ in 0..3 {
            let result = receiver.recv().expect("a result");
            assert_eq!(result, Err(TimerError::NoWorker(error.clone())));
        }
        drop(reader);
        worker.join().expect("the worker ends with its reader");
    }
}
