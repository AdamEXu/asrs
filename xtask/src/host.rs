use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgba};

use crate::oracle::{self, Paths};
use crate::{Result, canvas, gl, pack};

const USAGE: &str = r"usage: cargo xtask host <command>

Render hosts for the Windows and Linux parity track: VMs driven over SSH. Neither has GPU
acceleration, so Chromium and native code render in software (`cargo xtask host local describe`
records which rasterizers are in use).

  windows      Windows 11 x86_64 (QEMU), the Windows reference: ~/vms/win11-x64 on omarchy
               (ASRS_WINDOWS_VM; README.md there). ASRS_WINDOWS_VIA is the ssh destination of the
               machine it runs on, empty when that is this one. The VM's own vm.sh starts it and reaches
               it. `start` takes the VM's lock directory `in-use`
               for this ASRS_HOST_DIR and `stop` gives it back; a lock of anyone else's is
               refused. Start it cold (`restart`) for captures: once Windows' shell has been used
               for a while its Snap Layouts flyout can cover a hovered caption button.
  windows-arm  Windows 11 ARM64 (UTM, Win11-ARM), /Volumes/T9/vms/win11-arm
               (ASRS_WINDOWS_ARM_VM), for windows-aarch64 data vectors. An update changed its
               fonts, so it is not a pixel reference. Shared with other projects: do not stop it
               while someone else is using it.
  linux        Ubuntu 24.04 ARM64 (tart), the Linux reference: `asrs-linux` under
               TART_HOME=/Volumes/T9/tart, Xorg on virtio-gpu with an autologin xfce session on
               :0, 2880x1800 so that a 2200x1300 window (1100x650 at scale 2) fits. SSH
               admin@$(tart ip asrs-linux) with /Volumes/T9/asrs/hosts/asrs-linux/id_ed25519
               (ASRS_LINUX_KEYS).

Another Linux machine can run the `local` commands and the suites with the same fonts and a
display of the same size, without touching its own desktop: unpack the fonts with
xtask/hosts/linux/fonts.sh and run `FONTCONFIG_FILE=<dir>/gray.conf
xtask/hosts/linux/display.sh <command>`. It is a check on another architecture, not a reference,
except that the DOM text scenes and the LCD table were recorded on x86_64 (linux.json says so).

Checkouts are C:\work\asrs and ~/asrs, oracle packs C:\work\asrs-pack.tar.zst and
~/asrs-pack.tar.zst. ASRS_HOST_DIR=<name> replaces `asrs` in those paths (and suffixes the Windows
.sync and .gui directories, the desktop task and the local sync state in target/hosts), so agents
sharing a VM keep separate checkouts.

The dev machine has 16 GB of memory; the ARM64 VMs take 8 GB (Windows) and 6 GB (Linux, plus 8 GB
of swap inside it), so avoid heavy builds on both at once.

commands on a host (cargo xtask host <name> ...):
  start | stop | restart | status
                          boot the VM and wait for SSH, shut it down, both, or report its state
  ssh [<command>...]      interactive shell, or one remote command
  provision               run xtask/hosts/<os>/provision.* there (packages, display, text
                          rendering, theme, updates, toolchains) and on Linux unpack the fonts of
                          fonts.sh into ~/asrs-fonts; idempotent. `host local describe --check`
                          then compares the host with its recorded description
  sync                    copy the checkout (tracked plus untracked files that are not ignored),
                          remove files deleted since the last sync, and upload the oracle pack
                          (`cargo xtask oracle pack` first when it is missing or older than the
                          oracle install)
  setup                   `cargo xtask oracle setup --from-pack` there
  run [<options>] <command>...
                          run a command in the checkout over SSH
  gui [<options>] <command>...
                          run a command in the checkout inside the desktop session and wait. One
                          runs at a time on a host, whoever started it; the others queue.
                          Both set ASRS_GL, the rasterizer of 3D references (--gl native,
                          swiftshader or d3d11-warp; default swiftshader on Linux and
                          d3d11-warp on Windows), and on Linux
                          FONTCONFIG_FILE (--fonts gray, the default, for grayscale DOM text, or
                          stock for LCD)
  fetch [<dir>]           copy target/parity[/<dir>] from the host into target/parity/<name>/
  gen <id>...             sync, run `oracle gen <id>...` there for the host's platform vectors
                          and architecture (specs with `platforms`), and copy those vectors and
                          their manifest entries back into parity/vectors
  theme light|dark        switch the desktop theme
  screenshot <file>       PNG of the host display
  manifest                store the fetched host description (target/parity/<name>/host/host.json)
                          as `host` in parity/vectors/canvas/<os>.json
  check                   the X01 exit criteria: sync, setup (selftest and smoke), selftest,
                          smoke, `oracle gen --check` (in the desktop session: the proto-renderer
                          vector opens a BrowserWindow), a workspace build, `host local
                          gui-check` (host description, colour patches at 1x and 2x, real app
                          launch), the 3D references, DOM text antialiasing (under both font
                          profiles on Linux), the S1 spike window, then fetch target/parity/host

commands on the machine itself (cargo xtask host local ...):
  describe [--manifest | --check]
                          write target/parity/host/host.json: OS, Electron, displays, theme,
                          Chromium and native rasterizers, text rendering settings, update
                          settings, sha256 of the UI font files. With --manifest also store it as
                          `host` in parity/vectors/canvas/<os>.json, unless a font recorded there
                          is another file on this host. With --check require that it equals the
                          one stored there
  colour-patch            512 sRGB patches through capturePage at device scale 1 and 2, with
                          and without --force-color-profile=srgb. Gate on Windows and Linux,
                          whose display profile is pinned to sRGB: every patch exact in both
                          runs. On macOS: within 1 level per channel with the switch; the run
                          without it is recorded only
  launch                  start the real upstream app with isolated userData and capture its hub
  gui-check               describe, colour-patch and launch
  dom-text                Chromium's DOM text at device scale 1 and 2: every text box grayscale
                          under fontconfig's gray.conf; under stock.conf and on Windows every one
                          LCD up to 48 device px and grayscale above
  s1                      run the S1 spike (`auto`) and require that its window opened

Results land in target/parity/host/ on the machine that ran them.";

const LINUX_VM: &str = "asrs-linux";
const LINUX_DISPLAY: &str = "2880x1800";
const FONT_PROFILES: [&str; 2] = ["gray", "stock"];
const VM_LOCK: &str = "in-use";
const COLOUR_SWITCHES: &[&str] = &["--force-color-profile=srgb", "--lang=en-US"];
const S1_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Os {
    Windows,
    Linux,
}

