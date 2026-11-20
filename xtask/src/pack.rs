use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Component, Path, PathBuf};

use asrs_parity::json::{self, Json};

use crate::Result;
use crate::oracle::{self, Paths};

const PACK_FORMAT: f64 = 1.0;
const PACK_INFO: &str = "asrs-oracle-pack.json";
const PACK_ROOT: &str = "upstream";
const ZSTD_LEVEL: i32 = 12;

pub(crate) fn pack(paths: &Paths, args: &[String]) -> Result {
    let out = match args {
        [] => paths.out.join("pack.tar.zst"),
        [flag, file] if flag == "--out" => PathBuf::from(file),
        _ => return Err("usage: cargo xtask oracle pack [--out <file>]".to_owned()),
    };
    let install = oracle::installed(paths)?;
    for built in [
        "bundles/hub.js",
        "bundles/wpilogIndexer.wasm",
        "src/licenses.json",
    ] {
        if !paths.upstream.join(built).is_file() {
            return Err(format!(
                "{} is missing; run `cargo xtask oracle setup` before packing",
                paths.upstream.join(built).display()
            ));
        }
    }

    oracle::step("pack");
    let info = Json::object([
        ("format", Json::Number(PACK_FORMAT)),
        ("upstreamCommit", Json::string(&install.upstream_commit)),
        ("upstreamVersion", Json::string(&install.upstream_version)),
        ("electron", Json::string(oracle::ELECTRON_VERSION)),
        ("emsdk", Json::string(oracle::EMSDK_VERSION)),
        ("packedOn", Json::string(&oracle::host_tag())),
    ])
    .to_pretty()
        + "\n";

    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let partial = out.with_extension("partial");
    let file = File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    let encoder = zstd::stream::Encoder::new(BufWriter::new(file), ZSTD_LEVEL)
        .map_err(|e| format!("zstd: {e}"))?;
    let mut builder = tar::Builder::new(encoder);
    builder.mode(tar::HeaderMode::Deterministic);
    builder.follow_symlinks(false);

    let mut header = tar::Header::new_gnu();
    header.set_size(info.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, PACK_INFO, info.as_bytes())
        .map_err(|e| format!("writing {PACK_INFO}: {e}"))?;

    let mut count = 0usize;
    append_tree(
        &mut builder,
        &paths.upstream,
        Path::new(PACK_ROOT),
        &mut count,
    )?;
    let encoder = builder
        .into_inner()
        .map_err(|e| format!("finishing the tar stream: {e}"))?;
    let writer = encoder.finish().map_err(|e| format!("zstd: {e}"))?;
    writer
        .into_inner()
        .map_err(|e| format!("{}: {e}", partial.display()))?
        .sync_all()
        .map_err(|e| format!("{}: {e}", partial.display()))?;
    fs::rename(&partial, &out).map_err(|e| format!("{}: {e}", out.display()))?;

    let size = fs::metadata(&out).map(|m| m.len()).unwrap_or_default();
    eprintln!(
        "  {count} entries, {:.1} MB -> {}",
        size as f64 / 1e6,
        out.display()
    );
    eprintln!("install on another host with `cargo xtask oracle setup --from-pack <file>`");
    Ok(())
}

