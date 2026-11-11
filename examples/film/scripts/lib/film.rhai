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

// --- Framing for a tall frame (#437) ----------------------------------
//
// The beats' moves were framed for 16:9. A 9:16 window keeps less than a
// third of that width at the same field of view, so the barrel and the
// figure either side of the table fall out of it. In a portrait window
// every move here is reframed: the camera backs away from its target
// along its own line of sight and the field of view opens, so the still
// life keeps most of its width and gains the sky above it and the grass
// below. The same scene and the same script serve both cuts; nothing
// changes in a landscape window.

// True in a window taller than wide (the reel's 1080x1920)
fn portrait() {
    screen_height() > screen_width()
}

// How far a portrait camera backs off, as a multiple of its distance to
// the target; the vertical field of view it opens to (degrees); and how far
// below the target it aims (metres), which lifts the still life from the
// lower middle of the tall frame to the middle of Instagram's safe area
fn portrait_push() { 1.45 }
fn portrait_fov() { 60.0 }
fn portrait_drop() { 0.3 }

// The camera at (px, py, pz) looking at (tx, ty, tz), reframed when the
// window is portrait. Every move below lands here.
fn place(px, py, pz, tx, ty, tz) {
    if portrait() {
        let k = portrait_push();
        set_camera_fov(portrait_fov());
        // Back along the line of sight, not as high as straight back would
        // put it, so the extra distance doesn't tip the view onto the grass
        set_camera_position(tx + (px - tx) * k, ty + (py - ty) * (k - 0.2), tz + (pz - tz) * k);
        set_camera_target(tx, ty - portrait_drop(), tz);
    } else {
        set_camera_position(px, py, pz);
        set_camera_target(tx, ty, tz);
    }
}

// A framing given for the tall window itself, used as is: a beat whose
// 16:9 move doesn't survive the reframe draws its own portrait path
fn place_portrait(px, py, pz, tx, ty, tz, fov) {
    set_camera_fov(fov);
    set_camera_position(px, py, pz);
    set_camera_target(tx, ty, tz);
}

// The camera on a circle round (cx, cz) at `angle` radians, looking at
// (cx, cy, cz)
fn orbit(cx, cy, cz, radius, height, angle) {
    place(cx + radius * angle.sin(), height, cz + radius * angle.cos(), cx, cy, cz);
}

// The camera from one framing to another by k (0..1): positions and
// targets interpolate in a straight line
fn dolly(from, to, from_target, to_target, k) {
    place(
        lerp(from[0], to[0], k), lerp(from[1], to[1], k), lerp(from[2], to[2], k),
        lerp(from_target[0], to_target[0], k),
        lerp(from_target[1], to_target[1], k),
        lerp(from_target[2], to_target[2], k));
}

// orbit() for a path drawn for the tall window, at field of view `fov`
fn orbit_portrait(cx, cy, cz, radius, height, angle, fov) {
    place_portrait(cx + radius * angle.sin(), height, cz + radius * angle.cos(), cx, cy, cz, fov);
}

// dolly() for a path drawn for the tall window, at field of view `fov`
fn dolly_portrait(from, to, from_target, to_target, k, fov) {
    place_portrait(
        lerp(from[0], to[0], k), lerp(from[1], to[1], k), lerp(from[2], to[2], k),
        lerp(from_target[0], to_target[0], k),
        lerp(from_target[1], to_target[1], k),
        lerp(from_target[2], to_target[2], k),
        fov);
}
