use std::fs;
use std::path::{Path, PathBuf};

use asrs_parity::json::{self, Json};
use asrs_parity::utf16::Utf16String;
use asrs_parity::{Response, diff, vectors};

use crate::Result;
use crate::oracle::{self, Paths};
use crate::{corpus, public};

const MANIFEST: &str = "MANIFEST.json";
const FN_SPEC: &str = ".jsonl";
const FN_ZONED_SPEC: &str = ".fn.json";
const REQUESTS: &str = ".requests.jsonl";
const ENTRY_SPEC: &str = ".entry.json";
const SESSION_SPEC: &str = ".session.json";
const SESSION_ENTRY: &str = "liveSession";
/// Names the spec file for an entry run, so an entry can read its fixture from the spec itself.
const SPEC_VARIABLE: &str = "ASRS_ORACLE_SPEC";
/// Set to `1` for a local spec's run; the oracle reads logs without a redistribution license
/// only then.
const LOCAL_VARIABLE: &str = "ASRS_ORACLE_LOCAL";
/// Local vectors live in `$ASRS_CACHE/vectors`, next to the public logs they come from.
const LOCAL_VECTORS: &str = "vectors";
const REGENERATE: &str = "regenerate with `cargo xtask oracle gen` and commit the result";

pub(crate) fn specs_dir(root: &Path) -> PathBuf {
    root.join("parity/specs")
}

enum Kind {
    Fn {
        requests: PathBuf,
    },
    Entry {
        entry: String,
        args: Vec<String>,
        inputs: Vec<PathBuf>,
    },
}

struct Spec {
    id: String,
    path: PathBuf,
    kind: Kind,
    zones: Option<Vec<String>>,
    platforms: Option<Vec<String>>,
    /// Derived from public logs without a redistribution license, so generated into
    /// `$ASRS_CACHE/vectors` and never committed.
    local: bool,
}

impl Spec {
    fn entry(&self) -> &str {
        match &self.kind {
            Kind::Fn { .. } => "fn",
            Kind::Entry { entry, .. } => entry,
        }
    }

    fn inputs(&self) -> Vec<&Path> {
        match &self.kind {
            Kind::Fn { requests } if requests != &self.path => vec![&self.path, requests],
            Kind::Entry { inputs, .. } => std::iter::once(self.path.as_path())
                .chain(inputs.iter().map(PathBuf::as_path))
                .collect(),
            Kind::Fn { .. } => vec![&self.path],
        }
    }

    /// Synthetic corpora the generator reads, named by an arg or input path through
    /// `corpus/synthetic/<name>` (the generated tree or the committed manifest).
    fn corpora(&self, known: &[String]) -> Vec<String> {
        let Kind::Entry { args, inputs, .. } = &self.kind else {
            return Vec::new();
        };
        let mut found: Vec<String> = args
            .iter()
            .map(String::as_str)
            .chain(inputs.iter().filter_map(|p| p.to_str()))
            .flat_map(|text| text.split("corpus/synthetic/").skip(1))
            .map(|rest| rest.split(['/', '.']).next().unwrap_or_default().to_owned())
            .filter(|name| known.contains(name))
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// One vector per zone or per platform; a spec with neither makes one UTC vector named
    /// after the spec.
    fn vectors(&self) -> Vec<Vector<'_>> {
        let one = |id: String, zone: &str, platform: Option<&String>| Vector {
            id,
            zone: zone.to_owned(),
            platform: platform.cloned(),
            spec: self,
        };
        match (&self.zones, &self.platforms) {
            (Some(zones), _) => zones
                .iter()
                .map(|zone| one(format!("{}/{zone}", self.id), zone, None))
                .collect(),
            (None, Some(platforms)) => platforms
                .iter()
                .map(|p| one(format!("{}/{p}", self.id), oracle::DEFAULT_ZONE, Some(p)))
                .collect(),
            (None, None) => vec![one(self.id.clone(), oracle::DEFAULT_ZONE, None)],
        }
    }
}

struct Vector<'a> {
    id: String,
    zone: String,
    /// Set for a vector of a platform-dependent spec: only a host of that platform can
    /// generate it or check its content.
    platform: Option<String>,
    spec: &'a Spec,
}

impl Vector<'_> {
    fn is_foreign(&self) -> bool {
        self.platform
            .as_ref()
            .is_some_and(|p| *p != oracle::host_tag())
    }
}

/// What the manifest must say about a vector for it to count as current.
#[derive(Debug)]
struct Expected {
    id: String,
    generator: String,
    spec: String,
    zone: String,
    platform: Option<String>,
    committed_content: Option<String>,
}

struct Current {
    commit: String,
    electron: String,
    environment: Json,
}

/// Where vectors are written: `parity/vectors` for committed ones, `$ASRS_CACHE/vectors` for
/// local ones (derived from public logs without a redistribution license).
struct Store {
    dir: PathBuf,
    manifest: Json,
    dirty: bool,
}

impl Store {
    fn open(dir: PathBuf) -> Result<Self> {
        let manifest = read_manifest(&dir)?;
        Ok(Store {
            dir,
            manifest,
            dirty: false,
        })
    }
}