fn append_tree<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    dir: &Path,
    name: &Path,
    count: &mut usize,
) -> Result {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .collect::<std::io::Result<_>>()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let file_name = entry.file_name();
        if name == Path::new(PACK_ROOT) && file_name == "node_modules" {
            continue;
        }
        let path = entry.path();
        let child = name.join(&file_name);
        let file_type = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if file_type.is_dir() {
            builder
                .append_dir(&child, &path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            append_tree(builder, &path, &child, count)?;
        } else {
            builder
                .append_path_with_name(&path, &child)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        *count += 1;
    }
    Ok(())
}

pub(crate) fn setup_from_pack(paths: &Paths, file: &Path, force: bool) -> Result {
    let open = || -> Result<tar::Archive<zstd::stream::Decoder<'static, BufReader<File>>>> {
        let reader = File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let decoder = zstd::stream::Decoder::new(reader).map_err(|e| format!("zstd: {e}"))?;
        Ok(tar::Archive::new(decoder))
    };
    check_pack_info(&mut open()?, file)?;

    fs::create_dir_all(&paths.out).map_err(|e| format!("{}: {e}", paths.out.display()))?;
    let _ = fs::remove_file(&paths.install);
    oracle::step("unpack upstream");
    clear_upstream(&paths.upstream)?;
    let mut archive = open()?;
    let mut unpacked = 0usize;
    let mut skipped_links = 0usize;
    for entry in archive
        .entries()
        .map_err(|e| format!("{}: {e}", file.display()))?
    {
        let mut entry = entry.map_err(|e| format!("{}: {e}", file.display()))?;
        let name = entry
            .path()
            .map_err(|e| format!("{}: {e}", file.display()))?
            .into_owned();
        if name == Path::new(PACK_INFO) {
            continue;
        }
        if !is_pack_path(&name) {
            return Err(format!(
                "{}: unexpected entry {} outside {PACK_ROOT}/",
                file.display(),
                name.display()
            ));
        }
        if cfg!(windows) && entry.header().entry_type().is_symlink() {
            skipped_links += 1;
            continue;
        }
        entry
            .unpack_in(&paths.out)
            .map_err(|e| format!("unpacking {}: {e}", name.display()))?;
        unpacked += 1;
    }
    eprintln!("  {unpacked} entries unpacked");
    if skipped_links > 0 {
        eprintln!("  {skipped_links} symlinks skipped (Lite build trees, not used by the oracle)");
    }

    fs::create_dir_all(&paths.cache).map_err(|e| format!("{}: {e}", paths.cache.display()))?;
    oracle::install_upstream_packages(paths, force)?;
    oracle::install_electron(paths, force)?;
    oracle::finish_setup(paths, force, "pack")
}

fn check_pack_info<R: Read>(archive: &mut tar::Archive<R>, file: &Path) -> Result {
    let bad = |why: String| format!("{} is not a usable oracle pack: {why}", file.display());
    let mut entries = archive.entries().map_err(|e| bad(e.to_string()))?;
    let mut first = entries
        .next()
        .ok_or_else(|| bad("it is empty".to_owned()))?
        .map_err(|e| bad(e.to_string()))?;
    if first.path().map_err(|e| bad(e.to_string()))?.as_ref() != Path::new(PACK_INFO) {
        return Err(bad(format!("the first entry is not {PACK_INFO}")));
    }
    let mut text = String::new();
    first
        .read_to_string(&mut text)
        .map_err(|e| bad(e.to_string()))?;
    let info = json::parse(&text).map_err(|e| bad(e.to_string()))?;
    let field = |key: &str| {
        info.get(key)
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
    };
    if info.get("format").and_then(Json::as_f64) != Some(PACK_FORMAT) {
        return Err(bad(format!("unsupported format, expected {PACK_FORMAT}")));
    }
    if field("upstreamCommit") != oracle::UPSTREAM_COMMIT
        || field("electron") != oracle::ELECTRON_VERSION
    {
        return Err(bad(format!(
            "it holds upstream {} with Electron {}, but xtask pins {} with Electron {}",
            field("upstreamCommit"),
            field("electron"),
            oracle::UPSTREAM_COMMIT,
            oracle::ELECTRON_VERSION
        )));
    }
    eprintln!(
        "oracle pack: upstream {} ({}), Electron {}, packed on {}",
        field("upstreamVersion"),
        &field("upstreamCommit")[..7],
        field("electron"),
        field("packedOn")
    );
    Ok(())
}

fn is_pack_path(path: &Path) -> bool {
    let mut components = path.components();
    components.next() == Some(Component::Normal(PACK_ROOT.as_ref()))
        && components.all(|c| matches!(c, Component::Normal(_)))
}

/// Removes everything in the upstream copy except `node_modules`, which `npm ci` reuses when
/// the lockfile is unchanged.
fn clear_upstream(upstream: &Path) -> Result {
    if !upstream.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(upstream).map_err(|e| format!("{}: {e}", upstream.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == "node_modules" {
            continue;
        }
        let path = entry.path();
        let result = if entry.file_type().is_ok_and(|t| t.is_dir()) {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        result.map_err(|e| format!("removing {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_paths_stay_under_the_upstream_root() {
        assert!(is_pack_path(Path::new("upstream/src/main.ts")));
        assert!(is_pack_path(Path::new("upstream")));
        assert!(!is_pack_path(Path::new("upstream/../x")));
        assert!(!is_pack_path(Path::new("/upstream/x")));
        assert!(!is_pack_path(Path::new("other/x")));
    }
}
