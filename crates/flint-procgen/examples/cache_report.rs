//! What the procgen cache saves, measured on real specs.
//!
//! For each spec: the cold generation time, the time a cache hit takes
//! (an `Arc` handed out), the time a deep copy of the output takes (what a
//! hit cost before outputs were shared), and the bytes the cache charges.
//!
//! ```bash
//! cargo run --release -p flint-procgen --example cache_report -- showcase_game/specs/*.procgen.toml
//! ```

use std::path::Path;
use std::time::Instant;

use flint_procgen::{
    register_built_in_generators, GeneratorRegistry, ProcGenCache, ProcGenCacheConfig, ProcGenSpec,
};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: cache_report <spec.procgen.toml>...");
        std::process::exit(2);
    }

    let mut registry = GeneratorRegistry::new();
    register_built_in_generators(&mut registry);
    let mut cache = ProcGenCache::new(ProcGenCacheConfig::default());

    println!("spec,generator,cold_ms,hit_us,deep_copy_us,bytes");
    for path in &paths {
        let spec = match ProcGenSpec::from_file(Path::new(path)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip {path}: {e}");
                continue;
            }
        };
        let seed = match spec.seed.to_seed() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip {path}: {e}");
                continue;
            }
        };
        let hash = flint_core::ContentHash::from_str(&toml::to_string(&spec).unwrap_or_default());

        let t = Instant::now();
        let output = match registry.generate_from_spec(&spec) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("skip {path}: {e}");
                continue;
            }
        };
        let cold_ms = t.elapsed().as_secs_f64() * 1e3;
        let bytes = output.estimated_bytes();
        cache.insert(hash, &seed, output);

        // Median of many runs: these are microsecond-scale.
        let mut hits = Vec::new();
        let mut copies = Vec::new();
        for _ in 0..51 {
            let t = Instant::now();
            let shared = cache.get(hash, &seed).expect("just inserted");
            hits.push(t.elapsed().as_secs_f64() * 1e6);

            let t = Instant::now();
            let copy = (*shared).clone();
            copies.push(t.elapsed().as_secs_f64() * 1e6);
            std::hint::black_box(copy);
        }
        hits.sort_by(f64::total_cmp);
        copies.sort_by(f64::total_cmp);

        println!(
            "{},{},{:.2},{:.2},{:.2},{}",
            spec.meta.name, spec.generator, cold_ms, hits[25], copies[25], bytes
        );
    }
    let stats = cache.stats();
    eprintln!(
        "cache: {} entries, {} bytes of {}, {} hits, {} misses",
        stats.entry_count, stats.total_bytes, stats.max_memory_bytes, stats.hits, stats.misses
    );
}
