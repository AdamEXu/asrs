//! `child_process.spawn` for the video processor: each child's output, `exit` and `close` events.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, ErrorKind, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use asrs_js::string::JsString;
use asrs_services::video::{Handle, VideoEvent};

const EXIT_POLL: Duration = Duration::from_millis(10);
const KILL_WAIT: Duration = Duration::from_millis(200);
const QUIT_START_WAIT: Duration = Duration::from_secs(5);
const CHUNK: usize = 64 * 1024;

enum State {
    Starting { killed: bool },
    Running { child: Child, killed: bool },
    Failed,
}

type Shared = Arc<Mutex<State>>;

#[derive(Default)]
pub struct Children(HashMap<Handle, Shared>);

impl Children {
    /// `spawn(command, args, { cwd })`, with a pipe for each of the child's streams. A child that
    /// does not start reports it as events, as Node does. It starts on its own thread: Windows
    /// returns from `CreateProcess` when the antivirus has scanned the program.
    pub fn spawn(
        &mut self,
        process: Handle,
        command: &str,
        args: &[JsString],
        cwd: Option<&str>,
        emit: impl Fn(VideoEvent) + Clone + Send + 'static,
    ) {
        let child = Arc::new(Mutex::new(State::Starting { killed: false }));
        self.0.insert(process, child.clone());
        let command = command.to_owned();
        let args: Vec<String> = args.iter().map(JsString::to_string_lossy).collect();
        let cwd = cwd.map(str::to_owned);
        thread::spawn(move || {
            let started = start(&command, &args, cwd.as_deref());
            report(process, &command, &child, started, emit);
        });
    }

    /// `child.kill()`, but with SIGKILL where Node sends SIGTERM: ffmpeg takes seconds to act on
    /// that, writes frames all the while and reports progress once more. Quitting deletes those
    /// frames, so the child gets a moment to be gone. A child still starting as the app quits gets
    /// longer: nothing would kill it once the app is gone.
    pub fn kill(&mut self, process: Handle, quitting: bool) -> bool {
        let Some(child) = self.0.get(&process) else {
            return false;
        };
        match &mut *lock(child) {
            State::Starting { killed } => *killed = true,
            State::Running { child, killed } => {
                if !matches!(child.try_wait(), Ok(None)) || child.kill().is_err() {
                    return false;
                }
                *killed = true;
            }
            State::Failed => return false,
        }
        if quitting {
            wait_while(child, QUIT_START_WAIT, |state| {
                matches!(state, State::Starting { .. })
            });
        }
        wait_while(child, KILL_WAIT, |state| match state {
            State::Starting { .. } => true,
            State::Running { child, .. } => matches!(child.try_wait(), Ok(None)),
            State::Failed => false,
        });
        true
    }

    /// Forgets a child that has closed.
    pub fn closed(&mut self, process: Handle) {
        self.0.remove(&process);
    }
}

fn lock(child: &Shared) -> std::sync::MutexGuard<'_, State> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait_while(child: &Shared, limit: Duration, mut waits: impl FnMut(&mut State) -> bool) {
    let start = Instant::now();
    while waits(&mut lock(child)) && start.elapsed() < limit {
        thread::sleep(Duration::from_millis(1));
    }
}

