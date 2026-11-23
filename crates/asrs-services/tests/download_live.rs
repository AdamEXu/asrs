#![allow(clippy::disallowed_methods)]

//! Runs the scenarios of `files/download-live` against the port and compares what happened with the
//! recording of upstream's main.ts and the real ssh2 package against the same fixture. Scenarios
//! with a scripted server run everywhere; the ones on a real sshd need one on this machine.

mod download_support;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use asrs_parity::Json;
use asrs_parity::vectors::load;
use asrs_services::download::client::{ClientEvent, Clients, Login, Target, lookup};
use asrs_services::download::downloader::{
    CONNECT_TIMEOUT_MS, ConnectOptions, ListItem, Progress, SSH_PORT, Token, WindowMessage,
};
use asrs_services::download::{DownloadConfig, DownloadEvent, DownloadEvents, DownloadService};
use asrs_services::path::PathStyle;
use download_support::{
    Fixture, Tools, array, blackhole_works, number, optional_text, scenarios, text, tools,
};

const VECTOR: &str = "files/download-live/macos-aarch64";
const LIMIT: Duration = Duration::from_secs(60);
const TIME_SCALE: f64 = 10.0;
const DEFAULT_TIMEOUT_MS: f64 = 30000.0;
const WORKERS: usize = 4;

fn kind(scenario: &Json) -> String {
    text(scenario.get("server").expect("server"), "kind")
}

fn is_client(scenario: &Json) -> bool {
    scenario.get("level").is_some()
}

fn timeout_ms(scenario: &Json) -> f64 {
    number(scenario, "timeoutMs").unwrap_or(DEFAULT_TIMEOUT_MS)
}

fn host(scenario: &Json) -> String {
    optional_text(scenario, "host").unwrap_or("127.0.0.1".to_owned())
}

fn k(name: &str) -> (&'static str, Json) {
    ("k", Json::string(name))
}

struct Effects {
    list: Vec<Json>,
    partial: bool,
    replace: Vec<(String, &'static str)>,
}

impl Effects {
    fn new(scenario: &Json, fixture: &Fixture) -> Effects {
        Effects {
            list: Vec::new(),
            partial: matches!(scenario.get("partial"), Some(Json::Bool(true))),
            replace: vec![
                (fixture.root.display().to_string(), "<root>"),
                (fixture.port.to_string(), "<port>"),
            ],
        }
    }

    fn text(&self, text: &str) -> Json {
        let mut out = text.to_owned();
        for (from, to) in &self.replace {
            out = out.replace(from, to);
        }
        Json::string(&out)
    }

    fn push(&mut self, effect: Json) {
        let repeats = optional_text(&effect, "name").as_deref() == Some("set-list")
            || text(&effect, "k") == "uncaught";
        let same = self.list.last().map(Json::to_compact) == Some(effect.to_compact());
        if !(repeats && same) {
            self.list.push(effect);
        }
    }

    fn send(&mut self, name: &str, data: Json) {
        self.push(Json::object([
            k("send"),
            ("name", Json::string(name)),
            ("data", data),
        ]));
    }

    fn window(&mut self, message: WindowMessage) {
        match message {
            WindowMessage::SetList(mut items) => {
                items.sort_by(|a, b| a.name.encode_utf16().cmp(b.name.encode_utf16()));
                self.send("set-list", Json::Array(items.iter().map(item).collect()));
            }
            WindowMessage::ShowError(message) => {
                let text = self.text(&message);
                self.send("show-error", text);
            }
            WindowMessage::ShowAlert(message) => {
                let text = self.text(&message);
                self.send("show-alert", text);
            }
            WindowMessage::SetProgress(Progress::Preparing) => {
                self.send("set-progress", Json::Number(0.0));
            }
            WindowMessage::SetProgress(Progress::Finished) => {
                self.send("set-progress", Json::Number(1.0));
            }
            WindowMessage::SetProgress(Progress::Transferring { current, total }) => {
                let is_progress = |effect: &Json| text(effect, "k") == "progress";
                if self.partial && self.list.iter().any(is_progress) {
                    return;
                }
                if self.list.last().is_some_and(is_progress) {
                    self.list.pop();
                }
                self.list.push(Json::object([
                    k("progress"),
                    ("total", Json::Number(total)),
                    ("complete", Json::Bool(current == total)),
                ]));
            }
        }
    }

    fn count(&self, name: &str) -> usize {
        self.list.iter().filter(|e| matches(e, name)).count()
    }

