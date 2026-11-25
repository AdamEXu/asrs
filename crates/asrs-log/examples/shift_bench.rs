use std::sync::Arc;
use std::time::Instant;

use asrs_js::string::JsString;
use asrs_js::time::SetRng;
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;

fn keep_min(best: &mut f64, value: f64) {
    if value < *best {
        *best = value;
    }
}

fn main() {
    const N: usize = 1_000_000;
    let key = JsString::from("/A");
    let mut source = Log::new(false);
    for i in 0..N {
        source.put_number(&key, i as f64 * 0.02, i as f64).unwrap();
    }
    println!("column: {N} samples, {:.1} MB", (N * 8) as f64 / 1e6);

    let column = source.get_field(&key).unwrap().to_serialized().timestamps;
    let mut best_old = f64::INFINITY;
    let mut best_reuse = f64::INFINITY;
    let mut best_old_nonzero = f64::INFINITY;
    let mut best_nonzero = f64::INFINITY;
    for _ in 0..100 {
        let start = Instant::now();
        let shifted: Arc<Vec<f64>> = Arc::new(column.iter().map(|t| t + 0.0).collect());
        keep_min(&mut best_old, start.elapsed().as_secs_f64() * 1e3);
        std::hint::black_box(&shifted);

        let mut field = source.get_field(&key).unwrap().to_serialized();
        let start = Instant::now();
        field.shift_timestamps(0.0);
        keep_min(&mut best_reuse, start.elapsed().as_secs_f64() * 1e3);
        std::hint::black_box(&field);

        let start = Instant::now();
        let shifted: Arc<Vec<f64>> = Arc::new(column.iter().map(|t| t + 1.0).collect());
        keep_min(&mut best_old_nonzero, start.elapsed().as_secs_f64() * 1e3);
        std::hint::black_box(&shifted);

        let mut field = source.get_field(&key).unwrap().to_serialized();
        let start = Instant::now();
        field.shift_timestamps(1.0);
        keep_min(&mut best_nonzero, start.elapsed().as_secs_f64() * 1e3);
        std::hint::black_box(&field);
    }

    let mut best_merge = f64::INFINITY;
    for _ in 0..10 {
        let mut target = Log::new(false);
        let mut realm = JsRealm::new(Box::new(SetRng::new(vec![0.5])));
        let start = Instant::now();
        target
            .merge_with(&source, &JsString::new(), &mut realm)
            .unwrap();
        keep_min(&mut best_merge, start.elapsed().as_secs_f64() * 1e3);
    }

    println!("old allocate+copy, zero offset:    {best_old:.3} ms");
    println!("new scan+reuse, zero offset:       {best_reuse:.3} ms");
    println!("old allocate+copy, nonzero offset: {best_old_nonzero:.3} ms");
    println!("new scan+copy, nonzero offset:     {best_nonzero:.3} ms");
    println!("merge_with into empty hub (new):   {best_merge:.3} ms");
}
