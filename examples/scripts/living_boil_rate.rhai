// ─── living_boil_rate.rhai ───────────────────────────────
// Directs examples/living_boil_rate.scene.toml (#529): the flame's boil ticks
// twice as fast, then at half the spec's pace, then eases back to it. The pose
// clock is the wooze field's step and doesn't move: only the cards' flicker.

fn crossed(at) {
    let now = total_time();
    now >= at && now - delta_time() < at
}

fn on_update() {
    if crossed(2.0) {
        tune_living(#{ living: #{ jitter_rate: 2.0 } });
    }
    if crossed(4.0) {
        tune_living(#{ living: #{ jitter_rate: 0.5 } });
    }
    if crossed(6.0) {
        tune_living(#{ living: #{ jitter_rate: 1.0 } }, 1.0);
    }
}
