use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use asrs_parity::json::{self, Json};

use crate::oracle::{self, Paths};
use crate::{Result, canvas, pack};

const USAGE: &str = r"usage: cargo xtask host <command>

Render hosts for the Windows and Linux parity track. Both are VMs on the dev machine and are
driven over SSH; neither has GPU acceleration, so Chromium and native code render in software
(`cargo xtask host local describe` records which rasterizers are in use).

  windows   Windows 11 Pro ARM64 in UTM (`Win11-ARM`), files in /Volumes/T9/vms/win11-arm
            (override with ASRS_WINDOWS_VM). SSH dev@127.0.0.1:2222 with keys/id_ed25519.
            Checkout C:\work\asrs, oracle pack C:\work\asrs-pack.tar.zst. GUI programs run in
            the logged-in desktop session through a scheduled task.
  linux     Ubuntu 24.04 ARM64 tart VM `asrs-linux` under TART_HOME=/Volumes/T9/tart, Xorg on
            virtio-gpu with an autologin xfce session on :0. SSH admin@$(tart ip asrs-linux) with
            /Volumes/T9/asrs/hosts/asrs-linux/id_ed25519 (override with ASRS_LINUX_KEYS).
            Checkout ~/asrs, oracle pack ~/asrs-pack.tar.zst.

The dev machine has 16 GB of memory; the Windows VM takes 8 GB and the Linux VM 6 GB (plus 8 GB
of swap inside it), so avoid heavy builds on both at once. The Windows VM is shared with other
projects: do not stop it while someone else is using it.

commands on a host (cargo xtask host <name> ...):
  start | stop | status   boot the VM and wait for SSH, shut it down, or report its state
  ssh [<command>...]      interactive shell, or one remote command
  provision               run xtask/hosts/<name>/provision.* there (packages, display, theme,
                          fonts, toolchains); idempotent
  sync                    copy the checkout (tracked plus untracked files that are not ignored),
                          remove files deleted since the last sync, and upload the oracle pack
                          (`cargo xtask oracle pack` first when it is missing or older than the
                          oracle install)
  setup                   `cargo xtask oracle setup --from-pack` there
  run <command>...        run a command in the checkout over SSH
  gui <command>...        run a command in the checkout inside the desktop session and wait
  fetch [<dir>]           copy target/parity[/<dir>] from the host into target/parity/<name>/
  theme light|dark        switch the desktop theme
  screenshot <file>       PNG of the host display
  manifest                store the fetched host description (target/parity/<name>/host/host.json)
                          as `host` in parity/vectors/canvas/<os>.json
  check                   the X01 exit criteria: sync, setup (selftest and smoke), selftest,
                          smoke, `oracle gen --check` (in the desktop session: the proto-renderer
                          vector opens a BrowserWindow), a workspace build, `host local
                          gui-check` (host description, colour patches at 1x and 2x, real app
                          launch), the S1 spike window, then fetch target/parity/host

commands on the machine itself (cargo xtask host local ...):
  describe [--manifest]   write target/parity/host/host.json: OS, Electron, displays, theme,
                          Chromium and native rasterizers, sha256 of the UI font files. With
                          --manifest also store it as `host` in parity/vectors/canvas/<os>.json
  colour-patch            512 sRGB patches through capturePage at device scale 1 and 2, with
                          and without --force-color-profile=srgb. Gate on Windows and Linux,
                          whose display profile is pinned to sRGB: every patch exact in both
                          runs. On macOS: within 1 level per channel with the switch; the run
                          without it is recorded only
  launch                  start the real upstream app with isolated userData and capture its hub
  gui-check               describe, colour-patch and launch
  s1                      run the S1 spike (`auto`) and require that its window opened

Results land in target/parity/host/ on the machine that ran them.";

const WINDOWS_NODE: &str = r"C:\tools\node-v24.18.0-win-arm64";
const GUI_DIR: &str = "C:/work/.gui";
const SYNC_DIR: &str = "C:/work/.sync";
const LINUX_VM: &str = "asrs-linux";
const COLOUR_SWITCHES: &[&str] = &["--force-color-profile=srgb", "--lang=en-US"];
const S1_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Os {
    Windows,
    Linux,
}

struct Host {
    name: &'static str,
    os: Os,
}

