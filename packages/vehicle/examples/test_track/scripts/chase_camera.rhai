// A chase camera for the test track (#1629): behind the kart and above it,
// eased so a slide or a landing reads as motion rather than a jolt. It stays
// in a script until the kernel's player and view take it (#1631).

let cam = #{ x: 0.0, y: 3.5, z: 11.0 };
let started = false;

const BACK = 7.0;     // metres behind the kart
const UP = 3.2;       // and above it
const EASE = 4.0;     // how fast the camera closes on where it wants to be, a second

fn on_update() {
    let kart = get_entity("kart");
    let p = get_position(kart);
    let fwd = get_field(kart, "vehicle_state", "forward");
    // The flat forward, so a jump's pitch doesn't swing the camera
    let fx = fwd[0];
    let fz = fwd[2];
    let len = sqrt(fx * fx + fz * fz);
    if len < 0.01 {
        fx = 0.0;
        fz = -1.0;
    } else {
        fx /= len;
        fz /= len;
    }
    let want_x = p.x - fx * BACK;
    let want_y = p.y + UP;
    let want_z = p.z - fz * BACK;
    if !started {
        cam.x = want_x;
        cam.y = want_y;
        cam.z = want_z;
        started = true;
    }
    let k = clamp(EASE * delta_time(), 0.0, 1.0);
    cam.x += (want_x - cam.x) * k;
    cam.y += (want_y - cam.y) * k;
    cam.z += (want_z - cam.z) * k;
    set_camera_position(cam.x, cam.y, cam.z);
    set_camera_target(p.x + fx * 4.0, p.y + 0.4, p.z + fz * 4.0);
}
