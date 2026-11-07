// A slow orbit around the clearing for the ASCII motion bench (#277).
// Slow on purpose: glyph shimmer is worst when the picture moves less than
// a cell a frame, so every cell's tone creeps across its ramp boundaries.
// Game time, so a --frame-step capture follows the same path every run.

let centre_x = 0.6;
let centre_z = 4.2;
let radius = 5.0;
let height = 1.5;
let speed = 0.12; // radians a second
let start = 0.55;

fn on_update() {
    let a = start + total_time() * speed;
    set_camera_position(centre_x + radius * a.sin(), height, centre_z + radius * a.cos());
    set_camera_target(centre_x, 0.6, centre_z);
}