const HOSTS: [Host; 2] = [
    Host {
        name: "windows",
        os: Os::Windows,
    },
    Host {
        name: "linux",
        os: Os::Linux,
    },
];

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    match args {
        [local, rest @ ..] if local == "local" => run_local(&paths, rest),
        [name, command, rest @ ..] => {
            let host = HOSTS
                .iter()
                .find(|h| h.name == name)
                .ok_or_else(|| format!("unknown host {name:?}\n\n{USAGE}"))?;
            run_remote(&paths, host, command, rest)
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn run_remote(paths: &Paths, host: &Host, command: &str, rest: &[String]) -> Result {
    let joined = rest.join(" ");
    match (command, rest) {
        ("start", []) => host.start(),
        ("stop", []) => host.stop(),
        ("status", []) => host.status(),
        ("ssh", _) => host.ssh_interactive(&joined),
        ("provision", []) => host.provision(paths),
        ("sync", []) => host.sync(paths),
        ("setup", []) => host.run(&host.setup_command()),
        ("run", [_, ..]) => host.run(&joined),
        ("gui", [_, ..]) => host.gui(&joined),
        ("fetch", []) => host.fetch(paths, ""),
        ("fetch", [dir]) => host.fetch(paths, dir),
        ("theme", [theme]) if theme == "light" || theme == "dark" => host.theme(theme == "dark"),
        ("screenshot", [file]) => host.screenshot(Path::new(file)),
        ("check", []) => host.check(paths),
        ("manifest", []) => host.record_manifest(paths),
        _ => Err(USAGE.to_owned()),
    }
}

impl Host {
    fn ssh_target(&self) -> Result<(Vec<String>, String)> {
        let (keys, port, destination) = match self.os {
            Os::Windows => (
                windows_vm().join("keys"),
                "2222",
                "dev@127.0.0.1".to_owned(),
            ),
            Os::Linux => (linux_keys(), "22", format!("admin@{}", linux_ip()?)),
        };
        let options = [
            "-i".to_owned(),
            keys.join("id_ed25519").display().to_string(),
            "-p".to_owned(),
            port.to_owned(),
            "-o".to_owned(),
            format!("UserKnownHostsFile={}", keys.join("known_hosts").display()),
        ]
        .into_iter()
        .chain(
            [
                "IdentitiesOnly=yes",
                "StrictHostKeyChecking=accept-new",
                "ConnectTimeout=10",
                "ServerAliveInterval=30",
                "LogLevel=ERROR",
            ]
            .iter()
            .flat_map(|o| ["-o".to_owned(), (*o).to_owned()]),
        )
        .collect();
        Ok((options, destination))
    }

    fn ssh(&self, remote: &str) -> Result<Command> {
        let (options, destination) = self.ssh_target()?;
        let mut command = Command::new("ssh");
        command
            .args(options)
            .args(["-o", "BatchMode=yes"])
            .arg(destination)
            .arg(remote);
        Ok(command)
    }

    fn scp(&self, from: &str, to: &str) -> Result {
        let (options, destination) = self.ssh_target()?;
        let options: Vec<String> = options
            .into_iter()
            .map(|o| if o == "-p" { "-P".to_owned() } else { o })
            .collect();
        let remote = |p: &str| match p.strip_prefix(':') {
            Some(path) => format!("{destination}:{path}"),
            None => p.to_owned(),
        };
        exec(
            Command::new("scp")
                .arg("-q")
                .args(options)
                .args(["-o", "BatchMode=yes"])
                .arg(remote(from))
                .arg(remote(to)),
        )
    }

    fn remote_script(&self, body: &str) -> String {
        match self.os {
            Os::Linux => format!("bash -lc {}", sh_quote(body)),
            Os::Windows => format!(
                "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -OutputFormat Text -EncodedCommand {}",
                powershell_encode(&format!("$ProgressPreference = 'SilentlyContinue'\n{body}"))
            ),
        }
    }

    fn script(&self, body: &str) -> Result {
        exec(&mut self.ssh(&self.remote_script(body))?)
    }

    fn script_output(&self, body: &str) -> Result<Vec<u8>> {
        let output = self
            .ssh(&self.remote_script(body))?
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| format!("ssh: {e}"))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(format!(
                "{}: remote script failed ({})",
                self.name, output.status
            ))
        }
    }

    fn checkout(&self) -> &'static str {
        match self.os {
            Os::Windows => "C:/work/asrs",
            Os::Linux => "$HOME/asrs",
        }
    }

    fn pack_path(&self) -> &'static str {
        match self.os {
            Os::Windows => "C:/work/asrs-pack.tar.zst",
            Os::Linux => "asrs-pack.tar.zst",
        }
    }

    fn setup_command(&self) -> String {
        match self.os {
            Os::Windows => format!("cargo xtask oracle setup --from-pack {}", self.pack_path()),
            Os::Linux => format!(
                "cargo xtask oracle setup --from-pack $HOME/{}",
                self.pack_path()
            ),
        }
    }

    fn run(&self, command: &str) -> Result {
        eprintln!("[{}] $ {command}", self.name);
        let body = match self.os {
            Os::Linux => format!("cd {} && {command}", self.checkout()),
            Os::Windows => format!(
                "$ErrorActionPreference = 'Continue'\n$env:Path = '{WINDOWS_NODE};' + $env:Path\nSet-Location {}\n& {{ {command} }} | Out-String -Stream\nexit $LASTEXITCODE",
                self.checkout()
            ),
        };
        self.script(&body)
            .map_err(|_| format!("{}: `{command}` failed", self.name))
    }

    fn gui(&self, command: &str) -> Result {
        eprintln!("[{} desktop] $ {command}", self.name);
        let body = match self.os {
            Os::Linux => format!(
                "export DISPLAY=:0 XAUTHORITY=$HOME/.Xauthority DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus\ncd {} && {command}",
                self.checkout()
            ),
            Os::Windows => {
                let gui = GUI_DIR.replace('/', "\\");
                let batch = format!(
                    "@echo off\r\nset PATH={WINDOWS_NODE};%PATH%\r\ncd /d {}\r\n{command} > {gui}\\last.log 2>&1\r\necho %ERRORLEVEL% > {gui}\\exit.txt\r\n",
                    self.checkout().replace('/', "\\")
                );
                format!(
                    r#"$ErrorActionPreference = 'Stop'
New-Item -Force -ItemType Directory {GUI_DIR} | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue {GUI_DIR}/exit.txt, {GUI_DIR}/last.log
Set-Content -NoNewline -Encoding ascii {GUI_DIR}/run.cmd @'
{batch}
'@
$action = New-ScheduledTaskAction -Execute "{gui}\run.cmd" -WorkingDirectory "{gui}"
$principal = New-ScheduledTaskPrincipal -UserId dev -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Hours 4) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName asrs-gui -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName asrs-gui
$idle = 0
while (-not (Test-Path {GUI_DIR}/exit.txt)) {{
    Start-Sleep -Milliseconds 500
    if ((Get-ScheduledTask -TaskName asrs-gui).State -eq 'Ready') {{ $idle++ }} else {{ $idle = 0 }}
    if ($idle -ge 20 -and -not (Test-Path {GUI_DIR}/exit.txt)) {{
        Get-Content -ErrorAction SilentlyContinue {GUI_DIR}/last.log
        [Console]::Error.WriteLine('the desktop task stopped without an exit code (is dev logged in to the desktop?)')
        exit 1
    }}
}}
Start-Sleep -Milliseconds 200
Get-Content {GUI_DIR}/last.log
exit [int](Get-Content {GUI_DIR}/exit.txt).Trim()"#
                )
            }
        };
        self.script(&body)
            .map_err(|_| format!("{}: `{command}` failed in the desktop session", self.name))
    }

    fn start(&self) -> Result {
        match self.os {
            Os::Windows => {
                vm_sh(&["start"])?;
                vm_sh(&["wait-ssh", "600"])
            }
            Os::Linux => {
                if !linux_running()? {
                    let log = tart_home().join("tmp").join(format!("{LINUX_VM}.log"));
                    exec(
                        Command::new("sh")
                            .arg("-c")
                            .arg(format!(
                                "nohup tart run {LINUX_VM} --no-graphics > {} 2>&1 &",
                                sh_quote(&log.display().to_string())
                            ))
                            .env("TART_HOME", tart_home()),
                    )?;
                }
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(600) {
                    if linux_ip().is_ok()
                        && self
                            .ssh("true")
                            .and_then(|mut c| {
                                c.stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .status()
                                    .map_err(|e| e.to_string())
                            })
                            .is_ok_and(|s| s.success())
                    {
                        eprintln!("ssh up");
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_secs(3));
                }
                Err(format!("{LINUX_VM}: SSH did not come up within 600 s"))
            }
        }
    }

    fn stop(&self) -> Result {
        match self.os {
            Os::Windows => vm_sh(&["stop"]),
            Os::Linux => {
                if linux_running()? {
                    exec(&mut tart(&["stop", LINUX_VM, "--timeout", "60"]))?;
                }
                eprintln!("stopped");
                Ok(())
            }
        }
    }

    fn status(&self) -> Result {
        match self.os {
            Os::Windows => vm_sh(&["status"]),
            Os::Linux => {
                let running = linux_running()?;
                println!(
                    "vm:  {LINUX_VM} {}",
                    if running { "running" } else { "stopped" }
                );
                if running {
                    let up = self.ssh("true").and_then(|mut c| {
                        c.stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status()
                            .map_err(|e| e.to_string())
                    });
                    match (linux_ip(), up) {
                        (Ok(ip), Ok(s)) if s.success() => println!("ssh: up ({ip})"),
                        _ => println!("ssh: not reachable"),
                    }
                }
                Ok(())
            }
        }
    }

    fn ssh_interactive(&self, command: &str) -> Result {
        let (options, destination) = self.ssh_target()?;
        let mut ssh = Command::new("ssh");
        ssh.args(options).arg("-t").arg(destination);
        if !command.is_empty() {
            ssh.arg(command);
        }
        exec(&mut ssh)
    }

    fn provision(&self, paths: &Paths) -> Result {
        match self.os {
            Os::Linux => {
                self.scp(
                    &paths
                        .root
                        .join("xtask/hosts/linux/provision.sh")
                        .display()
                        .to_string(),
                    ":asrs-provision.sh",
                )?;
                self.script("bash $HOME/asrs-provision.sh")
            }
            Os::Windows => {
                self.script(&format!(
                    "New-Item -Force -ItemType Directory {SYNC_DIR} | Out-Null"
                ))?;
                self.scp(
                    &paths
                        .root
                        .join("xtask/hosts/windows/provision.ps1")
                        .display()
                        .to_string(),
                    &format!(":{SYNC_DIR}/provision.ps1"),
                )?;
                self.script(&format!("& {SYNC_DIR}/provision.ps1\nexit $LASTEXITCODE"))
            }
        }
    }

    fn sync(&self, paths: &Paths) -> Result {
        let state = paths.root.join("target/hosts").join(self.name);
        fs::create_dir_all(&state).map_err(|e| format!("{}: {e}", state.display()))?;
        let files = checkout_files(&paths.root)?;
        let listing = state.join("synced.txt");
        let previous = read_sync_listing(&listing);
        let stamps = file_stamps(&paths.root, &files)?;
        let changed: BTreeSet<&String> = stamps
            .iter()
            .filter(|(f, stamp)| previous.get(f) != Some(stamp))
            .map(|(f, _)| f)
            .collect();
        let tarball = state.join("sync.tar.gz");
        write_tarball(&paths.root, &files, &changed, &tarball)?;
        let size = fs::metadata(&tarball).map(|m| m.len()).unwrap_or_default();
        eprintln!(
            "sync {} files ({:.1} MB) -> {}:{}",
            files.len(),
            size as f64 / 1e6,
            self.name,
            self.checkout()
        );
        let removed: Vec<&String> = previous.keys().filter(|f| !files.contains(*f)).collect();

        match self.os {
            Os::Linux => {
                let mut ssh = self.ssh(&self.remote_script(&format!(
                    "mkdir -p {0} && tar -xzf - -C {0}",
                    self.checkout()
                )))?;
                ssh.stdin(File::open(&tarball).map_err(|e| format!("{}: {e}", tarball.display()))?);
                exec(&mut ssh)?;
                if !removed.is_empty() {
                    let list: Vec<String> = removed.iter().map(|f| sh_quote(f)).collect();
                    self.script(&format!(
                        "cd {} && rm -f -- {}",
                        self.checkout(),
                        list.join(" ")
                    ))?;
                }
            }
            Os::Windows => {
                self.script(&format!(
                    "New-Item -Force -ItemType Directory {}, {SYNC_DIR} | Out-Null",
                    self.checkout()
                ))?;
                self.scp(
                    &tarball.display().to_string(),
                    &format!(":{SYNC_DIR}/asrs.tar.gz"),
                )?;
                let mut body = format!(
                    "tar.exe -xzf {SYNC_DIR}/asrs.tar.gz -C {}\nif ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\nRemove-Item {SYNC_DIR}/asrs.tar.gz\n",
                    self.checkout()
                );
                for file in &removed {
                    body.push_str(&format!(
                        "Remove-Item -Force -ErrorAction SilentlyContinue -LiteralPath '{}/{}'\n",
                        self.checkout(),
                        file.replace('\'', "''")
                    ));
                }
                self.script(&body)?;
            }
        }
        if !removed.is_empty() {
            eprintln!(
                "  removed {} files deleted since the last sync",
                removed.len()
            );
        }
        let text: String = stamps
            .iter()
            .map(|(f, stamp)| format!("{stamp} {f}\n"))
            .collect();
        fs::write(&listing, text).map_err(|e| format!("{}: {e}", listing.display()))?;
        self.sync_pack(paths, &state)
    }

    fn sync_pack(&self, paths: &Paths, state: &Path) -> Result {
        let pack_file = paths.out.join("pack.tar.zst");
        let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
        if modified(&pack_file) < modified(&paths.install) {
            pack::pack(paths, &[])?;
        }
        let digest = crate::sha256_hex(&oracle::read(&pack_file)?);
        let stamp = state.join("pack.sha256");
        if fs::read_to_string(&stamp).is_ok_and(|s| s.trim() == digest) {
            eprintln!("oracle pack already on {}", self.name);
            return Ok(());
        }
        eprintln!("upload oracle pack -> {}:{}", self.name, self.pack_path());
        self.scp(
            &pack_file.display().to_string(),
            &format!(":{}", self.pack_path()),
        )?;
        fs::write(&stamp, format!("{digest}\n")).map_err(|e| format!("{}: {e}", stamp.display()))
    }

    fn fetch(&self, paths: &Paths, dir: &str) -> Result {
        if dir.split(['/', '\\']).any(|c| c == ".." || c.is_empty()) && !dir.is_empty() {
            return Err(format!("fetch: {dir:?} is not a path under target/parity"));
        }
        let source = if dir.is_empty() {
            "parity".to_owned()
        } else {
            format!("parity/{dir}")
        };
        let state = paths.root.join("target/hosts").join(self.name);
        fs::create_dir_all(&state).map_err(|e| format!("{}: {e}", state.display()))?;
        let tarball = state.join("fetch.tar.gz");
        match self.os {
            Os::Linux => {
                let bytes = self.script_output(&format!(
                    "tar -czf - -C {}/target {source}",
                    self.checkout()
                ))?;
                fs::write(&tarball, bytes).map_err(|e| format!("{}: {e}", tarball.display()))?;
            }
            Os::Windows => {
                self.script(&format!(
                    "New-Item -Force -ItemType Directory {SYNC_DIR} | Out-Null\ntar.exe -czf {SYNC_DIR}/fetch.tar.gz -C {}/target {source}\nexit $LASTEXITCODE",
                    self.checkout()
                ))?;
                self.scp(
                    &format!(":{SYNC_DIR}/fetch.tar.gz"),
                    &tarball.display().to_string(),
                )?;
            }
        }
        let dest = paths.root.join("target/parity").join(self.name);
        let count = unpack_parity(&tarball, &dest)?;
        eprintln!("fetched {count} files into {}", dest.display());
        Ok(())
    }

    fn theme(&self, dark: bool) -> Result {
        match self.os {
            Os::Linux => self.gui(&format!(
                "xfconf-query -c xsettings -p /Net/ThemeName -s {}",
                if dark { "Adwaita-dark" } else { "Adwaita" }
            )),
            Os::Windows => {
                let value = if dark { 0 } else { 1 };
                self.script(&format!(
                    "$key = 'HKCU:/Software/Microsoft/Windows/CurrentVersion/Themes/Personalize'\nSet-ItemProperty $key AppsUseLightTheme {value} -Type DWord\nSet-ItemProperty $key SystemUsesLightTheme {value} -Type DWord"
                ))
            }
        }
    }

    fn screenshot(&self, file: &Path) -> Result {
        let file = std::path::absolute(file).map_err(|e| e.to_string())?;
        match self.os {
            Os::Windows => vm_sh(&["screenshot", &file.display().to_string()]),
            Os::Linux => {
                let png = self.script_output(
                    "DISPLAY=:0 XAUTHORITY=$HOME/.Xauthority import -window root png:-",
                )?;
                fs::write(&file, png).map_err(|e| format!("{}: {e}", file.display()))?;
                println!("{}", file.display());
                Ok(())
            }
        }
    }

    fn record_manifest(&self, paths: &Paths) -> Result {
        let file = paths
            .root
            .join("target/parity")
            .join(self.name)
            .join("host/host.json");
        let text = fs::read_to_string(&file).map_err(|e| {
            format!(
                "{}: {e}; run `cargo xtask host {} check` first",
                file.display(),
                self.name
            )
        })?;
        let description = json::parse(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        let platform = match self.os {
            Os::Windows => "windows",
            Os::Linux => "linux",
        };
        canvas::set_manifest_field(paths, platform, "host", description)?;
        eprintln!("host description stored in parity/vectors/canvas/{platform}.json");
        Ok(())
    }

    fn check(&self, paths: &Paths) -> Result {
        self.sync(paths)?;
        self.run(&self.setup_command())?;
        let s1_build = "cargo build --release --manifest-path spikes/s1-gpui-shell/Cargo.toml";
        let steps: [(&str, bool, &str); 7] = [
            ("selftest", false, "cargo xtask oracle selftest"),
            ("smoke", false, "cargo xtask oracle smoke"),
            ("gen --check", true, "cargo xtask oracle gen --check"),
            ("port build", false, "cargo build --workspace --all-targets"),
            ("gui-check", true, "cargo xtask host local gui-check"),
            ("s1 build", false, s1_build),
            ("s1 window", true, "cargo xtask host local s1"),
        ];
        let mut results = Vec::new();
        for (name, gui, command) in steps {
            let result = if gui {
                self.gui(command)
            } else {
                self.run(command)
            };
            results.push((name, result));
        }
        let fetched = self.fetch(paths, "host");
        eprintln!("\n{} check:", self.name);
        let mut failed = 0;
        for (name, result) in &results {
            match result {
                Ok(()) => eprintln!("  pass  {name}"),
                Err(e) => {
                    failed += 1;
                    eprintln!("  FAIL  {name}: {e}");
                }
            }
        }
        fetched?;
        if failed == 0 {
            Ok(())
        } else {
            Err(format!("{}: {failed} checks failed", self.name))
        }
    }
}

