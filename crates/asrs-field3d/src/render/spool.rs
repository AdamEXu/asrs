use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use super::scene::Geometry;
use crate::three::{ArrayData, ArrayKind, BufferAttribute, BufferGeometry};

/// Geometry smaller than this stays in memory.
const MIN_BYTES: usize = 256 << 10;
/// How long the spool thread waits for more geometry before writing a file.
const BATCH: Duration = Duration::from_millis(200);
const PREFIX: &str = "asrs-models-";

/// A temporary file only this process can open, holding the arrays of uploaded models so they
/// take no memory while the GPU has its own copy. It is deleted as soon as it is created
/// (Unix) or when its last handle closes, a crash included (Windows), so it goes with the last
/// geometry that reads it.
#[derive(Debug)]
pub(crate) struct SpoolFile(File);

#[cfg(target_os = "linux")]
fn directories() -> Vec<PathBuf> {
    vec![PathBuf::from("/var/tmp"), std::env::temp_dir()]
}

#[cfg(not(target_os = "linux"))]
fn directories() -> Vec<PathBuf> {
    vec![std::env::temp_dir()]
}

fn create() -> io::Result<File> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut error = io::Error::from(io::ErrorKind::NotFound);
    for directory in directories() {
        remove_stale(&directory);
        let name = format!(
            "{PREFIX}{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        match open(&directory.join(name)) {
            Ok(file) => return Ok(file),
            Err(e) => error = e,
        }
    }
    Err(error)
}

/// Spool files of other processes. A live one cannot be removed: on Unix it no longer has a
/// name, and on Windows no other process may open it.
fn remove_stale(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let own = format!("{PREFIX}{}-", std::process::id());
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(PREFIX) && !name.starts_with(&own) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(unix)]
fn open(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    std::fs::remove_file(path)?;
    Ok(file)
}

#[cfg(windows)]
fn open(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(0)
        .attributes(FILE_ATTRIBUTE_TEMPORARY)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(path)
}

impl SpoolFile {
    #[cfg(unix)]
    fn read_exact_at(&self, bytes: &mut [u8], offset: u64) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(&self.0, bytes, offset)
    }

    #[cfg(windows)]
    fn read_exact_at(&self, mut bytes: &mut [u8], mut offset: u64) -> io::Result<()> {
        while !bytes.is_empty() {
            let n = std::os::windows::fs::FileExt::seek_read(&self.0, bytes, offset)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            bytes = &mut bytes[n..];
            offset += n as u64;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Part {
    kind: ArrayKind,
    len: usize,
    offset: u64,
}

#[derive(Debug)]
struct SpooledAttribute {
    name: String,
    part: Part,
    item_size: usize,
    normalized: bool,
    interleaved: bool,
}

/// Where a geometry's arrays went in a spool file; everything else stays here.
#[derive(Debug)]
pub(crate) struct Spooled {
    file: Arc<SpoolFile>,
    index: Option<(Part, bool)>,
    attributes: Vec<SpooledAttribute>,
    template: BufferGeometry,
}

fn bytes(array: &ArrayData) -> &[u8] {
    match array {
        ArrayData::Float32(a) => bytemuck::cast_slice(a),
        ArrayData::Float64(a) => bytemuck::cast_slice(a),
        ArrayData::Uint8(a) => a,
        ArrayData::Int8(a) => bytemuck::cast_slice(a),
        ArrayData::Uint16(a) => bytemuck::cast_slice(a),
        ArrayData::Int16(a) => bytemuck::cast_slice(a),
        ArrayData::Uint32(a) => bytemuck::cast_slice(a),
        ArrayData::Int32(a) => bytemuck::cast_slice(a),
    }
}

fn read_array(file: &SpoolFile, part: Part) -> io::Result<ArrayData> {
    let mut array = ArrayData::zeroed(part.kind, part.len);
    let target: &mut [u8] = match &mut array {
        ArrayData::Float32(a) => bytemuck::cast_slice_mut(a),
        ArrayData::Float64(a) => bytemuck::cast_slice_mut(a),
        ArrayData::Uint8(a) => a,
        ArrayData::Int8(a) => bytemuck::cast_slice_mut(a),
        ArrayData::Uint16(a) => bytemuck::cast_slice_mut(a),
        ArrayData::Int16(a) => bytemuck::cast_slice_mut(a),
        ArrayData::Uint32(a) => bytemuck::cast_slice_mut(a),
        ArrayData::Int32(a) => bytemuck::cast_slice_mut(a),
    };
    file.read_exact_at(target, part.offset)?;
    Ok(array)
}

pub(crate) fn size(geometry: &BufferGeometry) -> usize {
    let index = geometry.index.iter().map(|i| bytes(&i.array).len());
    let attributes = geometry.attributes().map(|(_, a)| bytes(&a.array).len());
    index.chain(attributes).sum()
}

impl Spooled {
    /// The geometry as it was spooled.
    pub(crate) fn read(&self) -> io::Result<BufferGeometry> {
        let mut geometry = self.template.clone();
        if let Some((part, normalized)) = self.index {
            let array = read_array(&self.file, part)?;
            geometry.index = Some(BufferAttribute::new(array, 1, normalized));
        }
        for a in &self.attributes {
            let mut attribute =
                BufferAttribute::new(read_array(&self.file, a.part)?, a.item_size, a.normalized);
            attribute.interleaved = a.interleaved;
            geometry.set_attribute(&a.name, attribute);
        }
        Ok(geometry)
    }
}

struct Writer {
    file: Arc<SpoolFile>,
    out: io::BufWriter<File>,
    offset: u64,
}

impl Writer {
    fn new() -> io::Result<Writer> {
        let file = create()?;
        let out = io::BufWriter::with_capacity(1 << 20, file.try_clone()?);
        Ok(Writer {
            file: Arc::new(SpoolFile(file)),
            out,
            offset: 0,
        })
    }

    fn part(&mut self, array: &ArrayData) -> io::Result<Part> {
        let data = bytes(array);
        self.out.write_all(data)?;
        let part = Part {
            kind: array.kind(),
            len: array.len(),
            offset: self.offset,
        };
        self.offset += data.len() as u64;
        Ok(part)
    }

    fn spool(&mut self, geometry: &BufferGeometry) -> io::Result<Spooled> {
        let index = match &geometry.index {
            Some(index) if index.item_size == 1 && !index.interleaved => {
                Some((self.part(&index.array)?, index.normalized))
            }
            Some(_) => return Err(io::ErrorKind::Unsupported.into()),
            None => None,
        };
        let mut attributes = Vec::new();
        for (name, a) in geometry.attributes() {
            attributes.push(SpooledAttribute {
                name: name.to_owned(),
                part: self.part(&a.array)?,
                item_size: a.item_size,
                normalized: a.normalized,
                interleaved: a.interleaved,
            });
        }
        let mut template = geometry.clone_attributes(&[]);
        template.index = None;
        Ok(Spooled {
            file: self.file.clone(),
            index,
            attributes,
            template,
        })
    }
}

fn queue() -> &'static Mutex<Sender<Weak<Geometry>>> {
    static QUEUE: OnceLock<Mutex<Sender<Weak<Geometry>>>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("field3d-spool".to_owned())
            .spawn(move || run(&rx))
            .expect("spawning the spool thread");
        Mutex::new(tx)
    })
}

