// ─── living_ease.rhai ────────────────────────────────────
// Directs examples/living_ease.scene.toml (#514): the flame's echoes widen over
// two seconds, hold, then gather in over two more. The continuous knobs glide
// on the wooze field's smoothstep; echoes.count, a discrete knob, switches at
// the start of its tune.

fn crossed(at) {
    let now = total_time();
    now >= at && now - delta_time() < at
}

fn on_update() {
    if crossed(0.5) {
        tune_living(#{ living: #{ echoes: #{ spread: 1.8, alpha: 0.35 } } }, 2.0);
    }
    if crossed(4.0) {
        tune_living(#{ living: #{ echoes: #{ count: 6, spread: 0.3 } } }, 2.0);
    }
}