enum Vm {
    Script(VmScript),
    Tart,
}

/// A VM with a `vm.sh` that starts it and reaches it over SSH.
struct VmScript {
    dir: &'static str,
    dir_variable: &'static str,
    via: &'static str,
    via_variable: Option<&'static str>,
    port: u16,
    locked: bool,
    path: &'static str,
}

struct Host {
    name: &'static str,
    os: Os,
    arch: &'static str,
    reference: bool,
    gl: &'static str,
    vm: Vm,
}

const HOSTS: [Host; 3] = [
    Host {
        name: "windows",
        os: Os::Windows,
        arch: "x86_64",
        reference: true,
        gl: "d3d11-warp",
        vm: Vm::Script(VmScript {
            dir: "vms/win11-x64",
            dir_variable: "ASRS_WINDOWS_VM",
            via: "omarchy",
            via_variable: Some("ASRS_WINDOWS_VIA"),
            port: 2223,
            locked: true,
            path: r"C:\tools\node-v24.18.0-win-x64",
        }),
    },
    Host {
        name: "windows-arm",
        os: Os::Windows,
        arch: "aarch64",
        reference: false,
        gl: "d3d11-warp",
        vm: Vm::Script(VmScript {
            dir: "/Volumes/T9/vms/win11-arm",
            dir_variable: "ASRS_WINDOWS_ARM_VM",
            via: "",
            via_variable: None,
            port: 2222,
            locked: false,
            path: r"C:\tools\node-v24.18.0-win-arm64;C:\tools\clang-20.1.8\bin",
        }),
    },
    Host {
        name: "linux",
        os: Os::Linux,
        arch: "aarch64",
        reference: true,
        gl: "swiftshader",
        vm: Vm::Tart,
    },
];

impl VmScript {
    fn via(&self) -> Vec<String> {
        self.via_variable
            .and_then(|v| std::env::var(v).ok())
            .unwrap_or_else(|| self.via.to_owned())
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    fn dir(&self) -> String {
        let dir = std::env::var(self.dir_variable).unwrap_or_else(|_| self.dir.to_owned());
        match std::env::var("HOME") {
            Ok(home) if self.via().is_empty() && !dir.starts_with('/') => format!("{home}/{dir}"),
            _ => dir,
        }
    }

    fn shell(&self, script: &str, terminal: bool) -> Command {
        let via = self.via();
        if via.is_empty() {
            let mut command = Command::new("sh");
            command.arg("-c").arg(script);
            return command;
        }
        let mut command = Command::new("ssh");
        command.args(if terminal {
            ["-t"].as_slice()
        } else {
            &["-o", "BatchMode=yes"]
        });
        command
            .args(["-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=30"])
            .args(via)
            .arg(script);
        command
    }

    fn vm_sh(&self, args: &[&str]) -> Command {
        self.shell(&self.vm_sh_line(args), false)
    }

    fn vm_sh_line(&self, args: &[&str]) -> String {
        [format!("{}/vm.sh", self.dir())]
            .iter()
            .map(String::as_str)
            .chain(args.iter().copied())
            .map(sh_quote)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn guest_scp(&self, from: &str, to: &str) -> String {
        let keys = format!("{}/keys", self.dir());
        format!(
            "scp -q -i {} -P {} -o IdentitiesOnly=yes -o BatchMode=yes -o LogLevel=ERROR -o UserKnownHostsFile={} {from} {to}",
            sh_quote(&format!("{keys}/id_ed25519")),
            self.port,
            sh_quote(&format!("{keys}/known_hosts")),
        )
    }

    // Not through the guest's own standard input: its login shell, PowerShell, reads that too.
    fn upload(&self, remote: &str) -> Command {
        let copy = self.guest_scp("\"$file\"", &sh_quote(&format!("dev@127.0.0.1:{remote}")));
        self.shell(
            &format!(
                "file=$(mktemp) && cat >\"$file\" && {copy}; status=$?; unlink \"$file\"; exit $status"
            ),
            false,
        )
    }

    fn download(&self, remote: &str) -> Command {
        let copy = self.guest_scp(&sh_quote(&format!("dev@127.0.0.1:{remote}")), "\"$file\"");
        self.shell(
            &format!(
                "file=$(mktemp) && {copy} && cat \"$file\"; status=$?; unlink \"$file\"; exit $status"
            ),
            false,
        )
    }

    fn lock(&self) -> Result {
        if !self.locked {
            return Ok(());
        }
        exec(&mut self.shell(&lock_script(&self.dir(), &host_dir()), false))
            .map_err(|_| format!("{}: the VM is locked", self.dir()))
    }

    fn stop(&self) -> Result {
        if !self.locked {
            return exec(&mut self.vm_sh(&["stop"]));
        }
        let script = unlock_script(&self.dir(), &host_dir(), &self.vm_sh_line(&["stop"]));
        exec(&mut self.shell(&script, false))
    }
}

fn lock_script(dir: &str, owner: &str) -> String {
    let (lock, owner) = (sh_quote(&format!("{dir}/{VM_LOCK}")), sh_quote(owner));
    format!(
        r#"if mkdir {lock} 2>/dev/null; then printf '%s\n' {owner} >{lock}/owner; fi
if [ "$(cat {lock}/owner 2>/dev/null)" != {owner} ]; then
  echo "{lock} is held by $(cat {lock}/owner 2>/dev/null || echo someone else)" >&2; exit 1
fi"#
    )
}

fn unlock_script(dir: &str, owner: &str, stop: &str) -> String {
    let (lock, owner) = (sh_quote(&format!("{dir}/{VM_LOCK}")), sh_quote(owner));
    format!(
        r#"if [ -e {lock} ] && [ "$(cat {lock}/owner 2>/dev/null)" != {owner} ]; then
  echo "{lock} is held by $(cat {lock}/owner 2>/dev/null || echo someone else)" >&2; exit 1
fi
{stop} && if [ -e {lock} ]; then gio trash {lock}; fi"#
    )
}

struct RunOptions {
    fonts: Option<&'static str>,
    gl: &'static str,
    command: String,
}

impl RunOptions {
    fn parse(host: &Host, args: &[String]) -> Result<Self> {
        let mut options = Self::new(host, "");
        let mut rest = args;
        loop {
            match rest {
                [flag, value, tail @ ..] if flag == "--fonts" => {
                    let profile = FONT_PROFILES.into_iter().find(|p| p == value);
                    if host.os != Os::Linux || profile.is_none() {
                        return Err(format!(
                            "--fonts is {} on Linux hosts",
                            FONT_PROFILES.join(" or ")
                        ));
                    }
                    options.fonts = profile;
                    rest = tail;
                }
                [flag, value, tail @ ..] if flag == "--gl" => {
                    options.gl = gl::parse(value)?;
                    rest = tail;
                }
                [] => return Err(USAGE.to_owned()),
                _ => break,
            }
        }
        options.command = rest.join(" ");
        Ok(options)
    }

    fn new(host: &Host, command: &str) -> Self {
        Self {
            fonts: (host.os == Os::Linux).then_some(FONT_PROFILES[0]),
            gl: host.gl,
            command: command.to_owned(),
        }
    }
}

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
        ("restart", []) => host.stop().and_then(|()| host.start()),
        ("status", []) => host.status(),
        ("ssh", _) => host.ssh_interactive(&joined),
        ("provision", []) => host.provision(paths),
        ("sync", []) => host.sync(paths),
        ("setup", []) => host.run(paths, &RunOptions::new(host, &host.setup_command())),
        ("run", _) => host.run(paths, &RunOptions::parse(host, rest)?),
        ("gui", _) => host.gui(paths, &RunOptions::parse(host, rest)?),
        ("fetch", []) => host.fetch(paths, ""),
        ("fetch", [dir]) => host.fetch(paths, dir),
        ("theme", [theme]) if theme == "light" || theme == "dark" => {
            host.theme(paths, theme == "dark")
        }
        ("screenshot", [file]) => host.screenshot(Path::new(file)),
        ("check", []) => host.check(paths),
        ("manifest", []) => host.record_manifest(paths),
        ("gen", [_, ..]) => host.gen_vectors(paths, rest),
        _ => Err(USAGE.to_owned()),
    }
}

