// The camera for the ASCII motion bench (#277, #344), in game time, so a
// --frame-step capture follows the same path every run.
//
// orbit (the default): circles the clearing at `speed` radians a second.
// Slow by default: glyph shimmer is worst when the picture moves less than
// a cell a frame, so every cell's tone creeps across its ramp boundaries.
//
// pan (`[entities.camera_rig.orbit] pan = true`): stands where the orbit
// starts and turns in place at `speed`. At 1.2 rad/s the whole picture
// slides about two cells a frame, the case reprojected history is for
// (#344): a turn in place has no parallax, so every cell's past is known.

let centre_x = 0.6;
let centre_z = 4.2;
let radius = 5.0;
let height = 1.5;
let start = 0.55;

fn on_update() {
    let me = self_entity();
    let speed = get_field(me, "orbit", "speed");
    if type_of(speed) != "f64" {
        speed = 0.12; // radians a second
    }
    let a = start + total_time() * speed;
    if get_field(me, "orbit", "pan") == true {
        // Look from the orbit's start towards the clearing, turned by the
        // angle the orbit would have travelled
        let x = centre_x + radius * start.sin();
        let z = centre_z + radius * start.cos();
        let turn = a - start;
        let dx = centre_x - x;
        let dz = centre_z - z;
        set_camera_position(x, height, z);
        set_camera_target(
            x + dx * turn.cos() - dz * turn.sin(),
            0.6,
            z + dx * turn.sin() + dz * turn.cos()
        );
    } else {
        set_camera_position(centre_x + radius * a.sin(), height, centre_z + radius * a.cos());
        set_camera_target(centre_x, 0.6, centre_z);
    }
}
