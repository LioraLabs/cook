//! One-off measurement harness (COOK-313 follow-up question): how much of a
//! settled large-graph run is still attributable to the recipe index?
//!
//! Ignored by default and pointed at a real on-disk index via COOK_BENCH_INDEX,
//! because it needs a 1,700-node project's cache to say anything:
//!
//!     COOK_BENCH_INDEX=/home/alex/dev/duckdb-cook/.cook/cache/duckdb_lib.idx \
//!       cargo test -p cook-cache --test bench_real_index -- --ignored --nocapture

use std::time::Instant;

#[test]
#[ignore = "needs COOK_BENCH_INDEX pointing at a real large index"]
fn measure_decode_and_walk_costs() {
    let path = match std::env::var("COOK_BENCH_INDEX") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("set COOK_BENCH_INDEX");
            return;
        }
    };
    let bytes = std::fs::read(&path).expect("read index");
    println!("index: {} ({} bytes)", path, bytes.len());

    // 1. Decode.
    let mut decode_ms = Vec::new();
    let mut cache = None;
    for _ in 0..5 {
        let t = Instant::now();
        let c = cook_cache::index_bin::decode(&bytes).expect("decode");
        decode_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        cache = Some(c);
    }
    let cache = cache.unwrap();
    let records: usize = cache
        .steps
        .values()
        .map(|s| s.inputs.len() + s.outputs.len())
        .sum();
    println!(
        "steps={} records={} decode={:.1}ms (min {:.1})",
        cache.steps.len(),
        records,
        decode_ms.iter().sum::<f64>() / decode_ms.len() as f64,
        decode_ms.iter().cloned().fold(f64::MAX, f64::min),
    );

    // 2. The two things `check_inputs` does per unit per run, which is what a
    //    PathId(u32) would change: build the &str view, compare the slices,
    //    and clone the record vec (an atomic refcount bump per record under
    //    Arc<str>, a plain memcpy under u32).
    let mut view_ms = 0.0;
    let mut cmp_ms = 0.0;
    let mut clone_ms = 0.0;
    for _ in 0..5 {
        let t = Instant::now();
        let views: Vec<Vec<&str>> = cache
            .steps
            .values()
            .map(|s| s.inputs.iter().map(|f| &*f.path).collect())
            .collect();
        view_ms += t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let mut eq = 0usize;
        for (s, v) in cache.steps.values().zip(views.iter()) {
            let again: Vec<&str> = s.inputs.iter().map(|f| &*f.path).collect();
            if &again == v {
                eq += 1;
            }
        }
        cmp_ms += t.elapsed().as_secs_f64() * 1000.0;
        assert!(eq > 0);

        let t = Instant::now();
        let cloned: Vec<Vec<cook_cache::FileRecord>> =
            cache.steps.values().map(|s| s.inputs.to_vec()).collect();
        clone_ms += t.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(&cloned);
    }
    println!(
        "per settled run, whole graph:\n  \
         build &str views   {:.1} ms\n  \
         slice compare      {:.1} ms\n  \
         record vec clone   {:.1} ms  (Arc refcount atomics; u32 would be memcpy)\n  \
         SUM of the three   {:.1} ms",
        view_ms / 5.0,
        cmp_ms / 5.0,
        clone_ms / 5.0,
        (view_ms + cmp_ms + clone_ms) / 5.0
    );
}
