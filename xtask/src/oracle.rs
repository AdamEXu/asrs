use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{Result, canvas, vectors, workspace_root};

pub(crate) const ELECTRON_VERSION: &str = "39.2.7";
const EMSDK_VERSION: &str = "4.0.12";
const UPSTREAM_COMMIT: &str = "8fc5b89fc93be5dba5a66005ea6111fa16594aa8";
const UPSTREAM_REPO: &str = "https://github.com/Mechanical-Advantage/AdvantageScope.git";
const EMSDK_REPO: &str = "https://github.com/emscripten-core/emsdk.git";

const USAGE: &str = "usage: cargo xtask oracle <command>

commands:
  setup [--force]     build the oracle from a clean checkout, then run selftest and smoke.
                      Steps (each skipped when its inputs are unchanged, unless --force):
                        1. sync reference/AdvantageScope into target/oracle/upstream
                        2. npm ci --ignore-scripts there, then upstream's getLicenses.mjs
                        3. download the Electron 39.2.7 binary
                        4. upstream's rolldown -c (real app bundles in upstream/bundles)
                        5. emsdk 4.0.12 into the cache, then upstream's wasmCompile.mjs
                        6. npm ci for oracle/package.json in target/oracle/harness
                        7. esbuild the harness entry points into target/oracle/dist
  build               rebuild only the harness bundles (step 7)
  gen [<id>...] [--check]
                      regenerate committed vectors from parity/specs into parity/vectors and
                      update parity/vectors/MANIFEST.json; --check regenerates in memory and
                      fails if any committed vector differs
  fn [args]           run the fn oracle: JSONL requests on stdin, CVE-1 results on stdout
                      (`cargo xtask oracle fn --help` explains the protocol)
  run <entry> [args]  run target/oracle/dist/<entry>.mjs under the oracle runtime
  canvas [<scene>...] Chromium canvas references (same as `cargo xtask canvas ref`)
  canvas-lut [--check]
                      mask-gamma LUT for this platform (same as `cargo xtask canvas lut`)
  selftest            CVE-1 encoder and decoder self test
  smoke               run oracle/smoke/fn-requests.jsonl and compare with fn-expected.jsonl
  env                 print the oracle paths as JSON

Every command runs `ELECTRON_RUN_AS_NODE=1 <electron> <bundle>`, so V8 and ICU are the
shipped app's own. Downloads (npm, Electron, emsdk) go to $ASRS_CACHE, which defaults to
<real target dir>/../cache when target/ is a symlink and to target/cache otherwise.";

pub(crate) struct Paths {
    pub(crate) root: PathBuf,
    pub(crate) reference: PathBuf,
    pub(crate) oracle_src: PathBuf,
    upstream: PathBuf,
    harness: PathBuf,
    pub(crate) dist: PathBuf,
    stamps: PathBuf,
    cache: PathBuf,
}

impl Paths {
    pub(crate) fn new() -> Result<Self> {
        let root = workspace_root();
        let target = root.join("target");
        let cache = match std::env::var_os("ASRS_CACHE") {
            Some(dir) => PathBuf::from(dir),
            None if target.is_symlink() => {
                let real = fs::canonicalize(&target).map_err(|e| format!("target/: {e}"))?;
                real.parent()
                    .map_or_else(|| target.join("cache"), |p| p.join("cache"))
            }
            None => target.join("cache"),
        };
        let out = target.join("oracle");
        Ok(Self {
            reference: root.join("reference/AdvantageScope"),
            oracle_src: root.join("oracle"),
            upstream: out.join("upstream"),
            harness: out.join("harness"),
            dist: out.join("dist"),
            stamps: out.join("stamps"),
            cache,
            root,
        })
    }

    fn emsdk(&self) -> PathBuf {
        self.cache.join("emsdk")
    }

