// Two shots of the clearing, cut every second, for the ASCII cut bench
// (#343). Each shot drifts slowly so the style's history matters inside a
// shot; at a cut it belongs to the other shot and must start over.
// Game time, so a --frame-step capture follows the same path every run.
//
// mode: "script" resets at each cut itself, "auto" leaves it to the
// player's cut detection, "none" turns detection off and never resets.
let mode = "script";

// zoom: 1.0 cuts between the two places. Above 1, the camera never jumps:
// it keeps drifting over the clearing and every other second punches in,
// the view `zoom` times smaller from the FOV alone (#474), the cut a
// position-only detector missed.
let zoom = 1.0;

let shot_length = 1.0; // seconds
let wide_fov = 50.0;

fn on_init() {
    // A script that resets at its own cuts has no need for detection
    set_camera_cut_detection(mode == "auto");
}

fn on_update() {
    let t = total_time();
    let shot = (t / shot_length).floor().to_int();
    let local = t - shot * shot_length;
    if zoom > 1.0 {
        // One drift through both shots: only the FOV cuts
        let a = 0.55 + t * 0.05;
        set_camera_position(0.6 + 7.5 * a.sin(), 2.1, 4.2 + 7.5 * a.cos());
        set_camera_target(0.6, 0.6, 4.2);
        let half = (wide_fov / 2.0).to_radians().tan();
        let fov = if shot % 2 == 0 {
            wide_fov
        } else {
            2.0 * (half / zoom).atan().to_degrees()
        };
        set_camera_fov(fov);
    } else if shot % 2 == 0 {
        // Wide: over the clearing from the south-east, drifting left
        let a = 0.55 + local * 0.05;
        set_camera_position(0.6 + 7.5 * a.sin(), 2.1, 4.2 + 7.5 * a.cos());
        set_camera_target(0.6, 0.6, 4.2);
    } else {
        // Close and low from the north-west, by the bench, rising
        set_camera_position(-2.6, 0.7 + local * 0.08, 1.4);
        set_camera_target(0.8, 0.5, 4.6);
    }
    // The first frame of each new shot
    let last_shot = ((t - delta_time()) / shot_length).floor().to_int();
    if mode == "script" && shot != last_shot && t > 0.0 {
        reset_style_history();
    }
}