    /// Drops what came after the effect `await` was satisfied by.
    fn stop_at(&mut self, name: &str, count: usize) {
        let mut left = count;
        let end = self.list.iter().position(|effect| {
            left -= usize::from(matches(effect, name));
            left == 0
        });
        self.list.truncate(end.expect("an awaited effect") + 1);
    }
}

fn matches(effect: &Json, name: &str) -> bool {
    match text(effect, "k").as_str() {
        "send" => text(effect, "name") == name,
        other => other == name,
    }
}

fn item(item: &ListItem) -> Json {
    Json::object([
        ("name", Json::string(&item.name)),
        ("size", Json::Number(item.size.expect("a size"))),
        ("isFolder", Json::Bool(item.is_folder)),
    ])
}

struct Session {
    events: Option<DownloadEvents>,
    effects: Effects,
    dialogs: VecDeque<Token>,
}

impl Session {
    fn drain(&mut self) {
        while let Some(event) = self.events.as_mut().and_then(DownloadEvents::try_next) {
            match event {
                DownloadEvent::Window(message) => self.effects.window(message),
                DownloadEvent::ChooseFolder { token, .. } => {
                    self.dialogs.push_back(token);
                    self.effects.push(Json::object([k("open")]));
                }
                DownloadEvent::ChooseFile { token, .. } => {
                    self.dialogs.push_back(token);
                    self.effects.push(Json::object([k("save")]));
                }
                DownloadEvent::AskOpenLog { token, .. } => {
                    self.dialogs.push_back(token);
                    self.effects.push(Json::object([k("box")]));
                }
                DownloadEvent::DestroyWindow => {
                    self.effects.push(Json::object([k("destroyWindow")]));
                }
                DownloadEvent::FocusHub => self.effects.push(Json::object([k("focusHub")])),
                DownloadEvent::OpenFiles(files) => {
                    let files = files.iter().map(|f| self.effects.text(f)).collect();
                    self.effects.push(Json::object([
                        k("openFiles"),
                        (
                            "data",
                            Json::object([
                                ("files", Json::Array(files)),
                                ("merge", Json::Bool(false)),
                            ]),
                        ),
                    ]));
                }
                DownloadEvent::Uncaught(message) => self.effects.push(Json::object([
                    k("uncaught"),
                    ("message", Json::string(&message)),
                ])),
            }
        }
    }
}

fn run_main(scenario: &Json, fixture: &Fixture, tools: Option<&Tools>) -> Vec<Json> {
    let session = Arc::new(Mutex::new(Session {
        events: None,
        effects: Effects::new(scenario, fixture),
        dialogs: VecDeque::new(),
    }));
    *fixture.on_auth.lock().expect("on_auth") = Some(Box::new({
        let session = session.clone();
        move |attempt| {
            let mut session = session.lock().expect("session");
            session.drain();
            session.effects.push(attempt);
        }
    }));
    let target = {
        let session = session.clone();
        let account = optional_text(scenario.get("server").expect("server"), "account");
        let user = tools.map(|tools| tools.user.clone()).unwrap_or_default();
        let (port, key) = (fixture.port, fixture.key.clone());
        let ready_timeout_ms = timeout_ms(scenario);
        move |options: &ConnectOptions| {
            assert_eq!(options.port, SSH_PORT);
            assert_eq!(options.ready_timeout_ms, CONNECT_TIMEOUT_MS);
            let mut session = session.lock().expect("session");
            session.drain();
            session.effects.push(Json::object([
                k("attempt"),
                ("host", Json::string(&options.host)),
                ("username", Json::string(options.username)),
                ("password", Json::string(options.password)),
            ]));
            let login = if account.as_deref() == Some(options.username) {
                Login {
                    username: user.clone(),
                    password: None,
                    key: Some(key.clone()),
                }
            } else {
                Login {
                    username: options.username.to_owned(),
                    password: Some(options.password.to_owned()),
                    key: None,
                }
            };
            Target {
                host: options.host.clone(),
                port,
                ready_timeout_ms,
                login,
            }
        }
    };
    let (service, events) = DownloadService::spawn(DownloadConfig {
        style: PathStyle::host(),
        target: Box::new(target),
        delay: Box::new(|ms| ms / TIME_SCALE),
    })
    .expect("service");
    session.lock().expect("session").events = Some(events);
    service.window_opened();
    for step in array(scenario, "steps") {
        let op = text(step, "op");
        match op.as_str() {
            "start" => service.start(
                &host(scenario),
                &fixture.remote(&optional_text(scenario, "path").unwrap_or("logs".to_owned())),
            ),
            "await" => {
                let name = text(step, "name");
                let count = number(step, "count").map_or(1, |count| count as usize);
                let deadline = Instant::now() + LIMIT;
                loop {
                    let mut session = session.lock().expect("session");
                    session.drain();
                    if session.effects.count(&name) >= count {
                        break;
                    }
                    if Instant::now() > deadline {
                        session
                            .effects
                            .push(Json::object([k("timeout"), ("name", Json::string(&name))]));
                        return std::mem::take(&mut session.effects.list);
                    }
                    drop(session);
                    thread::sleep(Duration::from_millis(1));
                }
            }
            "settle" | "wait" => {
                thread::sleep(Duration::from_millis(number(step, "ms").expect("ms") as u64));
                session.lock().expect("session").drain();
            }
            "save" => service.save(
                array(step, "files")
                    .iter()
                    .map(|file| file.as_str().expect("a name").to_string_lossy())
                    .collect(),
            ),
            "answer" => {
                let token = session
                    .lock()
                    .expect("session")
                    .dialogs
                    .pop_front()
                    .expect("a pending dialog");
                match number(step, "response") {
                    Some(response) => service.open_log(token, response as usize),
                    None => {
                        let path = fixture.local(&text(step, "path"));
                        service.save_location(token, Some(path.display().to_string()));
                    }
                }
            }
            "drop" => fixture.drop_all(&text(step, "how")),
            "remote" => fixture.put(step),
            "removeRemote" => fixture.remove_remote(&text(step, "path")),
            "local" => fixture.put_local(
                &text(step, "path"),
                number(step, "size").expect("size") as usize,
            ),
            "close" => service.close(),
            other => panic!("unknown step {other}"),
        }
    }
    let mut session = session.lock().expect("session");
    let last = array(scenario, "steps").last().expect("a step");
    if text(last, "op") == "await" {
        let count = number(last, "count").map_or(1, |count| count as usize);
        session.effects.stop_at(&text(last, "name"), count);
    }
    session
        .effects
        .push(Json::object([k("saved"), ("files", fixture.saved())]));
    std::mem::take(&mut session.effects.list)
}

fn run_client(scenario: &Json, fixture: &Fixture, tools: Option<&Tools>) -> Vec<Json> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let mut effects = Effects::new(scenario, fixture);
    let account = scenario
        .get("server")
        .expect("server")
        .get("account")
        .is_some();
    let login = match tools {
        Some(tools) if account => Login {
            username: tools.user.clone(),
            password: None,
            key: Some(fixture.key.clone()),
        },
        _ => Login {
            username: "systemcore".to_owned(),
            password: Some("systemcore".to_owned()),
            key: None,
        },
    };
    let end_on_error = text(scenario, "onError") == "end";
    runtime.block_on(async {
        let (network, mut events) = tokio::sync::mpsc::unbounded_channel();
        let sender = network.clone();
        let mut clients = Clients::new(move |client, event| {
            let _ = sender.send((client, event));
        });
        let mut client = 0;
        let mut ready_timeout: Option<tokio::time::Instant> = None;
        for step in array(scenario, "steps") {
            let (wanted, deadline) = match text(step, "op").as_str() {
                "start" => {
                    client = clients.connect(Target {
                        host: host(scenario),
                        port: fixture.port,
                        ready_timeout_ms: timeout_ms(scenario),
                        login: login.clone(),
                    });
                    continue;
                }
                "end" => {
                    clients.end(client);
                    continue;
                }
                "drop" => {
                    fixture.drop_all(&text(step, "how"));
                    continue;
                }
                "stall" => {
                    fixture.drop_all("stall");
                    continue;
                }
                "sftp" => {
                    if let Err(message) = clients.sftp(client) {
                        let message = effects.text(&message);
                        effects.push(Json::object([k("sftp"), ("threw", message)]));
                    }
                    continue;
                }
                "await" => (Some(text(step, "name")), LIMIT),
                "settle" | "wait" => (
                    None,
                    Duration::from_millis(number(step, "ms").expect("ms") as u64),
                ),
                other => panic!("unknown step {other}"),
            };
            let deadline = tokio::time::Instant::now() + deadline;
            loop {
                while let Some(event) = clients.next_event() {
                    match event {
                        ClientEvent::Error(id, message) => {
                            let message = effects.text(&message);
                            effects.push(Json::object([k("error"), ("message", message)]));
                            if end_on_error {
                                clients.end(id);
                            }
                        }
                        ClientEvent::Ready(_) => effects.push(Json::object([k("ready")])),
                        ClientEvent::Sftp(_, _, result) => {
                            let error = match result {
                                Ok(_) => Json::Null,
                                Err(message) => effects.text(&message),
                            };
                            effects.push(Json::object([k("sftp"), ("error", error)]));
                        }
                        ClientEvent::Close(_) => effects.push(Json::object([k("close")])),
                        ClientEvent::StartTimeout(_, ms) => {
                            ready_timeout = Some(
                                tokio::time::Instant::now() + Duration::from_secs_f64(ms / 1000.0),
                            );
                        }
                        ClientEvent::ClearTimeout(_) => ready_timeout = None,
                        ClientEvent::Resolve(id, name) => {
                            let network = network.clone();
                            tokio::spawn(async move {
                                let _ = network.send((id, lookup(&name).await));
                            });
                        }
                        other => panic!("unexpected {other:?}"),
                    }
                }
                if wanted.as_ref().is_some_and(|name| effects.count(name) > 0) {
                    break;
                }
                let ready_at = ready_timeout.unwrap_or(deadline);
                tokio::select! {
                    Some((id, event)) = events.recv() => clients.received(id, event),
                    () = tokio::time::sleep_until(ready_at), if ready_timeout.is_some() => {
                        ready_timeout = None;
                        clients.timed_out(client);
                    }
                    () = tokio::time::sleep_until(deadline) => {
                        if let Some(name) = &wanted {
                            effects.push(Json::object([k("timeout"), ("name", Json::string(name))]));
                            return;
                        }
                        break;
                    }
                }
            }
        }
    });
    effects.list
}

