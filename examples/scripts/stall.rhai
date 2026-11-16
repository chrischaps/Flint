// Stalls the frame (#1078): every `EVERY` seconds of game time it holds
// on_update for `STALL` seconds of wall time, a busy wait, so the F2
// graph has a hitch to show and the scripts' colour to show it in.
//
// The wall clock is Rhai's timestamp(), which a deterministic capture
// (flint render --scripts) refuses: this script is for the player.

const EVERY = 3.0;  // seconds of game time between stalls
const STALL = 0.1;  // seconds each stall holds the frame

fn on_update() {
    let now = total_time();
    let before = now - delta_time();
    // Once, on the frame whose step crosses a multiple of EVERY
    if now > 1.0 && (now / global::EVERY).floor() > (before / global::EVERY).floor() {
        let start = timestamp();
        while start.elapsed < global::STALL {}
    }
}
