// Stalls the frame (#1078): every `EVERY` seconds of game time it holds
// on_update for `STALL` seconds of wall time, a busy wait, so the F2
// graph has a hitch to show and the scripts' colour to show it in.
//
// The wait is a helper of its own, `hold`, so a trace or `--perf` names
// it as where on_update's time went (#1100):
//   flint-player examples/hitch_metronome.scene.toml --trace stall.json
//   flint-player examples/hitch_metronome.scene.toml --perf
// In the trace, `stall.rhai:hold()` sits inside `stall.rhai:on_update`;
// `--perf` says `stall.rhai:on_update ... (hold() 100.0)`.
//
// The wall clock is Rhai's timestamp(), which a deterministic capture
// (flint render --scripts) refuses: this script is for the player.

const EVERY = 3.0;  // seconds of game time between stalls
const STALL = 0.1;  // seconds each stall holds the frame

// Busy-wait `seconds` of wall time.
fn hold(seconds) {
    let start = timestamp();
    while start.elapsed < seconds {}
}

fn on_update() {
    let now = total_time();
    let before = now - delta_time();
    // Once, on the frame whose step crosses a multiple of EVERY
    if now > 1.0 && (now / global::EVERY).floor() > (before / global::EVERY).floor() {
        hold(global::STALL);
    }
}