fn windows_vm() -> PathBuf {
    std::env::var_os("ASRS_WINDOWS_VM")
        .map_or_else(|| PathBuf::from("/Volumes/T9/vms/win11-arm"), PathBuf::from)
}

fn linux_keys() -> PathBuf {
    std::env::var_os("ASRS_LINUX_KEYS").map_or_else(
        || PathBuf::from("/Volumes/T9/asrs/hosts/asrs-linux"),
        PathBuf::from,
    )
}

fn tart_home() -> PathBuf {
    std::env::var_os("TART_HOME").map_or_else(|| PathBuf::from("/Volumes/T9/tart"), PathBuf::from)
}

fn tart(args: &[&str]) -> Command {
    let mut command = Command::new("tart");
    command.args(args).env("TART_HOME", tart_home());
    command
}

fn linux_running() -> Result<bool> {
    let output = tart(&["list", "--source", "local", "--format", "json"])
        .output()
        .map_err(|e| format!("tart: {e} (install it with `brew install openai/tools/tart`)"))?;
    let list = json::parse(&String::from_utf8_lossy(&output.stdout))
        .map_err(|e| format!("tart list: {e}"))?;
    Ok(list.as_array().unwrap_or_default().iter().any(|vm| {
        vm.get("Name")
            .and_then(Json::as_str)
            .is_some_and(|n| n.to_string_lossy() == LINUX_VM)
            && vm.get("Running") == Some(&Json::Bool(true))
    }))
}

