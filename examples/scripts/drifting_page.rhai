// ─── drifting_page.rhai ──────────────────────────────────
// A voice through the page's rows (examples/drifting_page.scene.toml, #576):
// phrases that quicken the drift, pauses that all but hold it. The script only
// sets a rate; the engine owns the motion, so no word jumps when the rate
// changes, it only goes on faster or slower from where it is.

// Each beat: when it starts in the cycle (seconds), its row rate, and the
// seconds the rate eases there over.
fn beats() {
    [
        [0.0, 1.0, 0.3],
        [2.0, 3.2, 0.25],
        [3.6, 0.15, 0.2],
        [4.5, 2.4, 0.35],
        [6.7, 0.0, 0.4],
        [7.9, 1.8, 0.3],
    ]
}

fn cycle() {
    10.3
}

// Whether this frame crossed `at` seconds into a cycle.
fn crossed(at) {
    let now = total_time();
    let before = now - delta_time();
    let mark = (now / cycle()).floor() * cycle() + at;
    now >= mark && before < mark
}

fn on_update() {
    for beat in beats() {
        if crossed(beat[0]) {
            tune_living(#{ living: #{ row_rate: beat[1] } }, beat[2]);
        }
    }
}