impl Host {
    fn tart_ssh(&self, terminal: bool) -> Result<Command> {
        let keys = linux_keys();
        let mut command = Command::new("ssh");
        command
            .arg("-i")
            .arg(keys.join("id_ed25519"))
            .arg("-o")
            .arg(format!(
                "UserKnownHostsFile={}",
                keys.join("known_hosts").display()
            ));
        for option in [
            "IdentitiesOnly=yes",
            "StrictHostKeyChecking=accept-new",
            "ConnectTimeout=10",
            "ServerAliveInterval=30",
            "LogLevel=ERROR",
        ] {
            command.args(["-o", option]);
        }
        command
            .args(if terminal {
                ["-t"].as_slice()
            } else {
                &["-o", "BatchMode=yes"]
            })
            .arg(format!("admin@{}", linux_ip()?));
        Ok(command)
    }

    fn ssh(&self, remote: &str) -> Result<Command> {
        let mut command = match &self.vm {
            Vm::Script(vm) => vm.vm_sh(&["ssh", remote]),
            Vm::Tart => {
                let mut command = self.tart_ssh(false)?;
                command.arg(remote);
                command
            }
        };
        command.stdin(Stdio::null());
        Ok(command)
    }

    fn upload(&self, file: &Path, remote: &str) -> Result {
        let mut command = match &self.vm {
            Vm::Script(vm) => vm.upload(remote),
            Vm::Tart => self.ssh(&self.remote_script(&format!("cat > {remote}")))?,
        };
        command.stdin(File::open(file).map_err(|e| format!("{}: {e}", file.display()))?);
        exec(&mut command)
    }

    fn download(&self, remote: &str, file: &Path) -> Result {
        match &self.vm {
            Vm::Script(vm) => {
                let out = File::create(file).map_err(|e| format!("{}: {e}", file.display()))?;
                exec(vm.download(remote).stdin(Stdio::null()).stdout(out))
            }
            Vm::Tart => self.script_to(&format!("cat {remote}"), file),
        }
    }