fn linux_ip() -> Result<String> {
    let output = tart(&["ip", LINUX_VM, "--wait", "5"])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("tart: {e}"))?;
    let ip = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() && !ip.is_empty() {
        Ok(ip)
    } else {
        Err(format!(
            "{LINUX_VM} has no IP address; start it with `cargo xtask host linux start`"
        ))
    }
}

fn vm_sh(args: &[&str]) -> Result {
    exec(Command::new(windows_vm().join("vm.sh")).args(args))
}

fn exec(command: &mut Command) -> Result {
    let status = command
        .status()
        .map_err(|e| format!("failed to run {:?}: {e}", command.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed ({status})", command.get_program()))
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn powershell_encode(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn checkout_files(root: &Path) -> Result<BTreeSet<String>> {
    let output = Command::new("git")
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !output.status.success() {
        return Err("git ls-files failed".to_owned());
    }
    Ok(output
        .stdout
        .split(|&b| b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .filter(|f| !f.split('/').any(|c| c == "target" || c == "node_modules"))
        .filter(|f| fs::symlink_metadata(root.join(f)).is_ok_and(|m| m.file_type().is_file()))
        .collect())
}

fn read_sync_listing(listing: &Path) -> std::collections::BTreeMap<String, String> {
    fs::read_to_string(listing)
        .unwrap_or_default()
        .lines()
        .map(|line| match line.splitn(3, ' ').collect::<Vec<_>>()[..] {
            [mtime, size, path] => (path.to_owned(), format!("{mtime} {size}")),
            _ => (line.to_owned(), String::new()),
        })
        .collect()
}

fn file_stamps(root: &Path, files: &BTreeSet<String>) -> Result<Vec<(String, String)>> {
    files
        .iter()
        .map(|f| {
            let meta = fs::metadata(root.join(f)).map_err(|e| format!("{f}: {e}"))?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .unwrap_or_default();
            Ok((
                f.clone(),
                format!(
                    "{}.{:09} {}",
                    mtime.as_secs(),
                    mtime.subsec_nanos(),
                    meta.len()
                ),
            ))
        })
        .collect()
}

// Unchanged files keep their mtime so the host's cargo does not rebuild them; changed files get
// the sync time, because their own mtime can predate the host's last build of the old content.
fn write_tarball(
    root: &Path,
    files: &BTreeSet<String>,
    changed: &BTreeSet<&String>,
    out: &Path,
) -> Result {
    let file = File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let encoder = flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    for f in files {
        let source = File::open(root.join(f)).map_err(|e| format!("{f}: {e}"))?;
        let meta = source.metadata().map_err(|e| format!("{f}: {e}"))?;
        let mut header = tar::Header::new_gnu();
        header.set_metadata(&meta);
        if changed.contains(f) {
            header.set_mtime(now);
        }
        builder
            .append_data(&mut header, f, source)
            .map_err(|e| format!("{f}: {e}"))?;
    }
    builder
        .into_inner()
        .and_then(|e| e.finish())
        .and_then(|mut w| w.flush())
        .map_err(|e| format!("{}: {e}", out.display()))
}

fn unpack_parity(tarball: &Path, dest: &Path) -> Result<usize> {
    let file = File::open(tarball).map_err(|e| format!("{}: {e}", tarball.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut count = 0;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let Ok(rel) = path.strip_prefix("parity") else {
            return Err(format!(
                "unexpected entry {} in the fetched archive",
                path.display()
            ));
        };
        if rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "unsafe entry {} in the fetched archive",
                path.display()
            ));
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let target = dest.join(rel);
        fs::create_dir_all(target.parent().expect("entry has a parent"))
            .map_err(|e| e.to_string())?;
        entry
            .unpack(&target)
            .map_err(|e| format!("{}: {e}", target.display()))?;
        count += 1;
    }
    Ok(count)
}

fn run_local(paths: &Paths, args: &[String]) -> Result {
    let out = paths.root.join("target/parity/host");
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    match args {
        [cmd] if cmd == "describe" => describe(paths, &out, false),
        [cmd, flag] if cmd == "describe" && flag == "--manifest" => describe(paths, &out, true),
        [cmd] if cmd == "colour-patch" => colour_patch(paths, &out),
        [cmd] if cmd == "launch" => launch(paths, &out),
        [cmd] if cmd == "gui-check" => {
            let results = [
                ("describe", describe(paths, &out, false)),
                ("colour-patch", colour_patch(paths, &out)),
                ("launch", launch(paths, &out)),
            ];
            let failed: Vec<String> = results
                .into_iter()
                .filter_map(|(name, r)| r.err().map(|e| format!("{name}: {e}")))
                .collect();
            if failed.is_empty() {
                Ok(())
            } else {
                Err(failed.join("\n"))
            }
        }
        [cmd] if cmd == "s1" => s1(paths, &out),
        _ => Err(USAGE.to_owned()),
    }
}

fn build_host_bundle(paths: &Paths) -> Result {
    exec(
        Command::new("node")
            .arg(paths.oracle_src.join("host/build.mjs"))
            .arg("--upstream")
            .arg(&paths.upstream)
            .arg("--harness")
            .arg(&paths.harness)
            .arg("--out")
            .arg(&paths.dist),
    )
}

fn electron_host(paths: &Paths, switches: &[&str], args: &[&str], result: &Path) -> Result {
    let _ = fs::remove_file(result);
    let mut command = Command::new(paths.electron()?);
    command
        .args(switches)
        .arg(paths.dist.join("hostMain.mjs"))
        .args(args)
        .env_remove("ELECTRON_RUN_AS_NODE");
    oracle::pin_environment(&mut command, oracle::DEFAULT_ZONE);
    let status = command
        .status()
        .map_err(|e| format!("running Electron: {e}"))?;
    if !result.exists() {
        return Err(format!(
            "hostMain {} wrote no {} ({status})",
            args.join(" "),
            result.display()
        ));
    }
    if !status.success() {
        eprintln!(
            "warning: hostMain {} wrote its result but Electron exited with {status}",
            args.join(" ")
        );
    }
    Ok(())
}

fn read_json_file(path: &Path) -> Result<Json> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn patch_result(
    paths: &Paths,
    dir: &Path,
    switches: &[&str],
    scale: &str,
    exact_required: bool,
) -> Result<(f64, f64)> {
    let file = dir.join(format!("colour-patch@{scale}x.json"));
    let factor = format!("--force-device-scale-factor={scale}");
    let switches: Vec<&str> = switches.iter().copied().chain([factor.as_str()]).collect();
    electron_host(
        paths,
        &switches,
        &["colour-patch", &dir.display().to_string(), scale],
        &file,
    )?;
    let result = read_json_file(&file)?;
    let number = |k: &str| result.get(k).and_then(Json::as_f64).unwrap_or(f64::NAN);
    let failures = result
        .get("failures")
        .and_then(Json::as_array)
        .map_or(usize::MAX, <[Json]>::len);
    if number("patches") != 512.0 || failures != 0 {
        return Err(format!(
            "colour-patch@{scale}x: {failures} of {} patches off by more than 1 level ({})",
            number("patches"),
            file.display()
        ));
    }
    if exact_required && number("exact") != 512.0 {
        return Err(format!(
            "colour-patch@{scale}x: {} of 512 patches exact on an sRGB display, max delta {} ({})",
            number("exact"),
            number("maxDelta"),
            file.display()
        ));
    }
    Ok((number("exact"), number("maxDelta")))
}

fn colour_patch(paths: &Paths, out: &Path) -> Result {
    build_host_bundle(paths)?;
    let srgb_display = std::env::consts::OS != "macos";
    let raw = out.join("raw");
    let runs: [(&str, &Path, &[&str], bool); 2] = [
        ("sRGB forced", out, COLOUR_SWITCHES, true),
        ("display profile", &raw, &["--lang=en-US"], srgb_display),
    ];
    let mut failures = Vec::new();
    for scale in ["1", "2"] {
        for (label, dir, switches, gated) in runs {
            match patch_result(paths, dir, switches, scale, srgb_display) {
                Ok((exact, delta)) => {
                    eprintln!(
                        "colour-patch@{scale}x ({label}): {exact}/512 exact, max delta {delta}"
                    )
                }
                Err(e) if gated => failures.push(format!("{label}: {e}")),
                Err(e) => eprintln!("colour-patch@{scale}x ({label}, not gated): {e}"),
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn launch(paths: &Paths, out: &Path) -> Result {
    build_host_bundle(paths)?;
    electron_host(
        paths,
        COLOUR_SWITCHES,
        &["launch", &out.display().to_string()],
        &out.join("launch.json"),
    )?;
    let result = read_json_file(&out.join("launch.json"))?;
    let number = |path: &[&str]| {
        path.iter()
            .try_fold(&result, |v, k| v.get(k))
            .and_then(Json::as_f64)
            .unwrap_or(f64::NAN)
    };
    let scale = number(&["scaleFactor"]);
    let expected = (
        number(&["contentBounds", "width"]) * scale,
        number(&["contentBounds", "height"]) * scale,
    );
    let captured = (
        number(&["capture", "width"]),
        number(&["capture", "height"]),
    );
    if captured == expected {
        Ok(())
    } else {
        Err(format!(
            "launch: hub capture is {}x{}, expected the {}x{} window content",
            captured.0, captured.1, expected.0, expected.1
        ))
    }
}

fn describe(paths: &Paths, out: &Path, manifest: bool) -> Result {
    build_host_bundle(paths)?;
    electron_host(
        paths,
        COLOUR_SWITCHES,
        &["info", &out.display().to_string()],
        &out.join("electron.json"),
    )?;
    let electron = read_json_file(&out.join("electron.json"))?;
    let pick = |path: &[&str]| {
        path.iter()
            .try_fold(&electron, |v, k| v.get(k))
            .cloned()
            .unwrap_or(Json::Null)
    };
    let fonts = font_hashes()?;
    let description = Json::object([
        ("os", Json::string(std::env::consts::OS)),
        ("arch", Json::string(std::env::consts::ARCH)),
        ("osRelease", pick(&["osRelease"])),
        ("osVersion", pick(&["osVersion"])),
        ("cpus", pick(&["cpus"])),
        ("memoryBytes", pick(&["memoryBytes"])),
        ("electron", pick(&["electron"])),
        ("chrome", pick(&["chrome"])),
        ("darkTheme", pick(&["darkTheme"])),
        ("displays", pick(&["displays"])),
        (
            "chromiumRasterizer",
            Json::object([
                (
                    "glRenderer",
                    pick(&["gpuInfo", "auxAttributes", "glRenderer"]),
                ),
                ("glVendor", pick(&["gpuInfo", "auxAttributes", "glVendor"])),
                (
                    "glImplementation",
                    pick(&["gpuInfo", "auxAttributes", "glImplementationParts"]),
                ),
                ("featureStatus", pick(&["gpuFeatureStatus"])),
            ]),
        ),
        ("nativeRasterizers", native_rasterizers()),
        ("displayProfile", display_profile()),
        (
            "fonts",
            Json::object(fonts.iter().map(|(path, hash)| {
                (
                    path.as_str(),
                    hash.as_deref().map_or(Json::Null, Json::string),
                )
            })),
        ),
    ]);
    let text = description.to_pretty() + "\n";
    fs::write(out.join("host.json"), &text).map_err(|e| e.to_string())?;
    print!("{text}");
    if manifest {
        canvas::set_manifest_field(paths, canvas::platform(), "host", description)?;
    }
    Ok(())
}

fn command_text(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn native_rasterizers() -> Json {
    let line = |text: Option<String>, prefix: &str| {
        text.and_then(|t| {
            t.lines()
                .find_map(|l| l.trim().strip_prefix(prefix).map(|v| v.trim().to_owned()))
        })
        .map_or(Json::Null, |v| Json::string(&v))
    };
    match std::env::consts::OS {
        "linux" => Json::object([
            (
                "openGl",
                line(command_text("glxinfo", &["-B"]), "OpenGL renderer string:"),
            ),
            (
                "vulkan",
                line(
                    command_text("vulkaninfo", &["--summary"]),
                    "deviceName         =",
                ),
            ),
            (
                "xServer",
                line(command_text("xdpyinfo", &[]), "vendor string:"),
            ),
        ]),
        "windows" => {
            let names = command_text(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    "(Get-CimInstance Win32_VideoController).Name",
                ],
            )
            .unwrap_or_default();
            Json::object([
                (
                    "displayAdapters",
                    Json::Array(names.lines().map(|l| Json::string(l.trim())).collect()),
                ),
                (
                    "direct3d",
                    Json::string(
                        if names.is_empty()
                            || names.contains("Basic Display")
                            || names.contains("DOD")
                        {
                            "WARP (no render-capable adapter: display-only driver)"
                        } else {
                            "adapter driver"
                        },
                    ),
                ),
            ])
        }
        _ => Json::object([(
            "gpu",
            line(
                command_text("system_profiler", &["SPDisplaysDataType"]),
                "Chipset Model:",
            ),
        )]),
    }
}

fn display_profile() -> Json {
    match std::env::consts::OS {
        "linux" => {
            let icc = command_text("xprop", &["-root", "_ICC_PROFILE"]).unwrap_or_default();
            Json::string(if icc.contains("not found") || icc.is_empty() {
                "none (sRGB)"
            } else {
                "_ICC_PROFILE set on the root window"
            })
        }
        "windows" => {
            let dpi = command_text(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    "(Get-ItemProperty 'HKCU:/Control Panel/Desktop').LogPixels",
                ],
            )
            .unwrap_or_default();
            Json::object([
                ("logPixels", Json::string(&dpi)),
                (
                    "iccProfiles",
                    Json::string(
                        &command_text(
                            "powershell",
                            &[
                                "-NoProfile",
                                "-Command",
                                "(Get-ChildItem C:/Windows/System32/spool/drivers/color -Filter *.ic* | Select-Object -ExpandProperty Name) -join ','",
                            ],
                        )
                        .unwrap_or_default(),
                    ),
                ),
            ])
        }
        _ => Json::Null,
    }
}

fn font_hashes() -> Result<Vec<(String, Option<String>)>> {
    let files: Vec<String> = match std::env::consts::OS {
        "windows" => [
            "segoeui.ttf",
            "segoeuib.ttf",
            "segoeuii.ttf",
            "segoeuiz.ttf",
            "segoeuil.ttf",
            "segoeuisl.ttf",
            "seguisb.ttf",
            "seguisbi.ttf",
            "SegUIVar.ttf",
            "seguiemj.ttf",
            "seguisym.ttf",
            "arial.ttf",
            "arialbd.ttf",
            "cour.ttf",
            "courbd.ttf",
            "consola.ttf",
            "consolab.ttf",
            "times.ttf",
        ]
        .iter()
        .map(|f| format!("C:/Windows/Fonts/{f}"))
        .collect(),
        "linux" => {
            let mut files = Vec::new();
            for pattern in [
                "sans-serif",
                "sans-serif:bold",
                "serif",
                "monospace",
                "monospace:bold",
                "system-ui",
                "ui-sans-serif",
                "Courier",
                "Courier:bold",
                "emoji",
            ] {
                let file = command_text("fc-match", &["-f", "%{file}", pattern])
                    .ok_or("fc-match failed; install fontconfig")?;
                if !files.contains(&file) {
                    files.push(file);
                }
            }
            files
        }
        _ => [
            "SFNS.ttf",
            "SFNSItalic.ttf",
            "SFNSMono.ttf",
            "Helvetica.ttc",
            "Courier.ttc",
            "Menlo.ttc",
            "Times.ttc",
            "Apple Color Emoji.ttc",
        ]
        .iter()
        .map(|f| format!("/System/Library/Fonts/{f}"))
        .collect(),
    };
    Ok(files
        .into_iter()
        .map(|f| {
            let hash = fs::read(&f).ok().map(|b| crate::sha256_hex(&b));
            (f, hash)
        })
        .collect())
}

fn s1(paths: &Paths, out: &Path) -> Result {
    let exe = paths
        .root
        .join("spikes/s1-gpui-shell/target/release")
        .join(format!("s1-gpui-shell{}", std::env::consts::EXE_SUFFIX));
    if !exe.exists() {
        return Err(format!(
            "{} is missing; run `cargo build --release --manifest-path spikes/s1-gpui-shell/Cargo.toml`",
            exe.display()
        ));
    }
    let dir = out.join("s1");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let log = dir.join("auto.log");
    let mut child = Command::new(&exe)
        .arg("auto")
        .env("S1_OUT", &dir)
        .stdout(File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?)
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if start.elapsed() > S1_TIMEOUT {
            let _ = child.kill();
            break None;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let mut text = String::new();
    File::open(&log)
        .and_then(|mut f| f.read_to_string(&mut text))
        .map_err(|e| e.to_string())?;
    print!("{text}");
    let opened = text.lines().any(|l| l.starts_with("gpui bounds="));
    match status {
        Some(s) if s.success() && opened => Ok(()),
        Some(s) => Err(format!("S1 spike exited with {s}; window opened: {opened}")),
        None => Err(format!(
            "S1 spike did not finish within {} s",
            S1_TIMEOUT.as_secs()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn powershell_commands_are_utf16le() {
        assert_eq!(powershell_encode("ls"), "bABzAA==");
    }

    #[test]
    fn sync_gives_only_changed_files_a_new_mtime() {
        let root = std::env::temp_dir().join(format!("asrs-host-sync-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let files: BTreeSet<String> = ["kept.rs", "edited.rs"].map(str::to_owned).into();
        let old = std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        for f in &files {
            fs::write(root.join(f), f).unwrap();
            File::options()
                .write(true)
                .open(root.join(f))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let stamps = file_stamps(&root, &files).unwrap();
        let listing = root.join("synced.txt");
        let text: String = stamps.iter().map(|(f, s)| format!("{s} {f}\n")).collect();
        fs::write(&listing, text).unwrap();
        assert_eq!(
            read_sync_listing(&listing),
            stamps.iter().cloned().collect()
        );

        let edited = "edited.rs".to_owned();
        let tarball = root.join("sync.tar.gz");
        write_tarball(&root, &files, &[&edited].into(), &tarball).unwrap();
        let mut archive =
            tar::Archive::new(flate2::read::GzDecoder::new(File::open(&tarball).unwrap()));
        let mut seen = 0;
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            seen += 1;
            let path = entry.path().unwrap().display().to_string();
            let mtime = entry.header().mtime().unwrap();
            if path == "kept.rs" {
                assert_eq!(mtime, 1_000_000_000);
            } else {
                assert!(mtime > 1_700_000_000, "{path}: {mtime}");
            }
        }
        assert_eq!(seen, 2);
        fs::remove_dir_all(&root).unwrap();
    }
}