/// Moves `geometry`'s arrays to a spool file soon, if they are worth it. Reading it back gives
/// the same geometry; if the file cannot be written, the arrays stay in memory.
pub(crate) fn spool_later(geometry: &Arc<Geometry>) {
    if geometry.claim_spool(MIN_BYTES)
        && let Ok(queue) = queue().lock()
    {
        let _ = queue.send(Arc::downgrade(geometry));
    }
}

fn run(rx: &Receiver<Weak<Geometry>>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(more) = rx.recv_timeout(BATCH) {
            batch.push(more);
        }
        let live: Vec<Arc<Geometry>> = batch.iter().filter_map(Weak::upgrade).collect();
        if live.is_empty() {
            continue;
        }
        if let Err(e) = spool(live) {
            eprintln!("asrs: 3D models stay in memory: {e}");
        }
    }
}

fn spool(live: Vec<Arc<Geometry>>) -> io::Result<()> {
    let mut writer = Writer::new()?;
    let mut spooled = Vec::new();
    for geometry in live {
        let Some(data) = geometry.in_memory() else {
            continue;
        };
        match writer.spool(&data) {
            Ok(s) => spooled.push((geometry, data, s)),
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {}
            Err(e) => return Err(e),
        }
    }
    writer.out.flush()?;
    drop(writer);
    for (geometry, data, s) in spooled {
        geometry.replace_with_spooled(&data, s);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::three::primitives::sphere_geometry;

    fn sphere() -> BufferGeometry {
        sphere_geometry(1.5, 128.0, 128.0, 0.0, 6.0, 0.0, 3.0)
    }

    fn own_files() -> usize {
        let own = format!("{PREFIX}{}-", std::process::id());
        directories()
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&own))
            .count()
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(20), "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_spooled_geometry_reads_back_the_same_and_its_file_goes_with_it() {
        let stale = directories()[0].join(format!("{PREFIX}0-0"));
        std::fs::write(&stale, b"left by a crashed run").expect("a stale spool file");
        let geometry = Geometry::new(sphere());
        let small = Geometry::new(sphere_geometry(1.0, 8.0, 4.0, 0.0, 6.0, 0.0, 3.0));
        geometry.spool_later();
        small.spool_later();
        wait_for("spooled", || !geometry.is_in_memory());
        assert!(small.is_in_memory());
        assert!(!stale.exists());
        assert_eq!(*geometry.data(), sphere());
        assert_eq!(own_files(), usize::from(cfg!(windows)));
        drop(geometry);
        wait_for("the spool file is gone", || own_files() == 0);
    }
}