    fn script_to(&self, body: &str, file: &Path) -> Result {
        let out = File::create(file).map_err(|e| format!("{}: {e}", file.display()))?;
        exec(self.ssh(&self.remote_script(body))?.stdout(out))
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

    fn state_dir(&self, paths: &Paths) -> PathBuf {
        let hosts = paths.root.join("target/hosts");
        let name = format!("{}-{}", self.name, self.arch);
        match host_dir().as_str() {
            "asrs" => hosts.join(name),
            dir => hosts.join(format!("{name}-{dir}")),
        }
    }

    fn checkout(&self) -> String {
        match self.os {
            Os::Windows => format!("C:/work/{}", host_dir()),
            Os::Linux => format!("$HOME/{}", host_dir()),
        }
    }

    fn pack_path(&self) -> String {
        match self.os {
            Os::Windows => format!("C:/work/{}-pack.tar.zst", host_dir()),
            Os::Linux => format!("{}-pack.tar.zst", host_dir()),
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

    fn path(&self) -> &'static str {
        match &self.vm {
            Vm::Script(vm) => vm.path,
            Vm::Tart => "",
        }
    }

    fn linux_environment(paths: &Paths, options: &RunOptions) -> Result<String> {
        let conf = format!(
            "{}/{}.conf",
            linux_fonts(paths)?,
            options.fonts.unwrap_or(FONT_PROFILES[0])
        );
        Ok(format!(
            "test -f {conf} || {{ echo '{conf} is missing; run `cargo xtask host linux provision`' >&2; exit 1; }}\nexport FONTCONFIG_FILE={conf} {}={}\n",
            gl::VARIABLE,
            options.gl
        ))
    }

    fn run(&self, paths: &Paths, options: &RunOptions) -> Result {
        let command = &options.command;
        eprintln!("[{}] $ {command}", self.name);
        let body = match self.os {
            Os::Linux => format!(
                "{}cd {} && {command}",
                Self::linux_environment(paths, options)?,
                self.checkout()
            ),
            Os::Windows => format!(
                "$ErrorActionPreference = 'Continue'\n$env:Path = '{};' + $env:Path\n$env:{} = '{}'\nSet-Location {}\n& {{ {command} }} | Out-String -Stream\nexit $LASTEXITCODE",
                self.path(),
                gl::VARIABLE,
                options.gl,
                self.checkout()
            ),
        };
        self.script(&body)
            .map_err(|_| format!("{}: `{command}` failed", self.name))
    }

    fn gui(&self, paths: &Paths, options: &RunOptions) -> Result {
        let command = &options.command;
        eprintln!("[{} desktop] $ {command}", self.name);
        let body = match self.os {
            Os::Linux => linux_gui_script(
                &Self::linux_environment(paths, options)?,
                &self.checkout(),
                command,
            ),
            Os::Windows => {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos();
                let id = format!("{}-{stamp}", std::process::id());
                let gui_dir = format!("{}/{id}", windows_aux_dir("gui"));
                let batch = format!(
                    "@echo off\r\nset PATH={};%PATH%\r\nset {}={}\r\ncd /d {}\r\n{command} > {}\\last.log 2>&1\r\n",
                    self.path(),
                    gl::VARIABLE,
                    options.gl,
                    self.checkout().replace('/', "\\"),
                    gui_dir.replace('/', "\\"),
                );
                windows_gui_script(&gui_dir, &format!("{}-{id}", gui_task()), &batch)
            }
        };
        self.script(&body)
            .map_err(|_| format!("{}: `{command}` failed in the desktop session", self.name))
    }

    fn start(&self) -> Result {
        match &self.vm {
            Vm::Script(vm) => {
                vm.lock()?;
                exec(&mut vm.vm_sh(&["start"]))?;
                exec(&mut vm.vm_sh(&["wait-ssh", "600"]))
            }
            Vm::Tart => {
                if !linux_running()? {
                    exec(&mut tart(&["set", LINUX_VM, "--display", LINUX_DISPLAY]))?;
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
        match &self.vm {
            Vm::Script(vm) => vm.stop(),
            Vm::Tart => {
                if linux_running()? {
                    // `tart stop` cuts the power, which loses what the guest wrote last.
                    let _ = self.script("sync; sudo systemctl poweroff");
                    let start = Instant::now();
                    while linux_running()? && start.elapsed() < Duration::from_secs(90) {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    if linux_running()? {
                        exec(&mut tart(&["stop", LINUX_VM, "--timeout", "60"]))?;
                    }
                }
                eprintln!("stopped");
                Ok(())
            }
        }
    }

    fn status(&self) -> Result {
        match &self.vm {
            Vm::Script(vm) => exec(&mut vm.vm_sh(&["status"])),
            Vm::Tart => {
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
        let arguments: Vec<&str> = [command].into_iter().filter(|c| !c.is_empty()).collect();
        match &self.vm {
            Vm::Script(vm) => {
                let line = vm.vm_sh_line(&[&["ssh"], arguments.as_slice()].concat());
                exec(&mut vm.shell(&line, true))
            }
            Vm::Tart => exec(self.tart_ssh(true)?.args(arguments)),
        }
    }

    fn provision(&self, paths: &Paths) -> Result {
        let hosts = paths.root.join("xtask/hosts");
        match self.os {
            Os::Linux => {
                let dir = linux_fonts(paths)?;
                self.upload(&hosts.join("linux/provision.sh"), "$HOME/asrs-provision.sh")?;
                self.upload(&hosts.join("linux/fonts.sh"), "$HOME/asrs-fonts.sh")?;
                self.script(&format!(
                    "set -e\nexec 9>>$HOME/.asrs-gui.lock\nflock 9\nbash $HOME/asrs-provision.sh\nif [ -e {dir} ]; then gio trash {dir}; fi\nbash $HOME/asrs-fonts.sh $HOME/.cache/asrs-fonts {dir}\nsync"
                ))
            }
            Os::Windows => {
                let sync_dir = windows_aux_dir("sync");
                self.script(&format!(
                    "New-Item -Force -ItemType Directory {sync_dir} | Out-Null"
                ))?;
                self.upload(
                    &hosts.join("windows/provision.ps1"),
                    &format!("{sync_dir}/provision.ps1"),
                )?;
                self.script(&format!("& {sync_dir}/provision.ps1\nexit $LASTEXITCODE"))
            }
        }
    }

    fn sync(&self, paths: &Paths) -> Result {
        let state = self.state_dir(paths);
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
                let remote = format!("{}.sync.tar.gz", self.checkout());
                self.upload(&tarball, &remote)?;
                self.script(&format!(
                    "mkdir -p {0} && tar -xzf {remote} -C {0} && unlink {remote}",
                    self.checkout()
                ))?;
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
                let sync_dir = windows_aux_dir("sync");
                self.script(&format!(
                    "New-Item -Force -ItemType Directory {}, {sync_dir} | Out-Null",
                    self.checkout()
                ))?;
                self.upload(&tarball, &format!("{sync_dir}/asrs.tar.gz"))?;
                let mut body = format!(
                    "tar.exe -xzf {sync_dir}/asrs.tar.gz -C {}\nif ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\nRemove-Item {sync_dir}/asrs.tar.gz\n",
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
        self.upload(&pack_file, &self.pack_path())?;
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
        let state = self.state_dir(paths);
        fs::create_dir_all(&state).map_err(|e| format!("{}: {e}", state.display()))?;
        let tarball = state.join("fetch.tar.gz");
        match self.os {
            Os::Linux => {
                let body = format!("tar -czf - -C {}/target {source}", self.checkout());
                self.script_to(&body, &tarball)?;
            }
            Os::Windows => {
                let sync_dir = windows_aux_dir("sync");
                self.script(&format!(
                    "New-Item -Force -ItemType Directory {sync_dir} | Out-Null\ntar.exe -czf {sync_dir}/fetch.tar.gz -C {}/target {source}\nexit $LASTEXITCODE",
                    self.checkout()
                ))?;
                self.download(&format!("{sync_dir}/fetch.tar.gz"), &tarball)?;
            }
        }
        let dest = paths.root.join("target/parity").join(self.name);
        let count = unpack_parity(&tarball, &dest)?;
        eprintln!("fetched {count} files into {}", dest.display());
        Ok(())
    }

    fn theme(&self, paths: &Paths, dark: bool) -> Result {
        let command = match self.os {
            Os::Linux => format!(
                "xfconf-query -c xsettings -p /Net/ThemeName -s {}",
                if dark { "Adwaita-dark" } else { "Adwaita" }
            ),
            Os::Windows => ["AppsUseLightTheme", "SystemUsesLightTheme"]
                .map(|name| {
                    format!(
                        r"reg add HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize /v {name} /t REG_DWORD /d {} /f",
                        u8::from(!dark)
                    )
                })
                .join(" && "),
        };
        self.gui(paths, &RunOptions::new(self, &command))
    }

    fn screenshot(&self, file: &Path) -> Result {
        let file = std::path::absolute(file).map_err(|e| e.to_string())?;
        match &self.vm {
            Vm::Script(vm) => {
                let shot = format!("{}/run/asrs-shot.png", vm.dir());
                let line = vm.vm_sh_line(&["screenshot", &shot]);
                let script = format!("{line} >/dev/null && cat {}", sh_quote(&shot));
                let out = File::create(&file).map_err(|e| format!("{}: {e}", file.display()))?;
                exec(vm.shell(&script, false).stdout(out))?;
            }
            Vm::Tart => self.script_to(
                "DISPLAY=:0 XAUTHORITY=$HOME/.Xauthority import -window root png:-",
                &file,
            )?,
        }
        println!("{}", file.display());
        Ok(())
    }

    fn gen_vectors(&self, paths: &Paths, ids: &[String]) -> Result {
        self.sync(paths)?;
        let command = format!("cargo xtask oracle gen {}", ids.join(" "));
        self.run(paths, &RunOptions::new(self, &command))?;
        let remote_dir = format!("{}/parity/vectors", self.checkout());
        let state = self.state_dir(paths);
        fs::create_dir_all(&state).map_err(|e| format!("{}: {e}", state.display()))?;
        let manifest = state.join("MANIFEST.json");
        self.download(&format!("{remote_dir}/MANIFEST.json"), &manifest)?;
        let text =
            fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
        let remote = json::parse(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
        let platform = format!("{}-{}", self.os.name(), self.arch);
        crate::vectors::import_platform_vectors(paths, &remote, ids, &platform, |id, dest| {
            self.download(&format!("{remote_dir}/{id}.jsonl.zst"), dest)
        })
    }

    fn record_manifest(&self, paths: &Paths) -> Result {
        if !self.reference {
            return Err(format!("{} is not a reference host", self.name));
        }
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
        let platform = self.os.name();
        canvas::set_manifest_host(paths, platform, description)?;
        eprintln!("host description stored in parity/vectors/canvas/{platform}.json");
        Ok(())
    }

    fn check(&self, paths: &Paths) -> Result {
        self.sync(paths)?;
        self.run(paths, &RunOptions::new(self, &self.setup_command()))?;
        let s1_build = "cargo build --release --manifest-path spikes/s1-gpui-shell/Cargo.toml";
        let dom_text = "cargo xtask host local dom-text";
        let mut steps = vec![
            ("selftest", false, None, "cargo xtask oracle selftest"),
            ("smoke", false, None, "cargo xtask oracle smoke"),
            ("gen --check", true, None, "cargo xtask oracle gen --check"),
            (
                "port build",
                false,
                None,
                "cargo build --workspace --all-targets",
            ),
            ("gui-check", true, None, "cargo xtask host local gui-check"),
            ("3D references", true, None, "cargo xtask field3d ref"),
            ("DOM text", true, None, dom_text),
            ("s1 build", false, None, s1_build),
            ("s1 window", true, None, "cargo xtask host local s1"),
        ];
        if self.os == Os::Linux {
            steps.push(("DOM text, stock fonts", true, Some("stock"), dom_text));
        }
        let mut results = Vec::new();
        for (name, gui, fonts, command) in steps {
            let mut options = RunOptions::new(self, command);
            options.fonts = fonts.or(options.fonts);
            let result = if gui {
                self.gui(paths, &options)
            } else {
                self.run(paths, &options)
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

impl Os {
    fn name(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Linux => "linux",
        }
    }
}

/// Where `fonts.sh` unpacks on the Linux host: one directory per version of the script.
fn linux_fonts(paths: &Paths) -> Result<String> {
    let script = oracle::read(&paths.root.join("xtask/hosts/linux/fonts.sh"))?;
    Ok(format!(
        "$HOME/asrs-fonts/{}",
        &crate::sha256_hex(&script)[..12]
    ))
}

fn linux_gui_script(environment: &str, checkout: &str, command: &str) -> String {
    format!(
        r#"{environment}export DISPLAY=:0 XAUTHORITY=$HOME/.Xauthority DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus
exec 9>>$HOME/.asrs-gui.lock
flock -n 9 || {{ echo 'waiting for the other desktop run' >&2; flock 9; }}
cd {checkout} && {{ {command}
}} 9>&-"#
    )
}

/// The lock is a mutex of the task's own process, not of this SSH session: the desktop run goes
/// on when SSH drops.
fn windows_gui_script(gui_dir: &str, task: &str, batch: &str) -> String {
    let gui = gui_dir.replace('/', "\\");
    format!(
        r#"$ErrorActionPreference = 'Stop'
New-Item -Force -ItemType Directory {gui_dir} | Out-Null
Set-Content -NoNewline -Encoding ascii {gui_dir}/run.cmd @'
{batch}
'@
Set-Content -Encoding ascii {gui_dir}/run.ps1 @'
$mutex = New-Object Threading.Mutex($false, 'Global\asrs-gui')
try {{ [void]$mutex.WaitOne() }} catch [Threading.AbandonedMutexException] {{ }}
cmd.exe /c {gui}\run.cmd
$code = $LASTEXITCODE
$mutex.ReleaseMutex()
Set-Content -Encoding ascii {gui}\exit.txt $code
'@
$action = New-ScheduledTaskAction -Execute powershell.exe -Argument '-NoProfile -ExecutionPolicy Bypass -File {gui}\run.ps1' -WorkingDirectory '{gui}'
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Hours 8) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName {task} -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName {task}
$idle = 0
while (-not (Test-Path {gui_dir}/exit.txt)) {{
    Start-Sleep -Milliseconds 500
    if ((Get-ScheduledTask -TaskName {task}).State -eq 'Ready') {{ $idle++ }} else {{ $idle = 0 }}
    if ($idle -ge 20 -and -not (Test-Path {gui_dir}/exit.txt)) {{
        Get-Content -ErrorAction SilentlyContinue {gui_dir}/last.log
        [Console]::Error.WriteLine("the desktop task stopped without an exit code (is $env:USERNAME logged in to the desktop?)")
        exit 1
    }}
}}
Start-Sleep -Milliseconds 200
Get-Content {gui_dir}/last.log
$code = [int](Get-Content {gui_dir}/exit.txt).Trim()
Unregister-ScheduledTask -TaskName {task} -Confirm:$false
Remove-Item -Recurse -Force {gui_dir}
exit $code"#
    )
}

fn host_dir() -> String {
    std::env::var("ASRS_HOST_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "asrs".to_owned())
}

fn windows_aux_dir(kind: &str) -> String {
    match host_dir().as_str() {
        "asrs" => format!("C:/work/.{kind}"),
        dir => format!("C:/work/.{kind}-{dir}"),
    }
}

fn gui_task() -> String {
    match host_dir().as_str() {
        "asrs" => "asrs-gui".to_owned(),
        dir => format!("asrs-gui-{dir}"),
    }
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
        [cmd] if cmd == "describe" => describe(paths, &out).map(|_| ()),
        [cmd, flag] if cmd == "describe" && flag == "--manifest" => {
            let description = describe(paths, &out)?;
            canvas::set_manifest_host(paths, canvas::platform(), description)
        }
        [cmd, flag] if cmd == "describe" && flag == "--check" => {
            check_description(paths, &describe(paths, &out)?)
        }
        [cmd] if cmd == "dom-text" => dom_text(paths, &out),
        [cmd] if cmd == "colour-patch" => colour_patch(paths, &out),
        [cmd] if cmd == "launch" => launch(paths, &out),
        [cmd] if cmd == "gui-check" => {
            let results = [
                ("describe", describe(paths, &out).map(|_| ())),
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

/// Every text box of Chromium's DOM text scenes is grayscale or LCD as the host's settings say.
/// Grey text on a grey background has a pixel whose channels differ only under LCD.
fn dom_text(paths: &Paths, out: &Path) -> Result {
    let lcd = match std::env::consts::OS {
        "windows" => true,
        "linux" => match font_profile().as_deref() {
            Some("gray") => false,
            Some("stock") => true,
            _ => return Err("FONTCONFIG_FILE is not gray.conf or stock.conf".to_owned()),
        },
        os => return Err(format!("dom-text: {os} has no LCD text to tell apart")),
    };
    canvas::check_fonts(paths)?;
    oracle::build_harness(paths)?;
    let out = out.join(if lcd { "dom-lcd" } else { "dom-gray" });
    let mut failures = Vec::new();
    canvas::dom_scenes(paths, &["dom"], &out)?;
    for scale in canvas::DOM_SCALES {
        for theme in ["light", "dark"] {
            let base = out.join("dom").join(format!("dom-text-{theme}@{scale}x"));
            let boxes = read_json_file(&base.with_extension("json"))?;
            let image = pixel::read_png(&base.with_extension("png"))?;
            let rows = text_modes(&boxes, &image)?;
            let lcd_rows = rows.iter().filter(|(_, lcd)| *lcd).count();
            let wrong: Vec<&str> = rows
                .iter()
                .filter(|(row, mode)| *mode != (lcd && lcd_sized(row, scale)))
                .map(|(row, _)| row.as_str())
                .collect();
            eprintln!(
                "dom-text-{theme}@{scale}x: {lcd_rows} of {} text boxes are LCD",
                rows.len()
            );
            if rows.is_empty() || !wrong.is_empty() {
                failures.push(format!(
                    "dom-text-{theme}@{scale}x: not {}: {}",
                    if lcd { "LCD" } else { "grayscale" },
                    wrong.join(", ")
                ));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// Skia draws no LCD text above 48 device pixels. A row is named by its font size (`25px/0`), or
/// `color/<n>` for 13 px.
pub(crate) fn lcd_sized(row: &str, scale: &str) -> bool {
    let size = match row.split("px").next() {
        Some(row) if row.starts_with("color/") => Some(13.0),
        size => size.and_then(|n| n.parse::<f64>().ok()),
    };
    let scale: f64 = scale.parse().unwrap_or(f64::NAN);
    size.is_some_and(|size| size * scale <= 48.0)
}

/// Whether each text box of a DOM capture is LCD, as the oracle flagged it, checked against the
/// pixels of the grey ones.
pub(crate) fn text_modes(boxes: &Json, image: &Rgba) -> Result<Vec<(String, bool)>> {
    let mut modes = Vec::new();
    for row in boxes.get("rows").and_then(Json::as_array).unwrap_or(&[]) {
        let text = |key| row.get(key).and_then(Json::as_str);
        let id = text("id").ok_or("DOM row without an id")?.to_string_lossy();
        let mode = text("mode").map(|m| m.to_string_lossy());
        let lcd = match mode.as_deref() {
            Some("lcd") => true,
            Some("grayscale") => false,
            _ => return Err(format!("{id}: no antialiasing mode")),
        };
        modes.push((id, lcd));
    }
    for (id, chromatic) in chromatic_rows(boxes, image)? {
        let lcd = modes.iter().find(|(row, _)| *row == id).map(|(_, l)| *l);
        if lcd != Some(chromatic > 0) {
            return Err(format!(
                "{id}: {chromatic} coloured pixels, flagged {lcd:?}"
            ));
        }
    }
    Ok(modes)
}

/// The boxes of grey text, with how many of their pixels are not grey.
fn chromatic_rows(boxes: &Json, image: &Rgba) -> Result<Vec<(String, usize)>> {
    let scale = boxes
        .get("scale")
        .and_then(Json::as_f64)
        .ok_or("DOM capture without a scale")?;
    let rows = boxes
        .get("rows")
        .and_then(Json::as_array)
        .ok_or("DOM capture without rows")?;
    let mut counted = Vec::new();
    for row in rows {
        let id = row
            .get("id")
            .and_then(Json::as_str)
            .ok_or("DOM row without an id")?
            .to_string_lossy();
        if id.starts_with("color/") {
            continue;
        }
        let rect: Vec<f64> = row
            .get("rect")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(Json::as_f64)
            .collect();
        let &[x, y, width, height] = rect.as_slice() else {
            return Err(format!("DOM row {id} without a rectangle"));
        };
        let clamp = |v: f64, max: u32| (v * scale).clamp(0.0, f64::from(max)) as u32;
        let (left, right) = (clamp(x, image.width), clamp(x + width, image.width));
        let (top, bottom) = (clamp(y, image.height), clamp(y + height, image.height));
        let chromatic = (top..bottom)
            .flat_map(|y| (left..right).map(move |x| ((y * image.width + x) * 4) as usize))
            .filter(|&at| {
                let p = &image.data[at..at + 3];
                p[0] != p[1] || p[1] != p[2]
            })
            .count();
        counted.push((id, chromatic));
    }
    Ok(counted)
}

fn check_description(paths: &Paths, description: &Json) -> Result {
    let name = format!("{}.json", canvas::platform());
    let manifest = read_json_file(&paths.root.join("parity/vectors/canvas").join(&name))?;
    let recorded = manifest
        .get("host")
        .ok_or_else(|| format!("{name} records no host"))?;
    let mut differences = Vec::new();
    description_differences("", recorded, description, &mut differences);
    if differences.is_empty() {
        eprintln!("the host equals its description in {name}");
        Ok(())
    } else {
        Err(format!(
            "the host differs from its description in {name}:\n  {}",
            differences.join("\n  ")
        ))
    }
}

fn description_differences(at: &str, recorded: &Json, here: &Json, out: &mut Vec<String>) {
    match (recorded, here) {
        (Json::Object(a), Json::Object(b)) => {
            let keys: BTreeSet<String> = a
                .iter()
                .chain(b)
                .map(|(k, _)| k.to_string_lossy())
                .collect();
            for key in keys {
                let field = |v: &Json| v.get(&key).cloned().unwrap_or(Json::Null);
                let at = format!("{at}/{key}");
                description_differences(&at, &field(recorded), &field(here), out);
            }
        }
        _ if recorded == here => {}
        _ => out.push(format!(
            "{at}: recorded {}, here {}",
            recorded.to_pretty(),
            here.to_pretty()
        )),
    }
}

fn describe(paths: &Paths, out: &Path) -> Result<Json> {
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
        ("fontRules", font_rules()?),
        ("updates", updates()),
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
    Ok(description)
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
            Json::string(if icc.is_empty() || !icc.contains(" = ") {
                "none (sRGB)"
            } else {
                "_ICC_PROFILE set on the root window"
            })
        }
        "windows" => {
            let powershell = |command: &str| {
                command_text("powershell", &["-NoProfile", "-Command", command]).unwrap_or_default()
            };
            let desktop = |name: &str| {
                powershell(&format!(
                    "(Get-ItemProperty 'HKCU:/Control Panel/Desktop').{name}"
                ))
            };
            let dwrite =
                "[string](Get-Item C:/Windows/System32/DWrite.dll).VersionInfo.FileVersionRaw";
            let profiles = "(Get-ChildItem C:/Windows/System32/spool/drivers/color -Filter *.ic* | Select-Object -ExpandProperty Name) -join ','";
            let tuner = "(Get-ChildItem HKCU:/Software/Microsoft/Avalon.Graphics -ErrorAction SilentlyContinue | Select-Object -ExpandProperty PSChildName) -join ','";
            let mut fields = vec![
                ("logPixels", desktop("LogPixels")),
                ("iccProfiles", powershell(profiles)),
                ("dwrite", powershell(dwrite)),
                ("clearTypeTunerDisplays", powershell(tuner)),
            ];
            for name in [
                "FontSmoothing",
                "FontSmoothingType",
                "FontSmoothingOrientation",
                "FontSmoothingGamma",
            ] {
                fields.push((name, desktop(name)));
            }
            let fields: Vec<_> = fields
                .iter()
                .map(|(name, value)| (*name, Json::string(value)))
                .collect();
            Json::object(fields)
        }
        _ => Json::Null,
    }
}

pub(crate) fn font_profile() -> Option<String> {
    let file = PathBuf::from(std::env::var_os("FONTCONFIG_FILE")?);
    Some(file.file_stem()?.to_str()?.to_owned())
}

/// What `fonts.sh` unpacked: its packages and rules, and the profile in use.
fn font_rules() -> Result<Json> {
    let Some(file) = std::env::var_os("FONTCONFIG_FILE").map(PathBuf::from) else {
        return Ok(Json::Null);
    };
    let dir = file.parent().ok_or("FONTCONFIG_FILE has no directory")?;
    let Ok(packages) = fs::read_to_string(dir.join("packages.txt")) else {
        return Ok(Json::Null);
    };
    let rules_dir = dir.join("conf.d");
    let mut rules: Vec<PathBuf> = fs::read_dir(&rules_dir)
        .map_err(|e| format!("{}: {e}", rules_dir.display()))?
        .filter_map(|e| Some(e.ok()?.path()))
        .collect();
    rules.sort();
    let profile = String::from_utf8_lossy(&oracle::read(&file)?).into_owned();
    let mut bytes = profile.replace(&dir.display().to_string(), "").into_bytes();
    for rule in &rules {
        bytes.extend(oracle::read(rule)?);
    }
    let packages = packages.lines().filter_map(|l| l.split_once(' '));
    let text = |t: Option<String>| t.map_or(Json::Null, |t| Json::string(&t));
    Ok(Json::object([
        ("profile", text(font_profile())),
        ("rules", Json::string(&crate::sha256_hex(&bytes))),
        (
            "fontconfig",
            text(command_text(
                "dpkg-query",
                &["-W", "-f", "${Version}", "libfontconfig1"],
            )),
        ),
        (
            "packages",
            Json::object(packages.map(|(hash, file)| (file, Json::string(hash)))),
        ),
    ]))
}

/// What keeps the host's operating system at its recorded state.
fn updates() -> Json {
    match std::env::consts::OS {
        "linux" => {
            let units = [
                "apt-daily.timer",
                "apt-daily-upgrade.timer",
                "unattended-upgrades.service",
            ];
            // A masked unit makes is-enabled exit nonzero.
            let state = |unit: &str| {
                let output = Command::new("systemctl")
                    .args(["is-enabled", unit])
                    .output();
                output.map_or(Json::Null, |o| {
                    Json::string(String::from_utf8_lossy(&o.stdout).trim())
                })
            };
            Json::object(units.map(|unit| (unit, state(unit))))
        }
        "windows" => {
            let script = r"$v = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
[string]::Concat('build=', $v.CurrentBuild, '.', $v.UBR)
foreach ($s in 'wuauserv', 'UsoSvc', 'WaaSMedicSvc', 'InstallService') {
  $s + '=' + (Get-ItemProperty HKLM:\SYSTEM\CurrentControlSet\Services\$s).Start
}
$wu = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate'
foreach ($n in 'WUServer', 'WUStatusServer', 'DoNotConnectToWindowsUpdateInternetLocations', 'ExcludeWUDriversInQualityUpdate', 'TargetReleaseVersionInfo') { $n + '=' + (Get-ItemProperty $wu -ErrorAction SilentlyContinue).$n }
foreach ($n in 'UseWUServer', 'NoAutoUpdate') { $n + '=' + (Get-ItemProperty $wu\AU -ErrorAction SilentlyContinue).$n }
$store = Get-ItemProperty HKLM:\SOFTWARE\Policies\Microsoft\WindowsStore -ErrorAction SilentlyContinue
'StoreAutoDownload=' + $store.AutoDownload
'RemoveWindowsStore=' + $store.RemoveWindowsStore";
            let text =
                command_text("powershell", &["-NoProfile", "-Command", script]).unwrap_or_default();
            Json::object(
                text.lines()
                    .filter_map(|l| l.trim().split_once('='))
                    .map(|(name, value)| (name, Json::string(value))),
            )
        }
        _ => Json::Null,
    }
}

// fontconfig finds the pinned files under the directory fonts.sh unpacked to.
fn font_key(path: &str) -> &str {
    path.find("/usr/share/fonts/")
        .map_or(path, |at| &path[at..])
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
            "seguibl.ttf",
            "arial.ttf",
            "arialbd.ttf",
            "ariali.ttf",
            "cour.ttf",
            "courbd.ttf",
            "consola.ttf",
            "consolab.ttf",
            "times.ttf",
            "timesbd.ttf",
            "timesi.ttf",
            "tahoma.ttf",
            "tahomabd.ttf",
            "malgun.ttf",
            "malgunbd.ttf",
            "simsunb.ttf",
            "l_10646.ttf",
            "cambria.ttc",
            "msyh.ttc",
            "msyhbd.ttc",
            "msyhl.ttc",
            "YuGothB.ttc",
            "YuGothL.ttc",
            "YuGothR.ttc",
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
                "monospace:italic",
                "Courier",
                "Courier:bold",
                "Arial",
                "Arial:bold",
                "Arial:italic",
                "Arial:bold:italic",
                "Times New Roman",
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
            (font_key(&f).to_owned(), hash)
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
    fn every_host_has_its_own_port_tools_and_name() {
        for (i, host) in HOSTS.iter().enumerate() {
            assert!(HOSTS[i + 1..].iter().all(|h| h.name != host.name));
            let node = match host.arch {
                "x86_64" => "win-x64",
                _ => "win-arm64",
            };
            assert_eq!(host.path().contains(node), host.os == Os::Windows);
        }
        for os in [Os::Windows, Os::Linux] {
            let references = HOSTS.iter().filter(|h| h.os == os && h.reference);
            assert_eq!(references.count(), 1);
        }
    }

    #[test]
    fn run_options_pick_fonts_and_rasterizer() {
        let args = |line: &str| -> Vec<String> { line.split(' ').map(str::to_owned).collect() };
        let [windows, _, linux] = &HOSTS;
        let options = RunOptions::parse(linux, &args("--fonts stock cargo test --gl")).unwrap();
        assert_eq!(options.fonts, Some("stock"));
        assert_eq!(options.gl, "swiftshader");
        assert_eq!(options.command, "cargo test --gl");
        let options = RunOptions::parse(linux, &args("cargo test")).unwrap();
        assert_eq!(options.fonts, Some("gray"));
        let options = RunOptions::parse(windows, &args("cargo run")).unwrap();
        assert_eq!((options.fonts, options.gl), (None, "d3d11-warp"));
        let options = RunOptions::parse(windows, &args("--gl swiftshader cargo run")).unwrap();
        assert_eq!(options.gl, "swiftshader");
        assert!(RunOptions::parse(windows, &args("--fonts gray cargo run")).is_err());
        assert!(RunOptions::parse(linux, &args("--fonts lcd cargo run")).is_err());
        assert!(RunOptions::parse(linux, &[]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_vm_lock_is_only_taken_and_given_back_by_its_owner() {
        let dir = std::env::temp_dir().join(format!("asrs host-lock-{}", std::process::id()));
        fs::create_dir_all(dir.join(VM_LOCK)).unwrap();
        let path = dir.display().to_string();
        let run = |script: &str| {
            let output = Command::new("sh").arg("-c").arg(script).output().unwrap();
            output.status.success()
        };
        assert!(!run(&lock_script(&path, "a")));
        fs::remove_dir(dir.join(VM_LOCK)).unwrap();
        assert!(run(&lock_script(&path, "a")));
        assert!(run(&lock_script(&path, "a")));
        assert!(!run(&lock_script(&path, "b")));
        let stopped = dir.join("stopped");
        let stop = format!("touch {}", sh_quote(&stopped.display().to_string()));
        assert!(!run(&unlock_script(&path, "b", &stop)));
        assert!(!stopped.exists());
        let trash = "gio() { mv \"$2\" \"$2.gone\"; }\n";
        assert!(run(&format!("{trash}{}", unlock_script(&path, "a", &stop))));
        assert!(stopped.exists() && !dir.join(VM_LOCK).exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_runs_queue() {
        let home = std::env::temp_dir().join(format!("asrs-host-gui-{}", std::process::id()));
        fs::create_dir_all(&home).unwrap();
        let script = linux_gui_script("", "$HOME", "echo start >> log; sleep 0.2; echo end >> log");
        let children: Vec<_> = (0..3)
            .map(|_| {
                let mut command = Command::new("bash");
                command.arg("-c").arg(&script).env("HOME", &home);
                command.stderr(Stdio::null()).spawn().unwrap()
            })
            .collect();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(
            fs::read_to_string(home.join("log")).unwrap(),
            "start\nend\n".repeat(3)
        );
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn description_differences_name_the_field() {
        let parse = |text: &str| json::parse(text).unwrap();
        let differences = |a: &str, b: &str| {
            let mut out = Vec::new();
            description_differences("", &parse(a), &parse(b), &mut out);
            out
        };
        assert!(differences(r#"{"a":{"b":1},"c":2}"#, r#"{"c":2,"a":{"b":1}}"#).is_empty());
        let found = differences(
            r#"{"a":{"b":"x"},"gone":1}"#,
            r#"{"a":{"b":"y"},"new":[2]}"#,
        );
        assert_eq!(found.len(), 3);
        assert!(found[0].starts_with("/a/b: recorded \"x\", here \"y\""));
    }

    #[test]
    fn grey_text_is_told_from_lcd_text() {
        let mut image = Rgba {
            width: 4,
            height: 2,
            data: [128, 128, 128, 255].repeat(8),
        };
        let boxes = json::parse(
            r#"{"scale":2,"rows":[{"id":"a","rect":[0,0,1,1]},{"id":"b","rect":[1,0,1,1]},{"id":"color/0","rect":[0,0,2,1]}]}"#,
        )
        .unwrap();
        image.data[4 * 3] = 129;
        assert_eq!(
            chromatic_rows(&boxes, &image).unwrap(),
            [("a".to_owned(), 0), ("b".to_owned(), 1)]
        );
    }

    #[test]
    fn font_files_are_named_without_the_unpacked_directory() {
        assert_eq!(
            font_key("/home/admin/asrs-fonts/1/root/usr/share/fonts/truetype/a.ttf"),
            "/usr/share/fonts/truetype/a.ttf"
        );
        assert_eq!(
            font_key("C:/Windows/Fonts/arial.ttf"),
            "C:/Windows/Fonts/arial.ttf"
        );
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