pub(crate) fn generate(paths: &Paths, args: &[String]) -> Result {
    let check = args.iter().any(|a| a == "--check");
    let stale_only = args.iter().any(|a| a == "--stale");
    let local_only = args.iter().any(|a| a == "--local");
    if let Some(flag) = args
        .iter()
        .find(|a| a.starts_with("--") && !["--check", "--stale", "--local"].contains(&a.as_str()))
    {
        return Err(format!("oracle gen: unknown option {flag}"));
    }
    if check && (stale_only || local_only) {
        return Err("oracle gen: --check cannot be combined with --stale or --local".to_owned());
    }
    let wanted: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let specs = discover(&specs_dir(&paths.root))?;
    let all: Vec<Vector<'_>> = specs.iter().flat_map(Spec::vectors).collect();
    for (i, vector) in all.iter().enumerate() {
        if all[..i].iter().any(|v| v.id == vector.id) {
            return Err(format!(
                "oracle gen: two specs produce vector {:?}",
                vector.id
            ));
        }
    }
    for id in &wanted {
        if !all.iter().any(|v| v.id == **id || v.spec.id == **id) {
            return Err(format!(
                "oracle gen: no spec or vector {id:?} under parity/specs"
            ));
        }
    }
    let every = wanted.is_empty();
    let full = every && !stale_only && !local_only;
    let is_wanted = |v: &Vector<'_>| wanted.iter().any(|w| **w == v.id || **w == v.spec.id);
    let mut selected: Vec<&Vector<'_>> = all
        .iter()
        .filter(|v| (every || is_wanted(v)) && (!local_only || v.spec.local))
        .collect();
    if check {
        if let Some(v) = selected.iter().find(|v| v.spec.local && is_wanted(v)) {
            return Err(format!(
                "oracle gen --check: {} is a local vector; --check covers committed vectors only",
                v.id
            ));
        }
        let local = selected.iter().filter(|v| v.spec.local).count();
        if local > 0 {
            eprintln!("  {local} local vectors are not committed and not checked");
        }
        selected.retain(|v| !v.spec.local);
    }
    let missing_logs = public::missing_local_logs(paths)?;
    let mut local_skipped = false;
    if !missing_logs.is_empty() && selected.iter().any(|v| v.spec.local) {
        let fetch = format!(
            "the public logs {} are not in {}; run `cargo xtask corpus fetch`",
            missing_logs.join(", "),
            paths.cache.join("public-logs").display()
        );
        if local_only || selected.iter().any(|v| v.spec.local && is_wanted(v)) {
            return Err(format!("oracle gen: local vectors need {fetch}"));
        }
        eprintln!("warning: skipping the local vectors: {fetch}");
        selected.retain(|v| !v.spec.local);
        local_skipped = true;
    }
    if selected.is_empty() {
        return Err("oracle gen: no specs under parity/specs".to_owned());
    }
    if !check && let Some(v) = selected.iter().find(|v| v.is_foreign() && is_wanted(v)) {
        return Err(format!(
            "oracle gen: {} can only be generated on {}; run `cargo xtask host <name> gen {}` from the dev machine",
            v.id,
            v.platform.as_deref().unwrap_or_default(),
            v.id
        ));
    }

    let install = oracle::installed(paths)?;
    let current = Current {
        commit: install.upstream_commit.clone(),
        electron: oracle::ELECTRON_VERSION.to_owned(),
        environment: oracle::environment_policy(),
    };
    let mut stores = [
        Store::open(paths.root.join("parity/vectors"))?,
        Store::open(paths.cache.join(LOCAL_VECTORS))?,
    ];
    let store_of = |v: &Vector<'_>| usize::from(v.spec.local);
    if !check && !full && selected.iter().any(|v| !v.spec.local) {
        let mismatched = header_mismatches(&stores[0].manifest, &current);
        if !mismatched.is_empty() {
            return Err(format!(
                "oracle gen: the manifest was generated under a different {}; regenerate every vector with `cargo xtask oracle gen` (no ids)",
                mismatched.join(", ")
            ));
        }
    }

    oracle::build_harness(paths)?;
    let entry_hashes = entry_hashes(&paths.oracle_src, &read_entry_inputs(&paths.dist)?)?;
    let mut jobs = Vec::new();
    for vector in selected {
        let generator = generator_hash(&entry_hashes, vector)?;
        jobs.push((vector, generator));
    }

    if check || stale_only {
        let mut expected = Vec::new();
        for (vector, generator) in &jobs {
            let dir = &stores[store_of(vector)].dir;
            expected.push(expected_entry(paths, dir, vector, generator)?);
        }
        if stale_only {
            let keep: Vec<bool> = jobs
                .iter()
                .zip(&expected)
                .map(|((vector, _), want)| {
                    let manifest = &stores[store_of(vector)].manifest;
                    !stale_entries(manifest, &current, std::slice::from_ref(want), false).is_empty()
                })
                .collect();
            let mut keep = keep.into_iter();
            jobs.retain(|_| keep.next().expect("one expectation per vector"));
            eprintln!("  {} stale vectors", jobs.len());
        } else {
            let stale = stale_entries(&stores[0].manifest, &current, &expected, full);
            if !stale.is_empty() {
                return Err(format!(
                    "oracle gen --check: committed vectors are stale ({} problems, oracle not run):\n  {}\n{REGENERATE}",
                    stale.len(),
                    stale.join("\n  ")
                ));
            }
            eprintln!(
                "  manifest current for {} vectors; regenerating to compare content",
                jobs.len()
            );
        }
    }

    let known = corpus::names(&paths.root)?;
    let mut corpora: Vec<String> = jobs
        .iter()
        .filter(|(v, _)| !v.is_foreign())
        .flat_map(|(v, _)| v.spec.corpora(&known))
        .collect();
    corpora.sort();
    corpora.dedup();
    corpus::ensure(paths, &corpora)?;

    let mut failures = Vec::new();
    let mut foreign_stale = Vec::new();
    for (vector, generator) in &jobs {
        if vector.is_foreign() {
            let platform = vector.platform.as_deref().unwrap_or_default();
            if check {
                eprintln!("==> {}: content is checked on {platform} only", vector.id);
            } else if stores[0]
                .manifest
                .get("vectors")
                .and_then(|v| v.get(&vector.id))
                .and_then(|e| string_field(e, "generator"))
                .as_deref()
                != Some(generator.as_str())
            {
                foreign_stale.push(format!(
                    "{} ({platform}): `cargo xtask host <name> gen {}`",
                    vector.id, vector.id
                ));
            }
            continue;
        }
        eprintln!(
            "==> {} ({}{})",
            vector.id,
            vector.zone,
            if vector.spec.local { ", local" } else { "" }
        );
        let text = run_vector(paths, vector)?;
        let cases = text.lines().count();
        if check {
            let committed = vectors::vector_path(&stores[0].dir, &vector.id)
                .ok_or_else(|| format!("{}: no committed vector", vector.id))
                .and_then(|p| vectors::read_text(&p))?;
            match diff::report_jsonl(&committed, &text, 3, diff::report_lines) {
                Some(report) => failures.push(format!(
                    "{}: regenerated vector differs from the committed one\n{report}",
                    vector.id
                )),
                None => eprintln!("  {cases} cases, identical"),
            }
            continue;
        }
        if vector.spec.local {
            remove_vector(&mut stores[0], &vector.id)?;
        }
        let store = &mut stores[store_of(vector)];
        let out = store.dir.join(format!("{}.jsonl.zst", vector.id));
        fs::create_dir_all(out.parent().expect("vector path has a parent"))
            .map_err(|e| e.to_string())?;
        fs::write(&out, vectors::compress(&text)).map_err(|e| format!("{}: {e}", out.display()))?;
        let plain = store.dir.join(format!("{}.jsonl", vector.id));
        if plain.exists() {
            fs::remove_file(&plain).map_err(|e| format!("{}: {e}", plain.display()))?;
        }
        set_entry(
            &mut store.manifest,
            &vector.id,
            Json::object([
                ("cases", Json::Number(cases as f64)),
                (
                    "content",
                    Json::string(&oracle::hash_bytes(text.as_bytes())),
                ),
                ("generator", Json::string(generator)),
                ("spec", Json::string(&spec_rel(paths, vector.spec))),
                ("zone", Json::string(&vector.zone)),
                ("generatedOn", Json::string(&oracle::host_tag())),
            ]),
        );
        store.dirty = true;
        eprintln!(
            "  {cases} cases -> {}",
            out.strip_prefix(&paths.root).unwrap_or(&out).display()
        );
    }

    if check {
        return if failures.is_empty() {
            eprintln!("oracle gen --check: vectors are up to date");
            Ok(())
        } else {
            Err(failures.join("\n\n"))
        };
    }
    if every {
        for (i, store) in stores.iter_mut().enumerate() {
            let local = i == 1;
            if (local && local_skipped) || (!local && local_only) {
                continue;
            }
            let ids: Vec<&str> = all
                .iter()
                .filter(|v| v.spec.local == local)
                .map(|v| v.id.as_str())
                .collect();
            remove_orphans(store, &ids)?;
        }
    }
    for (i, store) in stores.into_iter().enumerate() {
        if (i == 0 && !local_only) || store.dirty {
            fs::create_dir_all(&store.dir).map_err(|e| format!("{}: {e}", store.dir.display()))?;
            write_manifest(&store.dir, &install, store.manifest)?;
        }
    }
    if !foreign_stale.is_empty() {
        eprintln!(
            "warning: these platform vectors are stale and must be regenerated on their host:\n  {}",
            foreign_stale.join("\n  ")
        );
    }
    Ok(())
}