    pub(crate) fn electron(&self) -> Result<PathBuf> {
        let package = self.upstream.join("node_modules/electron");
        let rel = fs::read_to_string(package.join("path.txt")).map_err(|_| {
            "Electron binary is not installed; run `cargo xtask oracle setup`".to_owned()
        })?;
        Ok(package.join("dist").join(rel.trim()))
    }
}

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("setup") => setup(&paths, rest.iter().any(|a| a == "--force")),
        Some("build") => build_harness(&paths),
        Some("gen") => vectors::generate(&paths, rest),
        Some("fn") => run_entry(&paths, "fn", rest),
        Some("run") => match rest.split_first() {
            Some((entry, entry_args)) => run_entry(&paths, entry, entry_args),
            None => Err(USAGE.to_owned()),
        },
        Some("canvas") => canvas::reference(&paths, &canvas::reference_dir(&paths), rest),
        Some("canvas-lut") => canvas::lut(&paths, rest.iter().any(|a| a == "--check")),
        Some("selftest") => run_entry(&paths, "selftest", &[]),
        Some("smoke") => smoke(&paths),
        Some("env") => print_env(&paths),
        _ => Err(USAGE.to_owned()),
    }
}

fn setup(paths: &Paths, force: bool) -> Result {
    check_reference(paths)?;
    fs::create_dir_all(&paths.stamps).map_err(|e| e.to_string())?;
    fs::create_dir_all(&paths.cache).map_err(|e| e.to_string())?;

    step("sync upstream");
    let changed = sync_upstream(&paths.reference, &paths.upstream)?;
    eprintln!("  {changed} files updated");

    let lock = read(&paths.upstream.join("package-lock.json"))?;
    stamped(paths, "npm-upstream", &hash_bytes(&lock), force, || {
        step("npm ci (upstream)");
        npm_ci(paths, &paths.upstream)
    })?;

    let licenses = paths.upstream.join("src/licenses.json");
    if force || !licenses.exists() {
        step("getLicenses.mjs");
        exec(node().arg("getLicenses.mjs").current_dir(&paths.upstream))?;
    }

    let electron_ok = paths
        .electron()
        .ok()
        .filter(|bin| bin.exists())
        .and_then(|_| {
            fs::read_to_string(paths.upstream.join("node_modules/electron/dist/version")).ok()
        })
        .is_some_and(|v| v.trim().trim_start_matches('v') == ELECTRON_VERSION);
    if force || !electron_ok {
        step("download Electron");
        exec(
            node()
                .arg("node_modules/electron/install.js")
                .env("electron_config_cache", paths.cache.join("electron"))
                .env("force_no_cache", if force { "true" } else { "false" })
                .current_dir(&paths.upstream),
        )?;
    }
    check_electron_version(paths)?;

    let bundle_inputs = hash_tree(
        &paths.upstream,
        &[
            "src",
            "rolldown.config.mjs",
            "package-lock.json",
            "tsconfig.json",
        ],
    )?;
    let bundles_missing = !paths.upstream.join("bundles/hub.js").exists();
    stamped(
        paths,
        "rolldown",
        &bundle_inputs,
        force || bundles_missing,
        || {
            step("rolldown -c");
            exec(
                Command::new(paths.upstream.join("node_modules/.bin/rolldown"))
                    .arg("-c")
                    .env("NO_COLOR", "1")
                    .current_dir(&paths.upstream),
            )
        },
    )?;

    install_emsdk(paths, force)?;
    let wasm_inputs = format!(
        "{EMSDK_VERSION}:{}",
        hash_bytes(&read(
            &paths
                .upstream
                .join("src/hub/dataSources/wpilog/indexer/wpilogIndexer.c")
        )?)
    );
    let wasm_missing = !paths.upstream.join("bundles/wpilogIndexer.wasm").exists();
    stamped(paths, "wasm", &wasm_inputs, force || wasm_missing, || {
        step("wasmCompile.mjs");
        let script = format!(
            "source {} >/dev/null 2>&1 && node wasmCompile.mjs",
            shell_quote(&paths.emsdk().join("emsdk_env.sh"))
        );
        exec(
            Command::new("bash")
                .args(["-c", &script])
                .current_dir(&paths.upstream),
        )
    })?;

    fs::create_dir_all(&paths.harness).map_err(|e| e.to_string())?;
    for file in ["package.json", "package-lock.json"] {
        fs::copy(paths.oracle_src.join(file), paths.harness.join(file))
            .map_err(|e| format!("copying oracle/{file}: {e}"))?;
    }
    let harness_lock = read(&paths.harness.join("package-lock.json"))?;
    stamped(
        paths,
        "npm-harness",
        &hash_bytes(&harness_lock),
        force,
        || {
            step("npm ci (harness)");
            npm_ci(paths, &paths.harness)
        },
    )?;

    build_harness(paths)?;
    run_entry(paths, "selftest", &[])?;
    smoke(paths)?;
    eprintln!("oracle setup complete");
    print_env(paths)
}

