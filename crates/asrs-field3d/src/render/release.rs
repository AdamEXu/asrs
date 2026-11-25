use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::scene::Geometry;
use crate::three::{ArrayData, BufferGeometry};

/// Geometry smaller than this stays in memory.
const MIN_BYTES: usize = 256 << 10;

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

pub(super) fn size(geometry: &BufferGeometry) -> usize {
    let index = geometry.index.iter().map(|i| bytes(&i.array).len());
    let attributes = geometry.attributes().map(|(_, a)| bytes(&a.array).len());
    index.chain(attributes).sum()
}

fn mix(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3)
}

fn mix_bytes(mut hash: u64, data: &[u8]) -> u64 {
    let (words, rest) = data.as_chunks::<8>();
    for word in words {
        hash = mix(hash, u64::from_le_bytes(*word));
    }
    for &byte in rest {
        hash = mix(hash, u64::from(byte));
    }
    mix(hash, data.len() as u64)
}

/// A digest of everything an upload reads.
pub(super) fn digest(geometry: &BufferGeometry) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    if let Some(index) = &geometry.index {
        hash = mix(hash, index.array.kind() as u64);
        hash = mix_bytes(hash, bytes(&index.array));
    }
    for (name, a) in geometry.attributes() {
        hash = mix_bytes(hash, name.as_bytes());
        hash = mix(hash, a.array.kind() as u64);
        hash = mix(hash, a.item_size as u64 * 4 + u64::from(a.normalized) * 2);
        hash = mix(hash, u64::from(a.interleaved));
        hash = mix_bytes(hash, bytes(&a.array));
    }
    hash
}

fn queue() -> &'static Mutex<Sender<Weak<Geometry>>> {
    static QUEUE: OnceLock<Mutex<Sender<Weak<Geometry>>>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("field3d-release".to_owned())
            .spawn(move || run(&rx))
            .expect("spawning the geometry release thread");
        Mutex::new(tx)
    })
}

pub(super) fn release_later(geometry: &Arc<Geometry>) {
    if geometry.claim_release(MIN_BYTES)
        && let Ok(queue) = queue().lock()
    {
        let _ = queue.send(Arc::downgrade(geometry));
    }
}

fn run(rx: &Receiver<Weak<Geometry>>) {
    while let Ok(geometry) = rx.recv() {
        let Some(geometry) = geometry.upgrade() else {
            continue;
        };
        if let Some(data) = geometry.in_memory() {
            geometry.released(&data, digest(&data));
        }
    }
}