fn expected_entry(
    paths: &Paths,
    vectors_dir: &Path,
    vector: &Vector<'_>,
    generator: &str,
) -> Result<Expected> {
    let committed_content = vectors::vector_path(vectors_dir, &vector.id)
        .map(|p| vectors::read_text(&p))
        .transpose()?
        .map(|text| oracle::hash_bytes(text.as_bytes()));
    Ok(Expected {
        id: vector.id.clone(),
        generator: generator.to_owned(),
        spec: spec_rel(paths, vector.spec),
        zone: vector.zone.clone(),
        platform: vector.platform.clone(),
        committed_content,
    })
}

fn spec_rel(paths: &Paths, spec: &Spec) -> String {
    spec.path
        .strip_prefix(&paths.root)
        .unwrap_or(&spec.path)
        .to_string_lossy()
        .replace('\\', "/")
}

const SHARED_INPUTS: &[&str] = &["build.mjs", "package-lock.json"];
const ENTRY_INPUTS: &str = "inputs.json";
/// Entries that load another entry's page bundle at runtime, as build.mjs's `runtimeLoads`
/// records for its own entries. Kept here because any edit to build.mjs changes every
/// vector's generator hash.
const BORROWED_INPUTS: &[(&str, &str)] = &[("hubSession", "dom")];
const LAZY_HELPER_PREFIX: &str = "lazyHelpers/";

fn read_entry_inputs(dist: &Path) -> Result<Vec<(String, Vec<String>)>> {
    let path = dist.join(ENTRY_INPUTS);
    let json = read_json(&path)?;
    let Json::Object(pairs) = json else {
        return Err(format!("{}: not an object", path.display()));
    };
    let mut inputs = parse_entry_inputs(&path, pairs)?;
    for (entry, lender) in BORROWED_INPUTS {
        let Some(borrowed) = inputs
            .iter()
            .find(|(e, _)| e == lender)
            .map(|(_, f)| f.clone())
        else {
            continue;
        };
        if let Some((_, files)) = inputs.iter_mut().find(|(e, _)| e == entry) {
            for file in borrowed {
                if !files.contains(&file) {
                    files.push(file);
                }
            }
            files.sort();
        }
    }
    Ok(inputs)
}

