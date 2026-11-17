//! Spike #1095: what a scope costs through each candidate mechanism.
//!
//! cargo run --release -p flint-core --example scope_bench
//! cargo run --release -p flint-core --example scope_bench --features spike-tracy
//! (and without --release for the dev-build numbers)
#![allow(clippy::disallowed_methods)] // a bench: std::time on purpose

use std::hint::black_box;
use std::time::Instant;

static N_CELL: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
#[allow(non_snake_case)]
fn N() -> u32 {
    *N_CELL.get_or_init(|| {
        std::env::var("SPIKE_N")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2_000_000)
    })
}

fn per_scope(label: &str, mut f: impl FnMut(u32)) -> f64 {
    // Warm (allocates the ring, fills caches), then the best of five runs.
    for i in 0..N() / 10 {
        f(i);
    }
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        for i in 0..N() {
            f(i);
        }
        best = best.min(t.elapsed().as_nanos() as f64 / N() as f64);
    }
    println!("{label:<58} {best:>8.2} ns");
    best
}

fn main() {
    let profile = if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    };
    println!(
        "profile: {profile}, tracy: {}",
        cfg!(feature = "spike-tracy")
    );
    println!("recorder compiled in: {}", flint_core::profile::COMPILED_IN);

    #[cfg(feature = "spike-tracy")]
    {
        tracy_client::Client::start();
        if std::env::var_os("SPIKE_WAIT_VIEWER").is_some() {
            println!("waiting 3 s for tracy-capture to connect...");
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }

    per_scope("loop body alone (baseline)", |i| {
        black_box(i);
    });
    per_scope("Instant::now() x2", |i| {
        let a = Instant::now();
        black_box(i);
        black_box(a.elapsed());
    });

    #[cfg(target_arch = "x86_64")]
    per_scope("rdtsc x2 (what Tracy's clock costs)", |i| {
        // SAFETY: rdtsc has no preconditions on x86_64.
        let a = unsafe { core::arch::x86_64::_rdtsc() };
        black_box(i);
        black_box(unsafe { core::arch::x86_64::_rdtsc() } - a);
    });

    // Option 2: Flint's macro.
    flint_core::profile::set_recording(false);
    per_scope("(2) flint scope!, recording off", |i| {
        flint_core::scope!("bench");
        black_box(i);
    });
    flint_core::profile::set_recording(true);
    per_scope("(2) flint scope!, recording on", |i| {
        flint_core::scope!("bench");
        black_box(i);
    });
    per_scope("(2) flint scope!, recording on, count detail", |i| {
        flint_core::scope!("bench", i);
        black_box(i);
    });
    per_scope("(2) flint scope!, recording on, text detail", |i| {
        flint_core::scope!("bench", "models/oak_tree.glb");
        black_box(i);
    });
    per_scope(
        "(2) flint scope!, 3 nested, recording on (per scope = /3)",
        |i| {
            flint_core::scope!("a");
            {
                flint_core::scope!("b");
                {
                    flint_core::scope!("c");
                    black_box(i);
                }
            }
        },
    );
    flint_core::profile::set_recording(false);

    // Option 3: Tracy alone (profiling::scope! with the tracy backend).
    #[cfg(feature = "spike-tracy")]
    per_scope("(3) profiling::scope! -> tracy", |i| {
        profiling::scope!("bench");
        black_box(i);
    });

    // Option 1: a tracing span per scope, into a recording layer.
    option1::bench();
    #[cfg(any(debug_assertions, feature = "profiler"))]
    {
        let mut written = 0;
        flint_core::profile::with_rings(|r| written += r.written());
        println!("records written by the recorder: {written}");
    }
}

mod option1 {
    use super::*;
    use std::cell::RefCell;
    use tracing::span::{Attributes, Id};
    use tracing::Subscriber;
    use tracing_subscriber::layer::{Context, Layer};
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::registry::LookupSpan;

    struct Start(Instant);

    thread_local! {
        static RING: RefCell<Vec<(&'static str, u64)>> = RefCell::new(Vec::with_capacity(1 << 16));
    }

    /// What a recorder as a tracing layer must do: stamp on enter, push on exit.
    struct Recorder;
    impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Recorder {
        fn on_new_span(&self, _: &Attributes<'_>, _: &Id, _: Context<'_, S>) {}
        fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
            if let Some(span) = ctx.span(id) {
                span.extensions_mut().insert(Start(Instant::now()));
            }
        }
        fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
            if let Some(span) = ctx.span(id) {
                let dur = span
                    .extensions()
                    .get::<Start>()
                    .map(|s| s.0.elapsed().as_nanos() as u64)
                    .unwrap_or(0);
                let name = span.name();
                RING.with(|r| {
                    let mut r = r.borrow_mut();
                    if r.len() == r.capacity() {
                        r.clear();
                    }
                    r.push((name, dur));
                });
            }
        }
    }

    pub fn bench() {
        per_scope("(1) tracing span, no subscriber", |i| {
            let _s = tracing::info_span!("bench").entered();
            black_box(i);
        });
        let sub = tracing_subscriber::registry().with(Recorder);
        tracing::subscriber::with_default(sub, || {
            per_scope("(1) tracing span -> registry + recording layer", |i| {
                let _s = tracing::info_span!("bench").entered();
                black_box(i);
            });
        });
        // The player's own subscriber is fmt + env-filter: with the recorder
        // layer beside it, every scope also passes through the fmt layer.
        let sub = tracing_subscriber::registry()
            .with(Recorder)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::sink))
            .with(tracing_subscriber::EnvFilter::new("info"));
        tracing::subscriber::with_default(sub, || {
            per_scope(
                "(1) tracing span -> registry + recorder + fmt + filter",
                |i| {
                    let _s = tracing::info_span!("bench").entered();
                    black_box(i);
                },
            );
        });
    }
}
