// ─── living_sweep.rhai ───────────────────────────────────
// Directs examples/living_sweep.scene.toml: a hand on every slider of the F3 Living
// Props panel, end to end, so a run's [living] lines can say whether any frame
// went without a pose.
//
//   0 to 30 s: echo count, 0 to 16 and back, twice
//  30 to 60 s: echo spread, 0 to 1.818 steps and back, twice, at 16 echoes
//  60 to 90 s: both, out of step, with variance and stagger moving under them
//  from 90 s:  back as the scene set them, and holding still

fn tri(x) {
    1.0 - (2.0 * (x - x.floor()) - 1.0).abs()
}

fn crossed(at) {
    let now = total_time();
    now >= at && now - delta_time() < at
}

fn on_update() {
    let t = total_time();
    if t < 30.0 {
        tune_living(#{ living: #{ echoes: #{ count: (tri(t / 15.0) * 16.0).round().to_int() } } });
    } else if t < 60.0 {
        tune_living(#{ living: #{ echoes: #{ count: 16, spread: tri(t / 15.0) * 1.818 } } });
    } else if t < 90.0 {
        let stagger = if t < 70.0 { "per_branch" } else if t < 80.0 { "none" } else { "per_level" };
        tune_living(#{ living: #{
            stagger: stagger,
            echoes: #{
                count: (tri(t / 7.0) * 16.0).round().to_int(),
                spread: tri(t / 11.0) * 1.818,
            },
            variance: #{
                amount: tri(t / 5.0),
                steady: tri(t / 9.0),
                keep_size: t < 75.0,
            },
        } });
    }
    if crossed(90.0) {
        tune_living(#{ living: #{
            stagger: "per_level",
            echoes: #{ count: 6, spread: 0.9, alpha: 0.2 },
            variance: #{ amount: 0.35, steady: 0.8, keep_size: true },
        } });
        log("sweep: done, back as the scene set them");
    }
}