fn check_reference(paths: &Paths) -> Result {
    let reference = &paths.reference;
    let clone_hint = format!(
        "  git clone {UPSTREAM_REPO} {dir}\n  git -C {dir} checkout {UPSTREAM_COMMIT}",
        dir = reference.display()
    );
    if !reference.is_dir() {
        return Err(format!(
            "upstream checkout not found at {}; create it with\n{clone_hint}",
            reference.display()
        ));
    }
    let git = |args: &[&str]| -> Result<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(reference)
            .args(args)
            .output()
            .map_err(|e| format!("running git: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "{} is not a git checkout of upstream; create it with\n{clone_hint}",
                reference.display()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let head = git(&["rev-parse", "HEAD"])?;
    if head != UPSTREAM_COMMIT {
        return Err(format!(
            "{} is at {head}, but the oracle is pinned to {UPSTREAM_COMMIT}; run\n  git -C {} checkout {UPSTREAM_COMMIT}",
            reference.display(),
            reference.display()
        ));
    }
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.is_empty() {
        return Err(format!(
            "{} has local modifications; the oracle must run upstream unmodified:\n{dirty}",
            reference.display()
        ));
    }
    Ok(())
}

fn step(name: &str) {
    eprintln!("==> {name}");
}

fn stamped(
    paths: &Paths,
    name: &str,
    inputs: &str,
    force: bool,
    action: impl FnOnce() -> Result,
) -> Result {
    let stamp = paths.stamps.join(name);
    if !force && fs::read_to_string(&stamp).is_ok_and(|s| s == inputs) {
        eprintln!("==> {name}: up to date");
        return Ok(());
    }
    let _ = fs::remove_file(&stamp);
    action()?;
    fs::write(&stamp, inputs).map_err(|e| format!("{}: {e}", stamp.display()))
}

fn install_emsdk(paths: &Paths, force: bool) -> Result {
    if cfg!(windows) {
        return Err("emsdk setup is only scripted for macOS and Linux so far".to_owned());
    }
    let emsdk = paths.emsdk();
    if !emsdk.join("emsdk").exists() {
        step("clone emsdk");
        exec(
            Command::new("git")
                .args([
                    "clone",
                    "--depth",
                    "1",
                    "--branch",
                    EMSDK_VERSION,
                    EMSDK_REPO,
                ])
                .arg(&emsdk),
        )?;
    }
    let activated = emsdk.join(format!(".asrs-activated-{EMSDK_VERSION}"));
    if force || !activated.exists() || !emsdk.join("upstream/emscripten/emcc").exists() {
        step("emsdk install");
        exec(
            Command::new("./emsdk")
                .args(["install", EMSDK_VERSION])
                .current_dir(&emsdk),
        )?;
        exec(
            Command::new("./emsdk")
                .args(["activate", EMSDK_VERSION])
                .current_dir(&emsdk),
        )?;
        fs::write(&activated, "").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn npm_ci(paths: &Paths, dir: &Path) -> Result {
    exec(
        Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
            .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
            .env("npm_config_cache", paths.cache.join("npm"))
            .env("npm_config_update_notifier", "false")
            .current_dir(dir),
    )
}

pub(crate) fn build_harness(paths: &Paths) -> Result {
    step("build harness");
    exec(
        node()
            .arg(paths.oracle_src.join("build.mjs"))
            .arg("--upstream")
            .arg(&paths.upstream)
            .arg("--harness")
            .arg(&paths.harness)
            .arg("--out")
            .arg(&paths.dist),
    )
}

pub(crate) fn oracle_command(paths: &Paths, entry: &str) -> Result<Command> {
    let bundle = paths.dist.join(format!("{entry}.mjs"));
    if !bundle.exists() {
        return Err(format!(
            "{} is missing; run `cargo xtask oracle setup`",
            bundle.display()
        ));
    }
    let mut command = Command::new(paths.electron()?);
    command.arg(bundle).env("ELECTRON_RUN_AS_NODE", "1");
    Ok(command)
}

fn run_entry(paths: &Paths, entry: &str, args: &[String]) -> Result {
    exec(oracle_command(paths, entry)?.args(args))
}

fn check_electron_version(paths: &Paths) -> Result {
    let output = Command::new(paths.electron()?)
        .args(["-p", "process.versions.electron"])
        .env("ELECTRON_RUN_AS_NODE", "1")
        .output()
        .map_err(|e| format!("running Electron: {e}"))?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version == ELECTRON_VERSION {
        Ok(())
    } else {
        Err(format!(
            "expected Electron {ELECTRON_VERSION}, runtime reports {version:?}"
        ))
    }
}

fn smoke(paths: &Paths) -> Result {
    let smoke_dir = paths.oracle_src.join("smoke");
    let requests = read(&smoke_dir.join("fn-requests.jsonl"))?;
    let request_text = String::from_utf8_lossy(&requests).into_owned();
    let expected = String::from_utf8(read(&smoke_dir.join("fn-expected.jsonl"))?)
        .map_err(|e| e.to_string())?;
    let actual = pipe(&mut oracle_command(paths, "fn")?, requests)?;
    let request_lines: Vec<&str> = request_text.lines().collect();
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let mut failures = 0;
    for (i, want) in expected.iter().enumerate() {
        let got = actual.get(i).copied().unwrap_or("<missing>");
        if got != *want {
            failures += 1;
            eprintln!(
                "smoke {}: {}\n  expected {want}\n  actual   {got}",
                i + 1,
                request_lines.get(i).copied().unwrap_or("?")
            );
        }
    }
    if actual.len() != expected.len() {
        failures += 1;
        eprintln!(
            "smoke: expected {} responses, got {}",
            expected.len(),
            actual.len()
        );
    }
    if failures == 0 {
        eprintln!(
            "fn smoke: {}/{} calls match",
            expected.len(),
            expected.len()
        );
        Ok(())
    } else {
        Err(format!("fn smoke: {failures} mismatches"))
    }
}

/// Runs `command` with `input` on stdin and returns its stdout; stderr passes through.
pub(crate) fn pipe(command: &mut Command, input: Vec<u8>) -> Result<String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting {program}: {e}"))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    writer
        .join()
        .map_err(|_| "stdin writer panicked".to_owned())?
        .map_err(|e| format!("writing to {program}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{program} exited with {}", output.status));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("{program} output: {e}"))
}

fn print_env(paths: &Paths) -> Result {
    let electron = paths.electron().unwrap_or_default();
    let json_path = |p: &Path| json_string(&p.display().to_string());
    println!(
        "{{\n  \"electron\": {},\n  \"electronVersion\": {},\n  \"upstream\": {},\n  \"dist\": {},\n  \"cache\": {},\n  \"root\": {}\n}}",
        json_path(&electron),
        json_string(ELECTRON_VERSION),
        json_path(&paths.upstream),
        json_path(&paths.dist),
        json_path(&paths.cache),
        json_path(&paths.root)
    );
    Ok(())
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn node() -> Command {
    Command::new("node")
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

pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(0xcbf2_9ce4_8422_2325, bytes))
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

pub(crate) fn hash_tree(base: &Path, roots: &[&str]) -> Result<String> {
    let mut files = Vec::new();
    for root in roots {
        collect_files(base, &base.join(root), &mut files)?;
    }
    files.sort();
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for rel in files {
        hash = fnv1a(hash, rel.as_bytes());
        hash = fnv1a(hash, &read(&base.join(&rel))?);
    }
    Ok(format!("{hash:016x}"))
}

fn collect_files(base: &Path, path: &Path, out: &mut Vec<String>) -> Result {
    let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_file() {
        let rel = path.strip_prefix(base).expect("path is under base");
        out.push(rel.to_string_lossy().replace('\\', "/"));
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        collect_files(base, &entry.path(), out)?;
    }
    Ok(())
}

const SYNC_SKIPPED: &[&str] = &[".git", "node_modules"];
const SYNC_GENERATED: &[&str] = &[
    "node_modules",
    "bundles",
    "lite/static/bundles",
    "src/licenses.json",
    "ThirdPartyLicenses.txt",
];

fn sync_upstream(src: &Path, dst: &Path) -> Result<usize> {
    let mut seen = HashSet::new();
    let changed = copy_newer(src, dst, Path::new(""), &mut seen)?;
    remove_stale(dst, Path::new(""), &seen)?;
    Ok(changed)
}

fn copy_newer(src: &Path, dst: &Path, rel: &Path, seen: &mut HashSet<PathBuf>) -> Result<usize> {
    let from_dir = src.join(rel);
    let err = |e: std::io::Error| format!("{}: {e}", from_dir.display());
    fs::create_dir_all(dst.join(rel)).map_err(err)?;
    let mut changed = 0;
    for entry in fs::read_dir(&from_dir).map_err(err)? {
        let entry = entry.map_err(err)?;
        let name = entry.file_name();
        if SYNC_SKIPPED.iter().any(|s| name == *s) {
            continue;
        }
        let child = rel.join(&name);
        seen.insert(child.clone());
        let file_type = entry.file_type().map_err(err)?;
        let from = src.join(&child);
        let to = dst.join(&child);
        if file_type.is_symlink() {
            changed += copy_symlink(&from, &to)?;
            continue;
        }
        if file_type.is_dir() {
            changed += copy_newer(src, dst, &child, seen)?;
            continue;
        }
        let src_meta = entry.metadata().map_err(err)?;
        let up_to_date = fs::metadata(&to).is_ok_and(|dst_meta| {
            dst_meta.len() == src_meta.len()
                && matches!((dst_meta.modified(), src_meta.modified()), (Ok(d), Ok(s)) if d >= s)
        });
        if !up_to_date {
            fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
            changed += 1;
        }
    }
    Ok(changed)
}

fn copy_symlink(from: &Path, to: &Path) -> Result<usize> {
    let err = |e: std::io::Error| format!("{}: {e}", from.display());
    let link = fs::read_link(from).map_err(err)?;
    if fs::read_link(to).is_ok_and(|existing| existing == link) {
        return Ok(0);
    }
    if fs::symlink_metadata(to).is_ok() {
        let result = if fs::symlink_metadata(to).is_ok_and(|m| m.is_dir()) {
            fs::remove_dir_all(to)
        } else {
            fs::remove_file(to)
        };
        result.map_err(|e| format!("{}: {e}", to.display()))?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&link, to).map_err(err)?;
    #[cfg(windows)]
    return Err(format!(
        "{}: symlinks in upstream are not supported on Windows",
        from.display()
    ));
    Ok(1)
}

fn remove_stale(dst: &Path, rel: &Path, seen: &HashSet<PathBuf>) -> Result {
    let dir = dst.join(rel);
    for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let child = rel.join(entry.file_name());
        let child_str = child.to_string_lossy().replace('\\', "/");
        if SYNC_GENERATED.contains(&child_str.as_str()) {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map_err(|e| e.to_string())?.is_dir();
        if !seen.contains(&child) {
            let result = if is_dir {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            result.map_err(|e| format!("removing stale {}: {e}", path.display()))?;
        } else if is_dir {
            remove_stale(dst, &child, seen)?;
        }
    }
    Ok(())
}
