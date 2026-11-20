use asrs_tabs::heatmap::Heatmap;

#[test]
fn smoke() {
    let mut h = Heatmap::new();
    let translations: Vec<[f64; 2]> = vec![[0.0, 0.0], [0.01, 0.02], [4.0, -2.0], [-4.0, 2.0], [8.0, 4.0]];
    let out = h.update(&translations, [800.0, 400.0], [16.54, 8.21]).unwrap();
    assert!(out.created);
    let set = out.set_data.unwrap();
    eprintln!("min={} max={} points={}", set.min, set.max, set.data.len());
    for p in set.data.iter().take(8) { eprintln!("  {p:?}"); }
    let store = h.get_data().unwrap();
    eprintln!("store min={} max={} points={}", store.min, store.max, store.data.len());
    let img = h.image_data().unwrap().unwrap();
    eprintln!("img {}x{} nonzero={}", img.width, img.height, img.data.iter().filter(|&&b| b != 0).count());
    let out2 = h.update(&translations, [800.0, 400.0], [16.54, 8.21]).unwrap();
    assert!(!out2.created && out2.set_data.is_none());
    let out3 = h.update(&[[1.0, 1.0]], [800.0, 400.0], [16.54, 8.21]).unwrap();
    assert!(!out3.created && out3.set_data.is_some());
}
