// The pilot that recorded lap.input (#1629): a closed-loop driver that
// follows the centre line and writes what it does as input-script lines.
//
// lap.input is a recording, not a hand-tuned script: a key script tuned by
// hand left the hairpin 3 degrees off and drove onto the runoff 40 m on,
// since nothing open-loop corrects. When a deliberate change to the
// vehicle's handling makes the lap fail, record it again:
//
//   flint playtest packages/vehicle/examples/test_track/test_track.scene.toml \
//     --test-script packages/vehicle/examples/test_track/playtests/pilot.rhai \
//     --exit-after 12.6 --report lap.json
//
// and replace lap.input's lines with the report's `log.lines` (the times
// are already the script's), then run the referee on it.
//
// It steers by pure pursuit, at a point 4 m + a quarter of the speed in
// seconds ahead on the centre line, and holds a speed: 24 m/s on the
// straights, 12.5 in the turns, braking 14 m/s² ahead of them. It changes
// its stick and triggers only every 0.1 s, at 0.08, 0.18, ... (20 ms before
// a 1/30 s frame, so a line lands on the same fixed step at 1/30 and 1/144
// s frames), to values a pad sends: the stick's 0.05 deadzone put back in,
// the triggers to tenths. So the recording replays what it drove.

let path = [];       // the centre line, a point a metre: [x, z]
let speed = [];      // the speed wanted at each point
let near = 0;        // the point nearest the kart, searched forward
let next_t = 0.08;   // when the next change may be made
let steer = 0.0;
let throttle = 0.0;
let brake = 0.0;
let said = #{ steer: 99.0, throttle: 99.0, brake: 99.0 };
let lines = [];

const PI = 3.14159265;

fn on_update() {
    if path.len() == 0 {
        // The start straight to the hairpin, the hairpin, the back
        // straight, the closing turn, and the start straight again
        let z = 4.0;
        while z > -60.0 { path.push([0.0, z]); speed.push(24.0); z -= 1.0; }
        for i in 0..22 {
            let a = global::PI * i.to_float() / 22.0;
            path.push([-7.0 + 7.0 * cos(a), -60.0 - 7.0 * sin(a)]);
            speed.push(12.5);
        }
        z = -60.0;
        while z < 10.0 { path.push([-14.0, z]); speed.push(24.0); z += 1.0; }
        for i in 0..22 {
            let a = global::PI + global::PI * i.to_float() / 22.0;
            path.push([-7.0 + 7.0 * cos(a), 10.0 - 7.0 * sin(a)]);
            speed.push(12.5);
        }
        z = 10.0;
        while z > -30.0 { path.push([0.0, z]); speed.push(24.0); z -= 1.0; }
        // Slow down ahead of each turn: v² = v_next² + 2 a d, a metre apart
        let i = speed.len() - 2;
        while i >= 0 {
            let most = sqrt(speed[i + 1] * speed[i + 1] + 2.0 * 14.0);
            if most < speed[i] { speed[i] = most; }
            i -= 1;
        }
    }

    let kart = get_entity("kart");
    if total_time() + 0.0001 >= next_t {
        let p = get_position(kart);
        let state = get_component(kart, "vehicle_state");
        let best = 1e9;
        let j = near;
        while j < near + 15 && j < path.len() {
            let dx = path[j][0] - p.x;
            let dz = path[j][1] - p.z;
            if dx * dx + dz * dz < best { best = dx * dx + dz * dz; near = j; }
            j += 1;
        }
        // Steer at the point ahead: its angle off the nose, positive left
        let ahead = min(near + (4.0 + 0.25 * state.speed).to_int(), path.len() - 1);
        let tx = path[ahead][0] - p.x;
        let tz = path[ahead][1] - p.z;
        let yaw = state.yaw * global::PI / 180.0;
        let fx = -sin(yaw);
        let fz = -cos(yaw);
        let off = atan2(tx * fz - tz * fx, tx * fx + tz * fz);
        steer = (clamp(-off * 2.2, -1.0, 1.0) * 20.0).round() / 20.0;
        // Hold the speed wanted 3 m on
        let e = speed[min(near + 3, speed.len() - 1)] - state.speed;
        throttle = 0.0;
        brake = 0.0;
        if e > 0.0 {
            throttle = clamp(0.5 + e * 0.4, 0.0, 1.0);
        } else if e < -1.0 {
            brake = clamp(-e * 0.25, 0.0, 1.0);
        } else {
            throttle = 0.45;
        }
        throttle = (throttle * 10.0).round() / 10.0;
        brake = (brake * 10.0).round() / 10.0;

        let at = (next_t * 100.0).round() / 100.0;
        if steer != said.steer {
            // The stick that reads as `steer` past the binding's deadzone
            let stick = 0.0;
            if steer != 0.0 {
                stick = (0.05 + steer.abs() * 0.95) * steer / steer.abs();
            }
            lines.push(`${at} axis LeftStickX ${(stick * 1000.0).round() / 1000.0}`);
            said.steer = steer;
        }
        if throttle != said.throttle {
            lines.push(`${at} pull RightTrigger2 ${throttle}`);
            said.throttle = throttle;
        }
        if brake != said.brake {
            lines.push(`${at} pull LeftTrigger2 ${brake}`);
            said.brake = brake;
        }
        test_log("lines", lines);
        next_t += 0.1;
    }
    // Over the package's own writes from the (empty) pad, every frame
    set_field(kart, "vehicle_input", "throttle", throttle);
    set_field(kart, "vehicle_input", "brake", brake);
    set_field(kart, "vehicle_input", "steer", steer);
}
