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
const CHUNK: usize = 64 * 1024;

type Shared = Arc<Mutex<Child>>;

#[derive(Default)]
pub struct Children(HashMap<Handle, Shared>);

impl Children {
    /// `spawn(command, args, { cwd })`, with a pipe for each of the child's streams. A child that
    /// does not start reports it as events, as Node does.
    pub fn spawn(
        &mut self,
        process: Handle,
        command: &str,
        args: &[JsString],
        cwd: Option<&str>,
        emit: impl Fn(VideoEvent) + Clone + Send + 'static,
    ) {
        match start(command, args, cwd) {
            Ok(mut child) => {
                // The stdin pipe stays open in the child's handle until the child exits.
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let child = Arc::new(Mutex::new(child));
                self.0.insert(process, child.clone());
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
                thread::spawn(move || {
                    let (code, signal) = wait(&child);
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
                });
            }
            Err(error) => {
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
            }
        }
    }

    /// `child.kill()`, but with SIGKILL where Node sends SIGTERM: ffmpeg takes seconds to act on
    /// that, writes frames all the while and reports progress once more. Quitting deletes those
    /// frames, so the child gets a moment to be gone.
    pub fn kill(&mut self, process: Handle) -> bool {
        let Some(child) = self.0.get(&process) else {
            return false;
        };
        let mut child = lock(child);
        if !matches!(child.try_wait(), Ok(None)) || child.kill().is_err() {
            return false;
        }
        let killed = Instant::now();
        while matches!(child.try_wait(), Ok(None)) && killed.elapsed() < KILL_WAIT {
            thread::sleep(Duration::from_millis(1));
        }
        true
    }

    /// Forgets a child that has closed.
    pub fn closed(&mut self, process: Handle) {
        self.0.remove(&process);
    }
}

fn lock(child: &Shared) -> std::sync::MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

fn start(command: &str, args: &[JsString], cwd: Option<&str>) -> io::Result<Child> {
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
        .args(args.iter().map(|arg| arg.to_string_lossy()))
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
        match lock(child).try_wait() {
            Ok(Some(status)) => return (status.code(), signal(status)),
            Ok(None) => {}
            Err(_) => return (None, None),
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

    #[cfg(unix)]
    #[test]
    fn a_killed_child_reports_its_signal() {
        let (send, receive) = mpsc::channel();
        let mut children = Children::default();
        children.spawn(1, "/bin/sleep", &[JsString::from("60")], None, move |e| {
            let _ = send.send(e);
        });
        assert!(children.kill(1));
        let events: Vec<VideoEvent> = receive.into_iter().collect();
        assert!(matches!(
            events.last(),
            Some(VideoEvent::Close {
                code: None,
                signal: Some(_),
                ..
            })
        ));
        assert!(!children.kill(1));
        children.closed(1);
        assert!(!children.kill(1));
    }
}