fn parse_entry_inputs(
    path: &Path,
    pairs: Vec<(Utf16String, Json)>,
) -> Result<Vec<(String, Vec<String>)>> {
    pairs
        .into_iter()
        .map(|(entry, files)| {
            let files = files
                .as_array()
                .and_then(|list| {
                    list.iter()
                        .map(|f| f.as_str().map(|s| s.to_string_lossy()))
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or_else(|| format!("{}: inputs must be lists of strings", path.display()))?;
            Ok((entry.to_string_lossy(), files))
        })
        .collect()
}

fn entry_hashes(
    oracle_src: &Path,
    inputs: &[(String, Vec<String>)],
) -> Result<Vec<(String, String)>> {
    inputs
        .iter()
        .map(|(entry, files)| {
            let mut all: Vec<String> = SHARED_INPUTS.iter().map(|f| (*f).to_owned()).collect();
            all.extend(
                files
                    .iter()
                    .filter(|f| !all.contains(f))
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            all.sort();
            Ok((entry.clone(), oracle::hash_text_files(oracle_src, &all)?))
        })
        .collect()
}

fn generator_hash(entry_hashes: &[(String, String)], vector: &Vector<'_>) -> Result<String> {
    let entry = vector.spec.entry();
    let entry_hash = entry_hashes
        .iter()
        .find(|(name, _)| name == entry)
        .map(|(_, hash)| hash)
        .ok_or_else(|| {
            format!(
                "{}: entry {entry:?} is not a harness entry point (oracle/src/{entry}.ts)",
                vector.id
            )
        })?;
    let mut parts = vec![entry_hash.clone()];
    let mut spec_text = String::new();
    for input in vector.spec.inputs() {
        let text = oracle::read(input)?;
        parts.push(oracle::hash_text(&text));
        spec_text.push_str(&String::from_utf8_lossy(&text));
    }
    if entry == "fn" {
        parts.extend(lazy_helper_hashes(entry_hashes, &spec_text));
    }
    parts.push(vector.zone.clone());
    Ok(oracle::hash_bytes(parts.join(":").as_bytes()))
}

/// `oracle:<name>` helpers that `fn` loads at runtime from `lazyHelpers/<name>` are not in the
/// `fn` bundle, so a `fn` spec that calls one also depends on that bundle's inputs.
fn lazy_helper_hashes(entry_hashes: &[(String, String)], spec_text: &str) -> Vec<String> {
    entry_hashes
        .iter()
        .filter_map(|(name, hash)| {
            let helper = name.strip_prefix(LAZY_HELPER_PREFIX)?;
            spec_text
                .contains(&format!("\"oracle:{helper}\""))
                .then(|| hash.clone())
        })
        .collect()
}

/// Copies the vectors a render host just generated for its own platform, with their manifest
/// entries, into the local `parity/vectors`. `wanted` are the vector or spec ids passed to
/// `gen` there; `fetch(id, dest)` copies one vector file from the host.
pub(crate) fn import_platform_vectors(
    paths: &Paths,
    remote: &Json,
    wanted: &[String],
    os: &str,
    mut fetch: impl FnMut(&str, &Path) -> Result,
) -> Result {
    let vectors_dir = paths.root.join("parity/vectors");
    let mut manifest = read_manifest(&vectors_dir)?;
    let install = oracle::installed(paths)?;
    let current = Current {
        commit: install.upstream_commit.clone(),
        electron: oracle::ELECTRON_VERSION.to_owned(),
        environment: oracle::environment_policy(),
    };
    let mismatched = header_mismatches(remote, &current);
    if !mismatched.is_empty() {
        return Err(format!(
            "the host's manifest was generated under a different {}",
            mismatched.join(", ")
        ));
    }
    let Some(Json::Object(entries)) = remote.get("vectors") else {
        return Err("the host's MANIFEST.json has no vectors".to_owned());
    };
    let prefix = format!("{os}-");
    let mut imported = 0;
    for (key, entry) in entries {
        let id = key.to_string_lossy();
        let requested = wanted
            .iter()
            .any(|w| id == *w || id.starts_with(&format!("{w}/")));
        let on_host = string_field(entry, "generatedOn").is_some_and(|g| g.starts_with(&prefix));
        if !requested || !on_host {
            continue;
        }
        let dest = vectors_dir.join(format!("{id}.jsonl.zst"));
        fs::create_dir_all(dest.parent().expect("vector path has a parent"))
            .map_err(|e| e.to_string())?;
        fetch(&id, &dest)?;
        let text = vectors::read_text(&dest)?;
        if Some(oracle::hash_bytes(text.as_bytes())) != string_field(entry, "content") {
            return Err(format!(
                "{id}: the copied file does not match the host's manifest"
            ));
        }
        set_entry(&mut manifest, &id, entry.clone());
        eprintln!(
            "  imported {id} ({})",
            string_field(entry, "generatedOn").unwrap_or_default()
        );
        imported += 1;
    }
    if imported == 0 {
        return Err(format!(
            "no vector generated on {os} matches {}",
            wanted.join(" ")
        ));
    }
    write_manifest(&vectors_dir, &install, manifest)
}

fn header_mismatches(manifest: &Json, current: &Current) -> Vec<String> {
    let has_vectors = matches!(manifest.get("vectors"), Some(Json::Object(v)) if !v.is_empty());
    if !has_vectors {
        return Vec::new();
    }
    let mut out = Vec::new();
    let recorded = |key: &str| string_field(manifest, key);
    if recorded("upstreamCommit").as_deref() != Some(current.commit.as_str()) {
        out.push(format!(
            "upstream commit (manifest {:?}, oracle {:?})",
            recorded("upstreamCommit").unwrap_or_default(),
            current.commit
        ));
    }
    if recorded("electron").as_deref() != Some(current.electron.as_str()) {
        out.push(format!(
            "Electron version (manifest {:?}, oracle {:?})",
            recorded("electron").unwrap_or_default(),
            current.electron
        ));
    }
    if manifest.get("environment") != Some(&current.environment) {
        out.push(format!(
            "environment policy (manifest {}, oracle {})",
            manifest
                .get("environment")
                .map_or_else(|| "none".to_owned(), Json::to_compact),
            current.environment.to_compact()
        ));
    }
    out
}

/// Every reason the committed manifest does not describe what the current oracle sources,
/// specs and policy would generate. Content is not compared here; that needs the oracle.
fn stale_entries(
    manifest: &Json,
    current: &Current,
    expected: &[Expected],
    full: bool,
) -> Vec<String> {
    let mut out = header_mismatches(manifest, current);
    for want in expected {
        let id = &want.id;
        let Some(entry) = manifest.get("vectors").and_then(|v| v.get(id)) else {
            out.push(format!("{id}: no entry in MANIFEST.json"));
            continue;
        };
        let field = |key: &str| string_field(entry, key).unwrap_or_default();
        if field("generator") != want.generator {
            out.push(format!(
                "{id}: generator hash {} is not the current {} (the spec or the oracle sources changed since it was generated)",
                field("generator"),
                want.generator
            ));
        }
        if field("spec") != want.spec {
            out.push(format!(
                "{id}: recorded spec {:?} is not {:?}",
                field("spec"),
                want.spec
            ));
        }
        if field("zone") != want.zone {
            out.push(format!(
                "{id}: recorded zone {:?} is not {:?}",
                field("zone"),
                want.zone
            ));
        }
        if let Some(platform) = &want.platform
            && field("generatedOn") != *platform
        {
            out.push(format!(
                "{id}: generated on {:?}, but it is the vector for {platform:?}",
                field("generatedOn")
            ));
        }
        match &want.committed_content {
            None => out.push(format!("{id}: no committed vector file")),
            Some(hash) if *hash != field("content") => out.push(format!(
                "{id}: committed file hashes to {hash}, MANIFEST.json says {}",
                field("content")
            )),
            Some(_) => {}
        }
    }
    if full && let Some(Json::Object(entries)) = manifest.get("vectors") {
        for (key, _) in entries {
            let key = key.to_string_lossy();
            if !expected.iter().any(|e| e.id == key) {
                out.push(format!("{key}: in MANIFEST.json but no spec produces it"));
            }
        }
    }
    out
}

fn remove_orphans(store: &mut Store, ids: &[&str]) -> Result {
    let Some(Json::Object(entries)) = store.manifest.get("vectors").cloned() else {
        return Ok(());
    };
    for (key, _) in entries {
        let id = key.to_string_lossy();
        if !ids.contains(&id.as_str()) {
            eprintln!("  removing {id}: no spec produces it here");
            remove_vector(store, &id)?;
        }
    }
    Ok(())
}

/// Deletes vector `id`'s file and manifest entry from `store`, if it has them.
fn remove_vector(store: &mut Store, id: &str) -> Result {
    if let Some(path) = vectors::vector_path(&store.dir, id) {
        fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        store.dirty = true;
    }
    if let Some(Json::Object(mut entries)) = store.manifest.get("vectors").cloned()
        && entries.iter().any(|(k, _)| k.eq_str(id))
    {
        entries.retain(|(k, _)| !k.eq_str(id));
        set_field(&mut store.manifest, "vectors", Json::Object(entries));
        store.dirty = true;
    }
    Ok(())
}

fn discover(dir: &Path) -> Result<Vec<Spec>> {
    let mut found = Vec::new();
    if dir.is_dir() {
        visit(dir, dir, &mut found)?;
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

fn visit(base: &Path, dir: &Path, found: &mut Vec<Spec>) -> Result {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            visit(base, &path, found)?;
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .expect("under base")
            .to_string_lossy()
            .replace('\\', "/");
        if rel.ends_with(REQUESTS) {
            continue;
        }
        if let Some(id) = rel.strip_suffix(SESSION_SPEC) {
            found.push(Spec {
                id: id.to_owned(),
                zones: None,
                platforms: None,
                local: false,
                kind: Kind::Entry {
                    entry: SESSION_ENTRY.to_owned(),
                    args: vec![path.to_string_lossy().into_owned()],
                    inputs: Vec::new(),
                },
                path,
            });
        } else if let Some(id) = rel.strip_suffix(ENTRY_SPEC) {
            let json = read_json(&path)?;
            let entry = json
                .get("entry")
                .and_then(Json::as_str)
                .ok_or_else(|| format!("{}: needs a string \"entry\"", path.display()))?
                .to_string_lossy();
            let args = json
                .get("args")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .map(|a| a.as_str().map(|s| s.to_string_lossy()))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("{}: \"args\" must be strings", path.display()))?;
            let dir = path.parent().expect("spec has a directory");
            let inputs = string_list(&path, &json, "inputs", |_| Ok(()))?
                .unwrap_or_default()
                .into_iter()
                .map(|input| dir.join(input))
                .collect();
            let (zones, platforms) = variants(&path, &json)?;
            let local = match json.get("local") {
                None => false,
                Some(Json::Bool(local)) => *local,
                Some(_) => return Err(format!("{}: \"local\" must be a boolean", path.display())),
            };
            if local && platforms.is_some() {
                return Err(format!(
                    "{}: \"local\" and \"platforms\" cannot be combined",
                    path.display()
                ));
            }
            found.push(Spec {
                id: id.to_owned(),
                zones,
                platforms,
                local,
                kind: Kind::Entry {
                    entry,
                    args,
                    inputs,
                },
                path,
            });
        } else if let Some(id) = rel.strip_suffix(FN_ZONED_SPEC) {
            let json = read_json(&path)?;
            let requests = json
                .get("requests")
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .filter(|r| r.ends_with(REQUESTS))
                .ok_or_else(|| {
                    format!(
                        "{}: needs \"requests\", a path ending in {REQUESTS} relative to the spec",
                        path.display()
                    )
                })?;
            let (zones, platforms) = variants(&path, &json)?;
            found.push(Spec {
                id: id.to_owned(),
                zones,
                platforms,
                local: false,
                kind: Kind::Fn {
                    requests: path.parent().expect("spec has a directory").join(requests),
                },
                path,
            });
        } else if let Some(id) = rel.strip_suffix(FN_SPEC) {
            found.push(Spec {
                id: id.to_owned(),
                zones: None,
                platforms: None,
                local: false,
                kind: Kind::Fn {
                    requests: path.clone(),
                },
                path,
            });
        }
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<Json> {
    let text = String::from_utf8(oracle::read(path)?).map_err(|e| e.to_string())?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

type Variants = (Option<Vec<String>>, Option<Vec<String>>);

fn variants(path: &Path, spec: &Json) -> Result<Variants> {
    let zones = string_list(path, spec, "zones", oracle::check_zone)?;
    let platforms = string_list(path, spec, "platforms", check_platform)?;
    if zones.is_some() && platforms.is_some() {
        return Err(format!(
            "{}: \"zones\" and \"platforms\" cannot be combined",
            path.display()
        ));
    }
    Ok((zones, platforms))
}

fn check_platform(platform: &str) -> Result {
    match platform.split_once('-') {
        Some(("macos" | "windows" | "linux", "aarch64" | "x86_64")) => Ok(()),
        _ => Err(format!(
            "platform {platform:?} is not <macos|windows|linux>-<aarch64|x86_64>"
        )),
    }
}

fn string_list(
    path: &Path,
    spec: &Json,
    key: &str,
    check: impl Fn(&str) -> Result,
) -> Result<Option<Vec<String>>> {
    let Some(list) = spec.get(key) else {
        return Ok(None);
    };
    let items = list
        .as_array()
        .filter(|l| !l.is_empty())
        .and_then(|l| {
            l.iter()
                .map(|z| z.as_str().map(|s| s.to_string_lossy()))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| {
            format!(
                "{}: \"{key}\" must be a non-empty list of strings",
                path.display()
            )
        })?;
    for (i, item) in items.iter().enumerate() {
        check(item).map_err(|e| format!("{}: {e}", path.display()))?;
        if items[..i].contains(item) {
            return Err(format!(
                "{}: {key} entry {item:?} is listed twice",
                path.display()
            ));
        }
    }
    Ok(Some(items))
}

fn run_vector(paths: &Paths, vector: &Vector<'_>) -> Result<String> {
    let zone = &vector.zone;
    match &vector.spec.kind {
        Kind::Entry { entry, args, .. } => {
            let text = oracle::pipe(
                oracle::oracle_command_in(paths, entry, zone)?
                    .args(args)
                    .env(SPEC_VARIABLE, &vector.spec.path)
                    .env(LOCAL_VARIABLE, if vector.spec.local { "1" } else { "0" })
                    .current_dir(&paths.root),
                Vec::new(),
            )?;
            for (i, line) in text.lines().enumerate() {
                json::parse(line).map_err(|e| format!("{entry} output line {}: {e}", i + 1))?;
            }
            Ok(text)
        }
        Kind::Fn { requests } => {
            let bytes = oracle::read(requests)?;
            let request_text = String::from_utf8(bytes.clone()).map_err(|e| e.to_string())?;
            let responses =
                oracle::pipe(&mut oracle::oracle_command_in(paths, "fn", zone)?, bytes)?;
            merge_fn_records(&request_text, &responses)
                .map_err(|e| format!("{}: {e}", requests.display()))
        }
    }
}

/// Each vector line is the request object with the oracle's response key appended.
fn merge_fn_records(requests: &str, responses: &str) -> Result<String> {
    let requests: Vec<&str> = requests
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let responses: Vec<&str> = responses.lines().collect();
    if requests.len() != responses.len() {
        return Err(format!(
            "{} requests but {} responses",
            requests.len(),
            responses.len()
        ));
    }
    let mut out = String::new();
    for (i, (request, response)) in requests.iter().zip(&responses).enumerate() {
        let line = i + 1;
        if let Ok(Response::Error(message)) = Response::parse(response) {
            return Err(format!("request {line} failed in the oracle: {message}"));
        }
        let Json::Object(mut pairs) =
            json::parse(request).map_err(|e| format!("request {line}: {e}"))?
        else {
            return Err(format!("request {line} is not an object"));
        };
        let Json::Object(response) =
            json::parse(response).map_err(|e| format!("response {line}: {e}"))?
        else {
            return Err(format!("response {line} is not an object"));
        };
        pairs.retain(|(k, _)| !["ok", "throw", "error"].iter().any(|r| k.eq_str(r)));
        pairs.extend(response);
        out.push_str(&Json::Object(pairs).to_compact());
        out.push('\n');
    }
    Ok(out)
}

fn read_manifest(dir: &Path) -> Result<Json> {
    let path = dir.join(MANIFEST);
    if !path.exists() {
        return Ok(Json::Object(Vec::new()));
    }
    read_json(&path)
}

fn string_field(object: &Json, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(|s| s.to_string_lossy())
}

fn set_entry(manifest: &mut Json, id: &str, entry: Json) {
    let mut vectors = match manifest.get("vectors") {
        Some(Json::Object(pairs)) => pairs.clone(),
        _ => Vec::new(),
    };
    vectors.retain(|(k, _)| !k.eq_str(id));
    vectors.push((id.into(), entry));
    vectors.sort_by(|a, b| a.0.cmp(&b.0));
    set_field(manifest, "vectors", Json::Object(vectors));
}

fn set_field(object: &mut Json, key: &str, value: Json) {
    if let Json::Object(pairs) = object {
        match pairs.iter_mut().find(|(k, _)| k.eq_str(key)) {
            Some(slot) => slot.1 = value,
            None => pairs.push((key.into(), value)),
        }
    }
}

fn write_manifest(dir: &Path, install: &oracle::Install, manifest: Json) -> Result {
    let vectors = match manifest.get("vectors") {
        Some(Json::Object(pairs)) => pairs.clone(),
        _ => Vec::new(),
    };
    let header = Json::object([
        ("format", Json::string("CVE-1")),
        ("upstreamCommit", Json::string(&install.upstream_commit)),
        ("upstreamVersion", Json::string(&install.upstream_version)),
        ("electron", Json::string(oracle::ELECTRON_VERSION)),
        ("environment", oracle::environment_policy()),
    ]);
    let path = dir.join(MANIFEST);
    fs::write(&path, manifest_text(&header, &vectors))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The header pretty-printed, then one sorted line per vector, so branches that add or
/// regenerate different vectors touch different lines.
fn manifest_text(header: &Json, vectors: &[(Utf16String, Json)]) -> String {
    let header = header.to_pretty();
    let mut out = header
        .strip_suffix("\n}")
        .expect("a non-empty object ends with a newline and a brace")
        .to_owned();
    out.push_str(",\n  \"vectors\": {");
    let mut sorted: Vec<&(Utf16String, Json)> = vectors.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (i, (id, entry)) in sorted.iter().enumerate() {
        out.push_str(if i == 0 { "\n    " } else { ",\n    " });
        json::write_string(id.units(), &mut out);
        out.push_str(": ");
        out.push_str(&entry.to_compact());
    }
    out.push_str(if sorted.is_empty() {
        "}\n}\n"
    } else {
        "\n  }\n}\n"
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> Current {
        Current {
            commit: "c0".to_owned(),
            electron: "39.2.7".to_owned(),
            environment: oracle::environment_policy(),
        }
    }

    fn manifest(environment: Json, generator: &str) -> Json {
        Json::object([
            ("upstreamCommit", Json::string("c0")),
            ("electron", Json::string("39.2.7")),
            ("environment", environment),
            (
                "vectors",
                Json::object([(
                    "a/b",
                    Json::object([
                        ("content", Json::string("h1")),
                        ("generator", Json::string(generator)),
                        ("spec", Json::string("parity/specs/a/b.jsonl")),
                        ("zone", Json::string("UTC")),
                    ]),
                )]),
            ),
        ])
    }

    fn expected(generator: &str) -> Expected {
        Expected {
            id: "a/b".to_owned(),
            generator: generator.to_owned(),
            spec: "parity/specs/a/b.jsonl".to_owned(),
            zone: "UTC".to_owned(),
            platform: None,
            committed_content: Some("h1".to_owned()),
        }
    }

    #[test]
    fn current_manifest_is_not_stale() {
        let m = manifest(oracle::environment_policy(), "g1");
        assert!(stale_entries(&m, &current(), &[expected("g1")], true).is_empty());
    }

    #[test]
    fn generator_change_is_stale() {
        let m = manifest(oracle::environment_policy(), "g1");
        let stale = stale_entries(&m, &current(), &[expected("g2")], true);
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert!(stale[0].contains("generator hash"));
    }

    #[test]
    fn header_changes_are_stale() {
        let m = manifest(Json::object([("TZ", Json::string("UTC"))]), "g1");
        let mut now = current();
        now.commit = "c1".to_owned();
        now.electron = "40.0.0".to_owned();
        let stale = stale_entries(&m, &now, &[expected("g1")], true);
        assert_eq!(stale.len(), 3, "{stale:?}");
    }

    #[test]
    fn content_zone_and_orphans_are_stale() {
        let m = manifest(oracle::environment_policy(), "g1");
        let mut want = expected("g1");
        want.committed_content = Some("h2".to_owned());
        want.zone = "Asia/Kolkata".to_owned();
        assert_eq!(stale_entries(&m, &current(), &[want], true).len(), 2);
        let mut other = expected("g1");
        other.id = "c".to_owned();
        let stale = stale_entries(&m, &current(), &[other], true);
        assert!(stale.iter().any(|s| s.contains("no entry")));
        assert!(stale.iter().any(|s| s.contains("no spec produces it")));
        let mut other = expected("g1");
        other.id = "c".to_owned();
        assert_eq!(stale_entries(&m, &current(), &[other], false).len(), 1);
    }

    #[test]
    fn platform_vector_from_another_host_is_stale() {
        let mut m = manifest(oracle::environment_policy(), "g1");
        let mut want = expected("g1");
        want.platform = Some("windows-aarch64".to_owned());
        let stale = stale_entries(&m, &current(), &[want], true);
        assert!(
            stale.iter().any(|s| s.contains("windows-aarch64")),
            "{stale:?}"
        );
        if let Some(Json::Object(vectors)) = m.get("vectors").cloned() {
            let mut entry = vectors[0].1.clone();
            set_field(&mut entry, "generatedOn", Json::string("windows-aarch64"));
            set_entry(&mut m, "a/b", entry);
        }
        let mut want = expected("g1");
        want.platform = Some("windows-aarch64".to_owned());
        assert!(stale_entries(&m, &current(), &[want], true).is_empty());
    }

    #[test]
    fn platform_specs_make_one_vector_per_platform() {
        let spec = Json::object([(
            "platforms",
            Json::Array(vec![
                Json::string("macos-aarch64"),
                Json::string("windows-aarch64"),
            ]),
        )]);
        let (zones, platforms) = variants(Path::new("s.fn.json"), &spec).unwrap();
        let spec = Spec {
            id: "js/x".to_owned(),
            path: PathBuf::from("s.fn.json"),
            kind: Kind::Fn {
                requests: PathBuf::from("s.requests.jsonl"),
            },
            zones,
            platforms,
            local: false,
        };
        let ids: Vec<String> = spec.vectors().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["js/x/macos-aarch64", "js/x/windows-aarch64"]);
        let bad = Json::object([("platforms", Json::Array(vec![Json::string("beos-ppc")]))]);
        assert!(variants(Path::new("s.fn.json"), &bad).is_err());
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("asrs-xtask-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("src")).unwrap();
            Scratch(dir)
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn harness(name: &str) -> Scratch {
        let dir = Scratch::new(name);
        for (rel, text) in [
            ("build.mjs", "build"),
            ("package-lock.json", "{}"),
            ("src/genA.ts", "a"),
            ("src/genB.ts", "b"),
            ("src/helpers.ts", "h"),
            ("src/fn.ts", "f"),
        ] {
            dir.write(rel, text);
        }
        dir
    }

    fn inputs() -> Vec<(String, Vec<String>)> {
        let list = |files: &[&str]| files.iter().map(|f| (*f).to_owned()).collect();
        vec![
            ("fn".to_owned(), list(&["src/fn.ts"])),
            ("genA".to_owned(), list(&["src/genA.ts", "src/helpers.ts"])),
            ("genB".to_owned(), list(&["src/genB.ts"])),
        ]
    }

    fn changed(before: &[(String, String)], after: &[(String, String)]) -> Vec<String> {
        before
            .iter()
            .zip(after)
            .filter(|(b, a)| b.1 != a.1)
            .map(|(b, _)| b.0.clone())
            .collect()
    }

    #[test]
    fn entry_hashes_follow_only_their_own_inputs() {
        let dir = harness("entry-hashes");
        let before = entry_hashes(&dir.0, &inputs()).unwrap();
        dir.write("src/genB.ts", "b2");
        let after = entry_hashes(&dir.0, &inputs()).unwrap();
        assert_eq!(changed(&before, &after), ["genB"]);
        dir.write("src/helpers.ts", "h2");
        let next = entry_hashes(&dir.0, &inputs()).unwrap();
        assert_eq!(changed(&after, &next), ["genA"]);
        dir.write("src/unused.ts", "u");
        assert!(changed(&next, &entry_hashes(&dir.0, &inputs()).unwrap()).is_empty());
        for shared in SHARED_INPUTS {
            let before = entry_hashes(&dir.0, &inputs()).unwrap();
            dir.write(shared, "changed");
            let after = entry_hashes(&dir.0, &inputs()).unwrap();
            assert_eq!(changed(&before, &after), ["fn", "genA", "genB"], "{shared}");
        }
    }

    #[test]
    fn entry_hashes_ignore_line_endings() {
        let dir = harness("entry-crlf");
        dir.write("src/genA.ts", "a\nb\n");
        let before = entry_hashes(&dir.0, &inputs()).unwrap();
        dir.write("src/genA.ts", "a\r\nb\r\n");
        assert_eq!(before, entry_hashes(&dir.0, &inputs()).unwrap());
    }

    #[test]
    fn entry_specs_name_the_corpora_they_read() {
        let dir = harness("corpora");
        dir.write(
            "x/args.entry.json",
            r#"{"entry": "genA", "args": ["target/parity/corpus/synthetic/b", "7"]}"#,
        );
        dir.write(
            "x/inputs.entry.json",
            r#"{"entry": "genA", "inputs": ["../../corpus/synthetic/a.manifest.json", "../../corpus/public.toml"]}"#,
        );
        dir.write(
            "x/unknown.entry.json",
            r#"{"entry": "genA", "args": ["target/parity/corpus/synthetic/zz"]}"#,
        );
        dir.write("x/none.entry.json", r#"{"entry": "genA", "args": ["7"]}"#);
        let known = ["a".to_owned(), "b".to_owned()];
        let corpora: Vec<(String, Vec<String>)> = discover(&dir.0)
            .unwrap()
            .iter()
            .map(|s| (s.id.clone(), s.corpora(&known)))
            .collect();
        let want = |id: &str, names: &[&str]| {
            (
                id.to_owned(),
                names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            corpora,
            [
                want("x/args", &["b"]),
                want("x/inputs", &["a"]),
                want("x/none", &[]),
                want("x/unknown", &[]),
            ]
        );
    }

    #[test]
    fn committed_specs_that_read_corpus_b_declare_it() {
        let root = crate::workspace_root();
        let known = corpus::names(&root).unwrap();
        let specs = discover(&specs_dir(&root)).unwrap();
        for id in ["log/merge-offset", "urcl/urcl"] {
            let spec = specs.iter().find(|s| s.id == id).expect(id);
            assert_eq!(spec.corpora(&known), ["b"], "{id}");
        }
    }

    #[test]
    fn session_files_are_live_session_specs() {
        let dir = harness("sessions");
        dir.write("live/nt4/a.session.json", r#"{"protocol": "nt4"}"#);
        let specs = discover(&dir.0).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "live/nt4/a");
        assert_eq!(specs[0].entry(), SESSION_ENTRY);
        match &specs[0].kind {
            Kind::Entry { args, .. } => assert_eq!(args, &[specs[0].path.display().to_string()]),
            Kind::Fn { .. } => panic!("a session is an entry spec"),
        }
        assert_eq!(specs[0].inputs(), vec![specs[0].path.as_path()]);
    }

    #[test]
    fn generator_hash_covers_entry_spec_and_zone() {
        let dir = harness("generator-hash");
        dir.write("a.entry.json", r#"{"entry": "genA"}"#);
        dir.write("b.entry.json", r#"{"entry": "genB"}"#);
        dir.write("c.jsonl", "{}");
        let specs = discover(&dir.0).unwrap();
        let vectors: Vec<Vector<'_>> = specs.iter().flat_map(Spec::vectors).collect();
        let hashes = |vectors: &[Vector<'_>]| -> Vec<String> {
            let entries = entry_hashes(&dir.0, &inputs()).unwrap();
            vectors
                .iter()
                .map(|v| generator_hash(&entries, v).unwrap())
                .collect()
        };
        let before = hashes(&vectors);
        dir.write("src/genA.ts", "a2");
        let after = hashes(&vectors);
        assert_eq!(
            [
                before[0] != after[0],
                before[1] != after[1],
                before[2] != after[2]
            ],
            [true, false, false]
        );
        dir.write("c.jsonl", "{}\n{}");
        let next = hashes(&vectors);
        assert_eq!(
            [
                after[0] != next[0],
                after[1] != next[1],
                after[2] != next[2]
            ],
            [false, false, true]
        );
        dir.write("src/fn.ts", "f2");
        let last = hashes(&vectors);
        assert_eq!(
            [next[0] != last[0], next[1] != last[1], next[2] != last[2]],
            [false, false, true]
        );

        let mut zoned = Vector {
            id: vectors[1].id.clone(),
            zone: "UTC".to_owned(),
            platform: None,
            spec: vectors[1].spec,
        };
        let entries = entry_hashes(&dir.0, &inputs()).unwrap();
        let utc = generator_hash(&entries, &zoned).unwrap();
        zoned.zone = "Asia/Kolkata".to_owned();
        assert_ne!(utc, generator_hash(&entries, &zoned).unwrap());

        dir.write("d.entry.json", r#"{"entry": "genMissing"}"#);
        let specs = discover(&dir.0).unwrap();
        let missing = specs
            .iter()
            .flat_map(Spec::vectors)
            .find(|v| v.id == "d")
            .unwrap();
        assert!(
            generator_hash(&entries, &missing)
                .unwrap_err()
                .contains("genMissing")
        );
    }

    #[test]
    fn fn_specs_follow_only_the_lazy_helpers_they_call() {
        let dir = harness("lazy-helpers");
        fs::create_dir_all(dir.0.join("src/lazyHelpers")).unwrap();
        dir.write("src/lazyHelpers/aRun.ts", "a");
        dir.write("src/lazyHelpers/bRun.ts", "b");
        dir.write("a.jsonl", r#"{"fn":"oracle:aRun","args":[]}"#);
        dir.write("plain.jsonl", r#"{"fn":"oracle:identity","args":[1]}"#);
        let mut inputs = inputs();
        for helper in ["aRun", "bRun"] {
            inputs.push((
                format!("{LAZY_HELPER_PREFIX}{helper}"),
                vec![format!("src/lazyHelpers/{helper}.ts")],
            ));
        }
        let specs = discover(&dir.0).unwrap();
        let vectors: Vec<Vector<'_>> = specs.iter().flat_map(Spec::vectors).collect();
        let hashes = || -> Vec<String> {
            let entries = entry_hashes(&dir.0, &inputs).unwrap();
            vectors
                .iter()
                .map(|v| generator_hash(&entries, v).unwrap())
                .collect()
        };
        let before = hashes();
        dir.write("src/lazyHelpers/bRun.ts", "b2");
        assert_eq!(before, hashes());
        dir.write("src/lazyHelpers/aRun.ts", "a2");
        let after = hashes();
        let changed: Vec<&str> = vectors
            .iter()
            .zip(before.iter().zip(&after))
            .filter(|(_, (b, a))| b != a)
            .map(|(v, _)| v.id.as_str())
            .collect();
        assert_eq!(changed, ["a"]);
    }

    #[test]
    fn manifest_has_one_sorted_line_per_vector() {
        let header = Json::object([
            ("format", Json::string("CVE-1")),
            ("electron", Json::string("39.2.7")),
        ]);
        let entry = |hash: &str| {
            Json::object([
                ("content", Json::string(hash)),
                ("zone", Json::string("UTC")),
            ])
        };
        let two = vec![("b/y".into(), entry("2")), ("a/x".into(), entry("1"))];
        let text = manifest_text(&header, &two);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[lines.len() - 4],
            r#"    "a/x": {"content":"1","zone":"UTC"},"#
        );
        assert_eq!(
            lines[lines.len() - 3],
            r#"    "b/y": {"content":"2","zone":"UTC"}"#
        );
        let parsed = json::parse(&text).unwrap();
        assert_eq!(parsed.get("electron"), Some(&Json::string("39.2.7")));
        assert_eq!(
            parsed.get("vectors").and_then(|v| v.get("b/y")),
            Some(&entry("2"))
        );

        let mut three = two.clone();
        three.push(("a/y".into(), entry("3")));
        let grown = manifest_text(&header, &three);
        let added: Vec<&str> = grown
            .lines()
            .filter(|l| !text.lines().any(|t| t == *l))
            .collect();
        assert_eq!(added, [r#"    "a/y": {"content":"3","zone":"UTC"},"#]);

        let empty = manifest_text(&header, &[]);
        assert!(json::parse(&empty).unwrap().get("vectors").is_some());
    }
}
