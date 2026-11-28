// The test track's referee (#1629): what the playtest holds the vehicle to.
//
//   flint playtest packages/vehicle/examples/test_track/test_track.scene.toml \
//     --input-script packages/vehicle/examples/test_track/playtests/lap.input \
//     --test-script packages/vehicle/examples/test_track/playtests/referee.rhai \
//     --timeout 30
//
// It fails the run when the kart
//   - leaves the track: a wheel stands on the runoff, or the kart falls
//     below it;
//   - flips: its up tilts more than 60 degrees from the world's;
//   - doesn't finish a lap: the hairpin, the back straight and the closing
//     turn in that order, then back over the line, within 20 s.
// At the line it logs the lap and passes. `--timeout` ends a run that never
// reaches either, since without it a playtest passes at 10 s.

let sector = 0;          // 0 start, 1 hairpin, 2 back straight, 3 closing turn
let last_t = 0.0;
let last_z = 4.0;
let top_speed = 0.0;
let airtime = 0.0;
let sand_speed = 99.0;
let worst_up = 1.0;
let widest = 0.0;
let done = false;

const LAP_LIMIT = 20.0;  // seconds
const MIN_UP = 0.5;      // cos 60 degrees

// How far the kart is off the centre line, m: the straights are x = 0 and
// x = -14, the turns 7 m circles about (-7, -60) and (-7, 10). The road is
// 3.5 m either side, and the kart's wheels 0.67 m either side of it.
fn off_centre(x, z) {
    if z < -60.0 {
        (sqrt((x + 7.0) * (x + 7.0) + (z + 60.0) * (z + 60.0)) - 7.0).abs()
    } else if z > 10.0 {
        (sqrt((x + 7.0) * (x + 7.0) + (z - 10.0) * (z - 10.0)) - 7.0).abs()
    } else if x > -7.0 {
        x.abs()
    } else {
        (x + 14.0).abs()
    }
}

// Two decimals, for the messages
fn r2(x) {
    (x * 100.0).round() / 100.0
}

fn on_update() {
    if done {
        return;
    }
    let t = total_time();
    let kart = get_entity("kart");
    let runoff = get_entity("runoff");
    let sand = get_entity("sand");
    let p = get_position(kart);
    let state = get_component(kart, "vehicle_state");

    // Left the track
    for w in vehicle_wheels(kart) {
        if w.contact && w.ground == runoff {
            done = true;
            test_fail(`left the track at ${r2(t)} s: ${entity_name(w.entity)} on the runoff at x ${r2(p.x)}, z ${r2(p.z)}`);
            return;
        }
    }
    if p.y < -1.0 {
        done = true;
        test_fail(`fell off the world at ${r2(t)} s (y ${r2(p.y)})`);
        return;
    }

    // Flipped: the body's up, from its rotation (y of q * (0, 1, 0))
    let q = get_rotation_quat(kart);
    let up = 1.0 - 2.0 * (q.x * q.x + q.z * q.z);
    if up < worst_up {
        worst_up = up;
    }
    if up < global::MIN_UP {
        done = true;
        test_fail(`flipped at ${r2(t)} s: up ${r2(up)}, at x ${r2(p.x)}, z ${r2(p.z)}`);
        return;
    }

    let off = off_centre(p.x, p.z);
    if off > widest {
        widest = off;
    }
    if state.speed > top_speed {
        top_speed = state.speed;
    }
    if state.airborne_time > airtime {
        airtime = state.airborne_time;
    }
    let on_sand = false;
    for w in vehicle_wheels(kart) {
        if w.contact && w.ground == sand {
            on_sand = true;
        }
    }
    if on_sand && state.speed < sand_speed {
        sand_speed = state.speed;
    }

    // The lap, sector by sector
    if sector == 0 && p.z < -63.0 {
        sector = 1;
        test_log("hairpin_at", t);
    } else if sector == 1 && p.x < -10.0 && p.z > -40.0 {
        sector = 2;
        test_log("back_straight_at", t);
    } else if sector == 2 && p.z > 14.0 {
        sector = 3;
        test_log("closing_turn_at", t);
    } else if sector == 3 && p.x > -3.5 && last_z > 0.0 && p.z <= 0.0 {
        // Over the line: the time it crossed z = 0, between this frame and
        // the last
        let lap = last_t + (t - last_t) * last_z / (last_z - p.z);
        done = true;
        test_log("lap_time", lap);
        test_log("top_speed", top_speed);
        test_log("longest_airtime", airtime);
        test_log("slowest_on_sand", sand_speed);
        test_log("least_up", worst_up);
        test_log("widest_off_centre", widest);
        test_pass();
        return;
    }
    if t > global::LAP_LIMIT {
        done = true;
        test_fail(`no lap in ${global::LAP_LIMIT} s: reached sector ${sector} of 3, at x ${r2(p.x)}, z ${r2(p.z)}`);
        return;
    }
    last_t = t;
    last_z = p.z;
}