fn describe(effects: &[Json]) -> String {
    effects
        .iter()
        .map(|effect| format!("    {}", effect.to_compact()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_all(wanted: impl Fn(&Json) -> bool, tools: Option<Tools>, skippable: &[&str]) {
    let spec = scenarios();
    let recorded: HashMap<String, Json> = load(VECTOR)
        .expect("vector")
        .iter()
        .map(|record| {
            (
                record.string("scenario").expect("scenario"),
                record.field("effects").expect("effects").clone(),
            )
        })
        .collect();
    let all = array(&spec, "scenarios");
    assert_eq!(all.len(), recorded.len(), "recorded scenarios");
    let filter = std::env::var("ASRS_DOWNLOAD_SCENARIO").ok();
    let selected: Vec<&Json> = all
        .iter()
        .filter(|s| wanted(s))
        .filter(|s| {
            filter
                .as_ref()
                .is_none_or(|f| text(s, "name").contains(f.as_str()))
        })
        .collect();
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let skipped = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                while let Some(scenario) = selected.get(next.fetch_add(1, Ordering::SeqCst)) {
                    let name = text(scenario, "name");
                    if kind(scenario) == "blackhole" && !blackhole_works(&host(scenario)) {
                        let reason = "the network answers for TEST-NET";
                        skipped
                            .lock()
                            .expect("skipped")
                            .push((name, reason.to_owned()));
                        continue;
                    }
                    let fixture = match Fixture::start(&spec, scenario, tools.as_ref()) {
                        Ok(fixture) => fixture,
                        Err(reason) => {
                            skipped.lock().expect("skipped").push((name, reason));
                            continue;
                        }
                    };
                    let actual = if is_client(scenario) {
                        run_client(scenario, &fixture, tools.as_ref())
                    } else {
                        run_main(scenario, &fixture, tools.as_ref())
                    };
                    let expected = &recorded[&name];
                    if expected.to_compact() != Json::Array(actual.clone()).to_compact() {
                        failures.lock().expect("failures").push(format!(
                            "{name}\n  expected\n{}\n  actual\n{}",
                            describe(expected.as_array().expect("effects")),
                            describe(&actual)
                        ));
                    }
                }
            });
        }
    });
    let skipped = skipped.into_inner().expect("skipped");
    for (name, reason) in &skipped {
        eprintln!("download_live: {name} skipped: {reason}");
        assert!(
            skippable.iter().any(|prefix| name.contains(prefix)),
            "{name} must not be skipped: {reason}"
        );
    }
    let failures = failures.into_inner().expect("failures");
    assert!(
        failures.is_empty(),
        "{} of {} scenarios differ\n{}",
        failures.len(),
        selected.len(),
        failures.join("\n")
    );
    eprintln!(
        "download_live: {} scenarios equal, {} skipped",
        selected.len() - skipped.len(),
        skipped.len()
    );
}

#[test]
fn scripted_servers() {
    run_all(
        |s| kind(s) != "sshd",
        None,
        &["blackhole", "unroutable", "ipv6"],
    );
}

#[test]
fn local_sshd() {
    match tools() {
        Ok(tools) => run_all(
            |s| kind(s) == "sshd",
            Some(tools),
            &["no-", "kex-", "cipher-", "mac-"],
        ),
        Err(reason) => eprintln!("download_live: sshd scenarios skipped: {reason}"),
    }
}
