//! The fixture of `files/download-live`: the same scenario file, remote tree, sshd configuration,
//! byte-counting proxy, raw servers and password server as `oracle/src/download/live.ts`.

use std::fs;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream as StdStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asrs_parity::Json;
use asrs_parity::vectors::workspace_root;
use russh::keys::{Algorithm, PrivateKey, key::safe_rng};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use sha2::{Digest, Sha256};
use socket2::{Domain, Socket, Type};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::sync::watch;

pub fn scenarios() -> Json {
    let path = workspace_root().join("parity/specs/files/download-live.scenarios.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    asrs_parity::json::parse(&text).expect("scenario file")
}

pub fn text(json: &Json, key: &str) -> String {
    optional_text(json, key).unwrap_or_else(|| panic!("no {key:?} in {}", json.to_compact()))
}

pub fn optional_text(json: &Json, key: &str) -> Option<String> {
    Some(json.get(key)?.as_str()?.to_string_lossy())
}

pub fn number(json: &Json, key: &str) -> Option<f64> {
    json.get(key)?.as_f64()
}

pub fn array<'a>(json: &'a Json, key: &str) -> &'a [Json] {
    json.get(key).and_then(Json::as_array).unwrap_or_default()
}

fn file_bytes(size: usize) -> Vec<u8> {
    (0..size)
        .map(|i| (i.wrapping_mul(31) + (i >> 8) * 7 + (i >> 16)) as u8)
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn first_existing(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
}

pub struct Tools {
    sshd: PathBuf,
    sftp_server: PathBuf,
    pub user: String,
}

/// What an sshd scenario needs, or why this machine cannot run one.
pub fn tools() -> Result<Tools, String> {
    if !cfg!(unix) {
        return Err("no unprivileged sshd on this platform".to_owned());
    }
    let sshd = first_existing(&[
        "/usr/sbin/sshd",
        "/usr/local/sbin/sshd",
        "/opt/homebrew/sbin/sshd",
    ])
    .ok_or("no sshd")?;
    let sftp_server = first_existing(&[
        "/usr/libexec/sftp-server",
        "/usr/lib/openssh/sftp-server",
        "/usr/libexec/openssh/sftp-server",
        "/usr/lib/ssh/sftp-server",
    ])
    .ok_or("no sftp-server")?;
    let id = |flag: &str| {
        let output = Command::new("id")
            .arg(flag)
            .output()
            .map_err(|e| format!("id {flag}: {e}"))?;
        Ok::<String, String>(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    if id("-u")? == "0" {
        return Err(
            "running as root, which every file permission scenario would ignore".to_owned(),
        );
    }
    Command::new("ssh-keygen")
        .arg("-?")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ssh-keygen: {e}"))?;
    Ok(Tools {
        sshd,
        sftp_server,
        user: id("-un")?,
    })
}

type OnAuth = Arc<Mutex<Option<Box<dyn Fn(Json) + Send>>>>;

pub struct Fixture {
    pub root: PathBuf,
    pub port: u16,
    pub key: String,
    pub on_auth: OnAuth,
    partial: bool,
    renames: AtomicU32,
    /// Bound and never listening, so that nothing else can take the port.
    bound: Option<Socket>,
    sshd: Option<Child>,
    drops: watch::Sender<Option<&'static str>>,
    runtime: Option<Runtime>,
}

#[derive(Clone)]
struct DropRule {
    bytes: usize,
    how: &'static str,
}

fn how(name: &str) -> &'static str {
    match name {
        "fin" => "fin",
        "rst" => "rst",
        "stall" => "stall",
        other => panic!("unknown drop {other}"),
    }
}

impl Fixture {
    /// `Err` when this machine cannot run the scenario.
    pub fn start(spec: &Json, scenario: &Json, tools: Option<&Tools>) -> Result<Fixture, String> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "asrs-sshd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let root = std::env::temp_dir().join(name);
        fs::create_dir_all(root.join("local")).expect("fixture folder");
        let root = fs::canonicalize(&root).expect("fixture folder");
        let mut fixture = Fixture {
            root,
            port: 0,
            key: String::new(),
            on_auth: OnAuth::default(),
            partial: matches!(scenario.get("partial"), Some(Json::Bool(true))),
            renames: AtomicU32::new(0),
            bound: None,
            sshd: None,
            drops: watch::channel(None).0,
            runtime: None,
        };
        let server = scenario.get("server").expect("server");
        match text(server, "kind").as_str() {
            "closed" => {
                let ipv6 = optional_text(scenario, "host").is_some_and(|host| host == "::1");
                let address = if ipv6 {
                    SocketAddr::from((Ipv6Addr::LOCALHOST, 0))
                } else {
                    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
                };
                let socket = Socket::new(Domain::for_address(address), Type::STREAM, None)
                    .and_then(|socket| socket.bind(&address.into()).map(|()| socket))
                    .map_err(|e| format!("no loopback: {e}"))?;
                let bound = socket.local_addr().expect("address").as_socket();
                fixture.port = bound.expect("an IP address").port();
                fixture.bound = Some(socket);
            }
            "unroutable" if !cfg!(target_os = "macos") => {
                return Err("recorded with macOS's error code".to_owned());
            }
            "blackhole" | "unroutable" => fixture.port = 22,
            "raw" => {
                let send = text(server, "send").chars().map(|c| c as u8).collect();
                let (then, when) = (text(server, "then"), text(server, "when"));
                fixture.listen(move |socket, _| {
                    raw(socket, Vec::clone(&send), then.clone(), when.clone())
                });
            }
            "password" => {
                let key = PrivateKey::random(&mut safe_rng(), Algorithm::Ed25519).expect("key");
                let config = Arc::new(server::Config {
                    keys: vec![key],
                    auth_rejection_time: Duration::ZERO,
                    ..server::Config::default()
                });
                let handler = PasswordServer {
                    accounts: match server.get("accounts") {
                        Some(Json::Object(accounts)) => accounts
                            .iter()
                            .map(|(name, password)| {
                                let password = password.as_str().expect("a password");
                                (name.to_string_lossy(), password.to_string_lossy())
                            })
                            .collect(),
                        other => panic!("accounts are {other:?}"),
                    },
                    none: matches!(server.get("none"), Some(Json::Bool(true))),
                    on_auth: fixture.on_auth.clone(),
                    received: Vec::new(),
                };
                fixture.listen(move |socket, _| {
                    let (config, handler) = (config.clone(), handler.clone());
                    async move {
                        if let Ok(session) = server::run_stream(config, socket, handler).await {
                            let _ = session.await;
                        }
                    }
                });
            }
            "sshd" => {
                for entry in array(spec, "tree") {
                    fixture.put(entry);
                }
                let tools = tools.expect("sshd scenarios run only with the tools");
                let port = fixture.start_sshd(server, tools)?;
                let rule = server.get("drop").map(|rule| DropRule {
                    bytes: number(rule, "bytes").expect("bytes") as usize,
                    how: how(&text(rule, "how")),
                });
                fixture.listen(move |socket, drops| proxy(socket, port, rule.clone(), drops));
            }
            other => panic!("unknown server {other}"),
        }
        Ok(fixture)
    }

    fn listen<F>(
        &mut self,
        serve: impl Fn(TcpStream, watch::Receiver<Option<&'static str>>) -> F + Send + 'static,
    ) where
        F: Future<Output = ()> + Send + 'static,
    {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let listener = runtime
            .block_on(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))
            .expect("bind");
        self.port = listener.local_addr().expect("address").port();
        let drops = self.drops.subscribe();
        runtime.spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let mut drops = drops.clone();
                drops.mark_unchanged();
                tokio::spawn(serve(socket, drops));
            }
        });
        self.runtime = Some(runtime);
    }

    fn start_sshd(&mut self, server: &Json, tools: &Tools) -> Result<u16, String> {
        let file = |name: &str| self.root.join(name);
        let keygen = |kind: &str, name: &str| {
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", kind, "-N", "", "-C", "asrs-fixture", "-f"])
                .arg(file(name))
                .status()
                .expect("ssh-keygen");
            assert!(status.success(), "ssh-keygen -t {kind}");
        };
        keygen("ed25519", "user");
        keygen(
            &optional_text(server, "hostKey").unwrap_or("ed25519".to_owned()),
            "host",
        );
        self.key = fs::read_to_string(file("user")).expect("user key");
        fs::copy(file("user.pub"), file("authorized")).expect("authorized key");
        let sftp_server = tools.sftp_server.display().to_string();
        let subsystem = match server.get("subsystem") {
            None => Some(sftp_server.clone()),
            Some(Json::Null) => None,
            Some(_) => Some(text(server, "subsystem").replace("SFTP_SERVER", &sftp_server)),
        };
        let mut lines = vec![
            "ListenAddress 127.0.0.1".to_owned(),
            format!("HostKey {}", file("host").display()),
            "PidFile none".to_owned(),
            format!("AuthorizedKeysFile {}", file("authorized").display()),
            "StrictModes no".to_owned(),
            "UsePAM no".to_owned(),
            "PasswordAuthentication yes".to_owned(),
            "KbdInteractiveAuthentication no".to_owned(),
        ];
        lines.extend(subsystem.map(|command| format!("Subsystem sftp {command}")));
        lines.extend(
            array(server, "options")
                .iter()
                .map(|option| option.as_str().expect("an option").to_string_lossy()),
        );
        let config = file("sshd_config");
        let check = |lines: &[String]| {
            fs::write(&config, lines.join("\n") + "\n").expect("sshd_config");
            let output = Command::new(&tools.sshd)
                .args(["-t", "-f"])
                .arg(&config)
                .output()
                .expect("sshd -t");
            if output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).into_owned())
            }
        };
        let mut penalties = lines.clone();
        penalties.push("PerSourcePenalties no".to_owned());
        check(&penalties).or_else(|_| check(&lines))?;
        let port = (0..20).find_map(|_| self.spawn_sshd(&config, tools));
        Ok(port.unwrap_or_else(|| {
            let log = fs::read_to_string(file("sshd.log")).unwrap_or_default();
            panic!("sshd did not start: {log}")
        }))
    }

    /// `None` if sshd exited: another process may take the port before sshd does.
    fn spawn_sshd(&mut self, config: &Path, tools: &Tools) -> Option<u16> {
        let port = {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
            listener.local_addr().expect("address").port()
        };
        let log = self.root.join("sshd.log");
        let mut sshd = Command::new(&tools.sshd)
            .args(["-D", "-e", "-f"])
            .arg(config)
            .args(["-p", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).expect("sshd.log"))
            .spawn()
            .expect("sshd");
        let listening = format!("Server listening on 127.0.0.1 port {port}.");
        while sshd.try_wait().expect("sshd").is_none() {
            if fs::read_to_string(&log).is_ok_and(|log| log.contains(&listening)) {
                self.sshd = Some(sshd);
                return Some(port);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    pub fn remote(&self, path: &str) -> String {
        format!("{}/remote/{path}", self.root.display())
    }

    pub fn local(&self, path: &str) -> PathBuf {
        match path {
            "." => self.root.join("local"),
            path => self.root.join("local").join(path),
        }
    }

    pub fn put(&self, entry: &Json) {
        let full = self.root.join("remote").join(text(entry, "path"));
        fs::create_dir_all(full.parent().expect("parent")).expect("remote folder");
        if entry.get("dir").is_some() {
            fs::create_dir_all(&full).expect("remote folder");
        } else if let Some(link) = optional_text(entry, "link") {
            symlink(&link, &full);
        } else {
            let size = number(entry, "size").expect("size") as usize;
            fs::write(&full, file_bytes(size)).expect("remote file");
        }
        if let Some(mode) = number(entry, "mode") {
            set_mode(&full, mode as u32);
        }
    }

    pub fn put_at_once(&self, entry: &Json) {
        let full = self.root.join("remote").join(text(entry, "path"));
        let top = full
            .ancestors()
            .take_while(|path| !path.exists())
            .last()
            .expect("a new path");
        let staged = self.unused("staged");
        let file = match full.strip_prefix(top).expect("an ancestor") {
            rest if rest.as_os_str().is_empty() => staged.clone(),
            rest => staged.join(rest),
        };
        let size = number(entry, "size").expect("size") as usize;
        fs::create_dir_all(file.parent().expect("parent")).expect("staged folder");
        fs::write(&file, file_bytes(size)).expect("staged file");
        fs::rename(&staged, top).expect("remote path");
    }

    pub fn remove_at_once(&self, path: &str) {
        let full = self.root.join("remote").join(path);
        fs::rename(full, self.unused("removed")).expect("remove remote path");
    }

    fn unused(&self, kind: &str) -> PathBuf {
        let n = self.renames.fetch_add(1, Ordering::SeqCst);
        self.root.join(format!("{kind}-{n}"))
    }

    pub fn put_local(&self, path: &str, size: usize) {
        fs::write(self.local(path), file_bytes(size)).expect("local file");
    }

    /// Drops every proxied connection.
    pub fn drop_all(&self, name: &str) {
        let _ = self.drops.send(Some(how(name)));
    }

    pub fn saved(&self) -> Json {
        let mut files = Vec::new();
        self.visit(&self.local("."), "", &mut files);
        let files: Vec<(&str, Json)> = files.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        Json::object(files)
    }

    fn visit(&self, dir: &Path, prefix: &str, files: &mut Vec<(String, Json)>) {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("local folder")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        for name in names {
            let full = dir.join(&name);
            if full.is_dir() {
                files.push((format!("{prefix}{name}/"), Json::Object(Vec::new())));
                self.visit(&full, &format!("{prefix}{name}/"), files);
            } else if self.partial {
                files.push((format!("{prefix}{name}"), Json::Object(Vec::new())));
            } else {
                let data = fs::read(&full).expect("saved file");
                let described = Json::object([
                    ("size", Json::Number(data.len() as f64)),
                    ("sha256", Json::string(&hex(&Sha256::digest(&data)[..8]))),
                ]);
                files.push((format!("{prefix}{name}"), described));
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
        if let Some(mut sshd) = self.sshd.take() {
            let _ = sshd.kill();
            let _ = sshd.wait();
        }
        unlock(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone)]
struct PasswordServer {
    accounts: Vec<(String, String)>,
    none: bool,
    on_auth: OnAuth,
    received: Vec<u8>,
}

impl PasswordServer {
    fn attempt(&self, method: &str, user: &str, password: Option<&str>, accepted: bool) -> Auth {
        let mut attempt = vec![
            ("k", Json::string("auth")),
            ("method", Json::string(method)),
            ("username", Json::string(user)),
        ];
        attempt.extend(password.map(|password| ("password", Json::string(password))));
        let on_auth = self.on_auth.lock().expect("on_auth");
        on_auth.as_ref().expect("a listener")(Json::object(attempt));
        if accepted {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::Password][..])),
                partial_success: false,
            }
        }
    }
}

impl server::Handler for PasswordServer {
    type Error = russh::Error;

    async fn auth_none(&mut self, user: &str) -> Result<Auth, russh::Error> {
        Ok(self.attempt("none", user, None, self.none))
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, russh::Error> {
        let account = (user.to_owned(), password.to_owned());
        let accepted = self.accounts.contains(&account);
        Ok(self.attempt("password", user, Some(password), accepted))
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), russh::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        assert_eq!(name, "sftp");
        session.channel_success(channel)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        const INIT: u8 = 1;
        const OPENDIR: u8 = 11;
        self.received.extend_from_slice(data);
        while let Some(length) = self.received.first_chunk::<4>() {
            let end = 4 + u32::from_be_bytes(*length) as usize;
            if self.received.len() < end {
                break;
            }
            let packet: Vec<u8> = self.received.drain(..end).collect();
            let reply = match packet[4] {
                INIT => vec![0, 0, 0, 5, 2, 0, 0, 0, 3],
                OPENDIR => {
                    let mut body = vec![101];
                    body.extend_from_slice(&packet[5..9]);
                    body.extend_from_slice(b"\0\0\0\x02\0\0\0\x0cNo such file\0\0\0\0");
                    let mut reply = (body.len() as u32).to_be_bytes().to_vec();
                    reply.extend(body);
                    reply
                }
                other => panic!("unexpected request {other}"),
            };
            session.data(channel, reply)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn symlink(target: &str, path: &Path) {
    std::os::unix::fs::symlink(target, path).expect("symlink");
}

#[cfg(not(unix))]
fn symlink(_: &str, _: &Path) {
    unreachable!("sshd scenarios need unix");
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("mode");
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {
    unreachable!("sshd scenarios need unix");
}

fn unlock(dir: &Path) {
    #[cfg(unix)]
    set_mode(dir, 0o700);
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            unlock(&entry.path());
        }
    }
}

/// Whether the TEST-NET address of a blackhole scenario really never answers here.
pub fn blackhole_works(host: &str) -> bool {
    let address = format!("{host}:22").parse().expect("an address");
    matches!(
        StdStream::connect_timeout(&address, Duration::from_millis(500)),
        Err(e) if e.kind() == io::ErrorKind::TimedOut
    )
}

fn reset(socket: TcpStream) {
    socket.set_zero_linger().expect("linger");
}

async fn raw(mut socket: TcpStream, send: Vec<u8>, then: String, when: String) {
    if !send.is_empty() && socket.write_all(&send).await.is_err() {
        return;
    }
    let mut acted = false;
    let mut received = Vec::new();
    loop {
        let ident = received
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|i| i + 2);
        let ready = match when.as_str() {
            "accept" => true,
            "data" => ident.is_some(),
            "kexinit" => ident.is_some_and(|start| {
                received[start..].first_chunk::<4>().is_some_and(|len| {
                    received.len() - start >= 4 + u32::from_be_bytes(*len) as usize
                })
            }),
            other => panic!("unknown when {other}"),
        };
        if ready && !acted {
            acted = true;
            match then.as_str() {
                "fin" => {
                    let _ = socket.shutdown().await;
                }
                "rst" => {
                    reset(socket);
                    return;
                }
                "hold" | "linger" => {}
                other => panic!("unknown then {other}"),
            }
        }
        let mut buffer = [0; 4096];
        match socket.read(&mut buffer).await {
            Ok(n) if n > 0 => received.extend_from_slice(&buffer[..n]),
            Ok(_) if then == "linger" => std::future::pending().await,
            _ => return,
        }
    }
}

async fn proxy(
    mut client: TcpStream,
    port: u16,
    rule: Option<DropRule>,
    mut drops: watch::Receiver<Option<&'static str>>,
) {
    let Ok(mut server) = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await else {
        return;
    };
    let mut sent = 0;
    let mut from_client = vec![0; 65536];
    let mut from_server = vec![0; 65536];
    let (mut client_open, mut server_open) = (true, true);
    let mut stalled = false;
    let how = loop {
        if !client_open && !server_open {
            return;
        }
        tokio::select! {
            result = client.read(&mut from_client), if client_open => match result {
                Ok(0) => {
                    client_open = false;
                    let _ = server.shutdown().await;
                }
                Ok(n) => if server.write_all(&from_client[..n]).await.is_err() { return },
                Err(_) => return,
            },
            result = server.read(&mut from_server), if server_open => match result {
                Ok(0) | Err(_) => {
                    server_open = false;
                    let _ = client.shutdown().await;
                }
                Ok(n) => {
                    if stalled {
                        continue;
                    }
                    let left = rule.as_ref().map_or(usize::MAX, |rule| rule.bytes - sent);
                    if client.write_all(&from_server[..n.min(left)]).await.is_err() {
                        return;
                    }
                    if let Some(rule) = &rule {
                        sent += n.min(left);
                        if sent >= rule.bytes {
                            if rule.how != "stall" {
                                break rule.how;
                            }
                            stalled = true;
                        }
                    }
                }
            },
            result = drops.changed() => {
                let how = *drops.borrow_and_update();
                match (result, how) {
                    (Ok(()), Some("stall")) => stalled = true,
                    (Ok(()), Some(how)) => break how,
                    (Ok(()), None) => {}
                    (Err(_), _) => return,
                }
            }
        }
    };
    drop(server);
    if how == "rst" {
        reset(client);
        return;
    }
    let _ = client.shutdown().await;
    while matches!(client.read(&mut from_client).await, Ok(n) if n > 0) {}
}