/// Reports a started child's events until it closes.
fn report(
    process: Handle,
    command: &str,
    shared: &Shared,
    started: io::Result<Child>,
    emit: impl Fn(VideoEvent) + Clone + Send + 'static,
) {
    let mut child = match started {
        Ok(child) => child,
        Err(error) => {
            *lock(shared) = State::Failed;
            let (code, errno) = errno(&error);
            emit(VideoEvent::SpawnError {
                process,
                code: code.to_owned(),
                errno,
                syscall: format!("spawn {command}"),
                path: command.to_owned(),
            });
            emit(VideoEvent::Close {
                process,
                code: Some(errno),
                signal: None,
            });
            return;
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    {
        let mut state = lock(shared);
        let killed = matches!(*state, State::Starting { killed: true }) && child.kill().is_ok();
        // The stdin pipe stays open in the child's handle until the child exits.
        *state = State::Running { child, killed };
    }
    let readers = [
        stdout.map(|pipe| {
            let emit = emit.clone();
            pump(pipe, move |data| emit(VideoEvent::Stdout { process, data }))
        }),
        stderr.map(|pipe| {
            let emit = emit.clone();
            pump(pipe, move |data| emit(VideoEvent::Stderr { process, data }))
        }),
    ];
    let (code, signal) = wait(shared);
    emit(VideoEvent::Exit {
        process,
        code,
        signal: signal.clone(),
    });
    for reader in readers.into_iter().flatten() {
        let _ = reader.join();
    }
    emit(VideoEvent::Close {
        process,
        code,
        signal,
    });
}

fn start(command: &str, args: &[String], cwd: Option<&str>) -> io::Result<Child> {
    // A program relative to `cwd` is found there on every platform.
    let relative = command
        .strip_prefix("./")
        .or_else(|| command.strip_prefix(".\\"));
    let program = match (relative, cwd) {
        (Some(name), Some(cwd)) => Path::new(cwd).join(name).into_os_string(),
        _ => OsString::from(command),
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn()
}

/// Emits whole lines, each ended by a CR or an LF. ffmpeg writes a line in many small pieces, and
/// the processor, like upstream, looks for a stream's size and rate, or a report's `frame=`, at
/// once in one chunk: Node's event loop is slow enough to find the pieces joined, a blocking read
/// is not.
fn pump(
    mut pipe: impl Read + Send + 'static,
    emit: impl Fn(Vec<u8>) + Send + 'static,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = vec![0; CHUNK];
        let mut line = Vec::new();
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    for part in buffer[..n].split_inclusive(|b| matches!(b, b'\r' | b'\n')) {
                        line.extend_from_slice(part);
                        if matches!(line.last(), Some(b'\r' | b'\n')) {
                            emit(std::mem::take(&mut line));
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        if !line.is_empty() {
            emit(line);
        }
    })
}

/// Polls, so that a kill can take the child between two looks.
fn wait(child: &Shared) -> (Option<i32>, Option<String>) {
    loop {
        if let State::Running { child, killed } = &mut *lock(child) {
            match child.try_wait() {
                // Windows has no signals: libuv reports the one it was asked to send, and no code.
                Ok(Some(_)) if cfg!(windows) && *killed => {
                    return (None, Some("SIGTERM".to_owned()));
                }
                Ok(Some(status)) => return (status.code(), signal(status)),
                Ok(None) => {}
                Err(_) => return (None, None),
            }
        }
        thread::sleep(EXIT_POLL);
    }
}

#[cfg(unix)]
fn signal(status: std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    let name = match status.signal()? {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return Some("SIGTERM".to_owned()),
    };
    Some(name.to_owned())
}

#[cfg(not(unix))]
fn signal(_: std::process::ExitStatus) -> Option<String> {
    None
}

/// The error's `code` and `errno` as libuv reports them.
fn errno(error: &io::Error) -> (&'static str, i32) {
    let windows = cfg!(windows);
    match error.kind() {
        ErrorKind::NotFound if windows => ("ENOENT", -4058),
        ErrorKind::PermissionDenied if windows => ("EACCES", -4092),
        ErrorKind::NotFound => ("ENOENT", -2),
        ErrorKind::PermissionDenied => ("EACCES", -13),
        _ => ("UNKNOWN", error.raw_os_error().map_or(-4094, |e| -e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn run(command: &str, args: &[&str]) -> Vec<VideoEvent> {
        let (send, receive) = mpsc::channel();
        let args: Vec<JsString> = args.iter().map(|a| JsString::from(*a)).collect();
        let mut children = Children::default();
        children.spawn(7, command, &args, None, move |event| {
            let _ = send.send(event);
        });
        receive.into_iter().collect()
    }

    #[test]
    fn a_missing_program_reports_enoent_then_closes() {
        let events = run("asrs-no-such-program", &[]);
        assert!(matches!(
            &events[0],
            VideoEvent::SpawnError { process: 7, code, path, .. }
                if code == "ENOENT" && path == "asrs-no-such-program"
        ));
        assert!(matches!(events[1], VideoEvent::Close { process: 7, .. }));
        assert_eq!(events.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn output_then_exit_then_close() {
        let events = run("/bin/sh", &["-c", "echo out; echo err >&2; exit 3"]);
        let data = |stdout: bool| -> Vec<u8> {
            events
                .iter()
                .flat_map(|e| match e {
                    VideoEvent::Stdout { data, .. } if stdout => data.clone(),
                    VideoEvent::Stderr { data, .. } if !stdout => data.clone(),
                    _ => Vec::new(),
                })
                .collect()
        };
        assert_eq!(data(true), b"out\n");
        assert_eq!(data(false), b"err\n");
        let exit = VideoEvent::Exit {
            process: 7,
            code: Some(3),
            signal: None,
        };
        assert!(events.contains(&exit));
        let close = VideoEvent::Close {
            process: 7,
            code: Some(3),
            signal: None,
        };
        assert_eq!(events.last(), Some(&close));
    }

    #[cfg(unix)]
    #[test]
    fn lines_written_in_pieces_arrive_whole() {
        let script = "for p in '320x180, ' '25 fps, ' 'x\nframe= 1\r' 'frame= 2\rtail'; do \
                      printf \"$p\" >&2; sleep 0.05; done";
        let lines: Vec<String> = run("/bin/sh", &["-c", script])
            .into_iter()
            .filter_map(|e| match e {
                VideoEvent::Stderr { data, .. } => String::from_utf8(data).ok(),
                _ => None,
            })
            .collect();
        assert_eq!(
            lines,
            ["320x180, 25 fps, x\n", "frame= 1\r", "frame= 2\r", "tail"]
        );
    }

    const LONG: (&str, &[&str]) = if cfg!(windows) {
        ("ping", &["-n", "60", "127.0.0.1"])
    } else {
        ("/bin/sleep", &["60"])
    };

    fn long_args() -> Vec<JsString> {
        LONG.1.iter().map(|arg| JsString::from(*arg)).collect()
    }

    #[test]
    fn a_killed_child_reports_its_signal() {
        let (send, receive) = mpsc::channel();
        let mut children = Children::default();
        children.spawn(1, LONG.0, &long_args(), None, move |e| {
            let _ = send.send(e);
        });
        while matches!(*lock(&children.0[&1]), State::Starting { .. }) {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(children.kill(1, false));
        let events: Vec<VideoEvent> = receive.into_iter().collect();
        assert!(matches!(
            events.last(),
            Some(VideoEvent::Close {
                code: None,
                signal: Some(_),
                ..
            })
        ));
        assert!(!children.kill(1, false));
        children.closed(1);
        assert!(!children.kill(1, false));
    }

    #[test]
    fn a_child_killed_as_it_starts_is_killed_once_started() {
        let (send, receive) = mpsc::channel();
        let emit = move |event| {
            let _ = send.send(event);
        };
        let shared = Arc::new(Mutex::new(State::Starting { killed: false }));
        let mut children = Children::default();
        children.0.insert(1, shared.clone());
        assert!(children.kill(1, false));
        let args: Vec<String> = LONG.1.iter().map(|arg| (*arg).to_owned()).collect();
        let started = start(LONG.0, &args, None);
        report(1, LONG.0, &shared, started, emit);
        let closes: Vec<(Option<i32>, bool)> = receive
            .into_iter()
            .filter_map(|event| match event {
                VideoEvent::Exit { code, signal, .. } | VideoEvent::Close { code, signal, .. } => {
                    Some((code, signal.is_some()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(closes, [(None, true), (None, true)]);
        assert!(!children.kill(1, false));
    }

    #[test]
    fn quitting_waits_for_a_child_that_takes_long_to_start() {
        let shared = Arc::new(Mutex::new(State::Starting { killed: false }));
        let mut children = Children::default();
        children.0.insert(1, shared.clone());
        let starter = shared.clone();
        let thread = thread::spawn(move || {
            thread::sleep(KILL_WAIT * 3);
            let args: Vec<String> = LONG.1.iter().map(|arg| (*arg).to_owned()).collect();
            report(1, LONG.0, &starter, start(LONG.0, &args, None), |_| {});
        });
        assert!(children.kill(1, true));
        let gone = match &mut *lock(&shared) {
            State::Running { child, .. } => matches!(child.try_wait(), Ok(Some(_))),
            _ => false,
        };
        assert!(gone);
        thread.join().expect("reported");
    }

    #[test]
    fn a_child_that_did_not_start_cannot_be_killed() {
        let (send, receive) = mpsc::channel();
        let mut children = Children::default();
        children.spawn(1, "asrs-no-such-program", &[], None, move |event| {
            let _ = send.send(event);
        });
        assert_eq!(receive.into_iter().count(), 2);
        assert!(!children.kill(1, false));
    }
}
