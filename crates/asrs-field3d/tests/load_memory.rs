#![allow(clippy::disallowed_methods, unsafe_code)]

//! The heap a model load takes at its peak, beside what it returns.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use asrs_field3d::glb::{LoadOptions, load_robot, with_load_stack};
use asrs_js::string::JsString;
use asrs_model::assets::{Parsed, parse_robot};
use asrs_model::preferences::Field3dMode;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MESHES: usize = 64;
const VERTICES: usize = 3000;

/// A GLB of `MESHES` meshes, each with its own float positions and normals and a u16 index.
fn model() -> Vec<u8> {
    let part = VERTICES * 12;
    let index = VERTICES * 2;
    let mut body = Vec::new();
    let (mut views, mut accessors, mut meshes, mut nodes) = (vec![], vec![], vec![], vec![]);
    for mesh in 0..MESHES {
        for _ in 0..2 {
            views.push(format!(
                r#"{{"buffer":0,"byteOffset":{},"byteLength":{part}}}"#,
                body.len()
            ));
            accessors.push(format!(
                r#"{{"bufferView":{},"componentType":5126,"count":{VERTICES},"type":"VEC3"}}"#,
                views.len() - 1
            ));
            for i in 0..VERTICES * 3 {
                body.extend(((i + mesh) as f32).to_le_bytes());
            }
        }
        views.push(format!(
            r#"{{"buffer":0,"byteOffset":{},"byteLength":{index}}}"#,
            body.len()
        ));
        accessors.push(format!(
            r#"{{"bufferView":{},"componentType":5123,"count":{VERTICES},"type":"SCALAR"}}"#,
            views.len() - 1
        ));
        for i in 0..VERTICES {
            body.extend((i as u16).to_le_bytes());
        }
        let first = mesh * 3;
        meshes.push(format!(
            r#"{{"primitives":[{{"attributes":{{"POSITION":{first},"NORMAL":{}}},"indices":{}}}]}}"#,
            first + 1,
            first + 2
        ));
        nodes.push(format!(r#"{{"mesh":{mesh}}}"#));
    }
    let list = (0..MESHES).map(|i| i.to_string()).collect::<Vec<_>>();
    let mut json = format!(
        r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":{}}}],"bufferViews":[{}],"accessors":[{}],"meshes":[{}],"nodes":[{}],"scenes":[{{"nodes":[{}]}}]}}"#,
        body.len(),
        views.join(","),
        accessors.join(","),
        meshes.join(","),
        nodes.join(","),
        list.join(",")
    )
    .into_bytes();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + body.len();
    let mut out = Vec::with_capacity(total);
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(&json);
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(&body);
    out
}

#[test]
fn a_load_holds_each_vertex_about_twice() {
    let file = model();
    let raw = asrs_js::json::parse_str(
        r#"{"name":"Test","isFTC":false,"disableSimplification":true,"rotations":[],"position":[0,0,0],"cameras":[],"components":[]}"#,
    )
    .expect("config JSON");
    let Parsed::Config(mut config) = parse_robot(&raw).expect("parses") else {
        panic!("the config does not parse");
    };
    config.path = JsString::from("/a/model.glb");
    let options = LoadOptions::for_mode(Field3dMode::Standard);
    let (before, peak, meshes) = with_load_stack(|| {
        let before = LIVE.load(Ordering::Relaxed);
        PEAK.store(before, Ordering::Relaxed);
        let meshes = load_robot(&config, false, &options, &|_| Some(file.clone())).expect("loads");
        (before, PEAK.load(Ordering::Relaxed), meshes)
    });
    let vertices = MESHES * VERTICES;
    let position = meshes[0][0].geometry.attribute("position");
    assert_eq!(position.expect("positions").count(), vertices);
    let result = LIVE.load(Ordering::Relaxed) - before;
    let returned = vertices * (12 + 12 + 3 + 4);
    assert!((returned..returned * 11 / 10).contains(&result), "{result}");
    let limit = returned * 5 / 2;
    assert!(peak - before < limit, "{} of {limit}", peak - before);
}
