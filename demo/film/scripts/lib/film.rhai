// Shared moves for the showcase film's beats (#435). Everything runs on
// game time (total_time()), never wall time, so a --frame-step capture
// follows the same path and the same style changes on every run.
//
//   import "film" as film;
//   film::orbit(0.0, 0.7, 0.0, 5.0, 1.6, film::lerp(0.4, 1.1, film::ease(t / 8.0)));

fn clamp01(x) {
    if x < 0.0 { 0.0 } else if x > 1.0 { 1.0 } else { x }
}

// Smoothstep: starts and lands at rest, as a camera operator would
fn ease(x) {
    let t = clamp01(x);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a, b, t) {
    a + (b - a) * t
}

// 0 before `start`, easing to 1 over `len` seconds
fn ramp(t, start, len) {
    ease((t - start) / len)
}

// 0 -> 1 -> 0: up over `rise`, held, down over `fall` ending at `end`
fn pulse(t, start, rise, end, fall) {
    ramp(t, start, rise) * (1.0 - ramp(t, end - fall, fall))
}

// The camera on a circle round (cx, cz) at `angle` radians, looking at
// (cx, cy, cz)
fn orbit(cx, cy, cz, radius, height, angle) {
    set_camera_position(cx + radius * angle.sin(), height, cz + radius * angle.cos());
    set_camera_target(cx, cy, cz);
}

// The camera from one framing to another by k (0..1): positions and
// targets interpolate in a straight line
fn dolly(from, to, from_target, to_target, k) {
    set_camera_position(
        lerp(from[0], to[0], k), lerp(from[1], to[1], k), lerp(from[2], to[2], k));
    set_camera_target(
        lerp(from_target[0], to_target[0], k),
        lerp(from_target[1], to_target[1], k),
        lerp(from_target[2], to_target[2], k));
}
