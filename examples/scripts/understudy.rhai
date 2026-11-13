// Understudy (#407): the director. Twenty-eight seconds, looping, on game
// time, so `flint-player --frame-step` (or `flint render --scripts --time T`)
// draws the same film every run. Captions and panel borders are the HUD's
// (understudy_hud.rhai), which keeps the same clock.
//
//    0.0  live. The mannequin waits by the easel
//    3.4  it walks to the page (2 s) and turns to it
//    5.8  it reaches out; at 6.0 the picture tears open at the page
//    6.6  the figure is drawn on twos from here (motion_hold 12)
//         (the tear bursts open and spreads, eased out over 5 s)
//    7.0  it walks into the drawing and round the clearing (12 s)
//   11.0  the tear has taken the whole frame: the rotoscope pencil
//   18.6  a wipe pulls the live frame back from the right (3 s)
//   21.6  live; the hold is off
//   23.0  the page is left drawn: a panel reveal, small, at the page
//   28.0  the page is cleared and it starts over
//
// Each beat is one call made on the frame it starts; the engine eases the
// reveal from there. The beat is kept on the director's `story`
// component, since a script's top level runs again before every callback
// and its functions can't see top-level variables.

// --- Small helpers -----------------------------------------------------------

fn smooth(a, b, x) {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// Catmull-Rom between p1 and p2 (each [x, z]) at s 0..1
fn cr(p0, p1, p2, p3, s) {
    let s2 = s * s;
    let s3 = s2 * s;
    let out = [];
    for i in 0..2 {
        out.push(0.5 * (2.0 * p1[i] + (p2[i] - p0[i]) * s
            + (2.0 * p0[i] - 5.0 * p1[i] + 4.0 * p2[i] - p3[i]) * s2
            + (3.0 * p1[i] - p0[i] * 1.0 - 3.0 * p2[i] + p3[i]) * s3));
    }
    out
}

// A spline through `pts` (the ends held) sampled to a polyline with the
// distance walked at each point: [[x, z, d], ...]
fn polyline(pts) {
    let n = pts.len();
    let line = [];
    let d = 0.0;
    let last = pts[0];
    for k in 0..(n - 1) {
        let p0 = if k == 0 { pts[0] } else { pts[k - 1] };
        let p3 = if k + 2 >= n { pts[n - 1] } else { pts[k + 2] };
        for j in 0..16 {
            let p = cr(p0, pts[k], pts[k + 1], p3, j.to_float() / 16.0);
            d += sqrt((p[0] - last[0]) * (p[0] - last[0]) + (p[1] - last[1]) * (p[1] - last[1]));
            line.push([p[0], p[1], d]);
            last = p;
        }
    }
    let p = pts[n - 1];
    d += sqrt((p[0] - last[0]) * (p[0] - last[0]) + (p[1] - last[1]) * (p[1] - last[1]));
    line.push([p[0], p[1], d]);
    line
}

// The point at distance d along a polyline, and its heading: [x, z, yaw°]
fn along(line, d) {
    let n = line.len();
    if d <= 0.0 {
        let a = line[0];
        let b = line[1];
        return [a[0], a[1], rad_to_deg(atan2(b[0] - a[0], b[1] - a[1]))];
    }
    for i in 1..n {
        let b = line[i];
        if b[2] >= d || i == n - 1 {
            let a = line[i - 1];
            let s = if b[2] > a[2] { clamp((d - a[2]) / (b[2] - a[2]), 0.0, 1.0) } else { 1.0 };
            return [lerp(a[0], b[0], s), lerp(a[1], b[1], s), rad_to_deg(atan2(b[0] - a[0], b[1] - a[1]))];
        }
    }
}

// Distance walked t seconds into a walk of `len` metres over `dur` seconds,
// easing in and out over `ramp` seconds; and the speed as a share of top
fn walked(t, len, dur, ramp) {
    if t <= 0.0 { return [0.0, 0.0]; }
    if t >= dur { return [len, 0.0]; }
    let v = len / (dur - ramp);
    if t < ramp {
        return [v * t * t / (2.0 * ramp), t / ramp];
    }
    if t > dur - ramp {
        let u = dur - t;
        return [len - v * u * u / (2.0 * ramp), u / ramp];
    }
    [v * (t - ramp * 0.5), 1.0]
}

// Turn from one yaw to another the short way round
fn yaw_mix(a, b, s) {
    let d = b - a;
    while d > 180.0 { d -= 360.0; }
    while d < -180.0 { d += 360.0; }
    a + d * s
}

// --- The set's numbers ----------------------------------------------------------

// Where it waits, where it stands to touch the page, and the way round
fn walk_in() { [[-0.8, 5.6], [-0.3, 5.5], [0.35, 5.3], [0.8, 5.08]] }
fn walk_round() {
    [[0.8, 5.08], [0.45, 4.55], [0.25, 3.6], [0.4, 2.5], [-0.5, 1.9], [-1.4, 2.5],
     [-1.55, 3.8], [-1.3, 5.0], [-0.8, 5.6]]
}
fn rest_yaw() { 25.0 }   // facing the camera, a little to the right
fn page_yaw() { 155.0 }  // facing the page

// --- The figure -------------------------------------------------------------------

// Pose every joint. phase: the stride's angle (radians); walk: 0 standing
// .. 1 full stride; reach: 0..1 the right arm up to the page; t: the clock,
// for breathing. Forward on a limb is a turn about -X (it faces +Z).
fn pose(phase, walk, reach, t, head_turn) {
    let breathe = sin(t * 1.9);
    // Pelvis: bob twice a stride, sway a little
    let bob = 0.022 * walk * cos(phase * 2.0);
    set_position(get_entity("mq_hips"), 0.012 * walk * sin(phase), 0.95 - 0.012 * walk + bob, 0.0);
    set_rotation(get_entity("mq_hips"), 0.0, -6.0 * walk * sin(phase), 2.0 * walk * cos(phase));
    // Waist counters the hips; a lean into the walk, a breath when still
    set_rotation(get_entity("mq_waist"), 4.0 * walk + 1.2 * breathe * (1.0 - walk), 11.0 * walk * sin(phase), -2.0 * walk * cos(phase));
    set_rotation(get_entity("mq_neck"), -3.0 * walk + 6.0 * reach, head_turn - 5.0 * walk * sin(phase), 0.0);

    for side in 0..2 {
        let suf = if side == 0 { "_l" } else { "_r" };
        let sx = if side == 0 { 1.0 } else { -1.0 };
        let ph = phase + if side == 0 { 0.0 } else { PI() };
        // Legs: hip swing, the knee bent through the swing, the foot kept level
        let hip = 24.0 * walk * sin(ph);
        let knee = walk * (6.0 + 52.0 * pow_pos(cos(ph - 0.35), 1.6));
        set_rotation(get_entity("mq_leg" + suf), -hip - 2.0 * walk, 0.0, 0.0);
        set_rotation(get_entity("mq_knee" + suf), knee, 0.0, 0.0);
        set_rotation(get_entity("mq_ankle" + suf), hip - knee * 0.6, 0.0, 0.0);
        // Arms: against the leg on the same side
        let swing = -18.0 * walk * sin(ph);
        let arm_x = -swing;
        let arm_z = sx * (6.0 + 1.0 * breathe * (1.0 - walk));
        let elbow = -(14.0 + 10.0 * walk + 8.0 * walk * max(0.0, sin(ph + PI())));
        if side == 1 && reach > 0.0 {
            // The right arm lifts to the page and opens at the elbow
            arm_x = lerp(arm_x, -78.0, reach);
            arm_z = lerp(arm_z, 12.0, reach);
            elbow = lerp(elbow, -12.0, reach);
        }
        set_rotation(get_entity("mq_arm" + suf), arm_x, 0.0, arm_z);
        set_rotation(get_entity("mq_elbow" + suf), elbow, 0.0, 0.0);
    }
}

fn pow_pos(x, k) {
    if x <= 0.0 { 0.0 } else { x ** k }
}

// --- Each frame ---------------------------------------------------------------------

let period = 28.0;

fn on_update() {
    let t = total_time() % 28.0;
    let me = self_entity();
    let mq = get_entity("mq");

    // Where the figure is: waiting, walking in, at the page, walking round
    let line_in = polyline(walk_in());
    let line_round = polyline(walk_round());
    let len_in = line_in[line_in.len() - 1][2];
    let len_round = line_round[line_round.len() - 1][2];
    let stride = 1.15; // metres a full cycle (two steps)

    let x = 0.0;
    let z = 0.0;
    let yaw = rest_yaw();
    let walk = 0.0;
    let dist = 0.0;
    let reach = 0.0;
    let head = 0.0;
    if t < 3.4 {
        let p = line_in[0];
        x = p[0]; z = p[1];
        // It looks round at the page before it goes
        head = 55.0 * smooth(2.2, 3.0, t);
        yaw = rest_yaw();
    } else if t < 7.0 {
        let w = walked(t - 3.4, len_in, 2.0, 0.6);
        dist = w[0];
        walk = w[1];
        let p = along(line_in, dist);
        x = p[0]; z = p[1];
        // Into the walk from its resting yaw, then round to face the page
        yaw = yaw_mix(rest_yaw(), p[2], smooth(3.4, 3.9, t));
        yaw = yaw_mix(yaw, page_yaw(), smooth(5.0, 5.7, t));
        head = 55.0 * (1.0 - smooth(3.4, 3.9, t));
        reach = smooth(5.7, 6.0, t) * (1.0 - smooth(6.4, 6.9, t));
    } else {
        let w = walked(t - 7.0, len_round, 12.0, 0.7);
        dist = len_in + w[0];
        walk = w[1];
        let p = along(line_round, w[0]);
        x = p[0]; z = p[1];
        yaw = yaw_mix(page_yaw(), p[2], smooth(7.0, 7.5, t));
        // Home: round to face the camera again
        yaw = yaw_mix(yaw, rest_yaw(), smooth(18.6, 19.6, t));
        // and a last look back at the page
        head = 50.0 * smooth(24.5, 25.5, t) * (1.0 - smooth(27.0, 27.8, t));
    }
    set_position(mq, x, 0.0, z);
    set_rotation(mq, 0.0, yaw, 0.0);
    pose(dist / stride * TAU(), walk, reach, t, head);

    // The camera: a slow move round the clearing that keeps the figure in
    // the frame, eased between six marks
    let page = get_world_position(get_entity("page"));
    let keys = [
        [0.0, 3.0, 1.75, 10.2], [6.0, 2.1, 1.5, 8.3], [11.0, 1.6, 2.0, 9.0],
        [16.0, -0.4, 1.8, 9.3], [21.0, 1.9, 1.65, 9.2], [28.0, 3.0, 1.75, 10.2],
    ];
    let cam = [0.0, 0.0, 0.0];
    for i in 0..(keys.len() - 1) {
        let a = keys[i];
        let b = keys[i + 1];
        if t >= a[0] && t <= b[0] {
            let s = smooth(a[0], b[0], t);
            cam = [lerp(a[1], b[1], s), lerp(a[2], b[2], s), lerp(a[3], b[3], s)];
        }
    }
    let follow = smooth(6.5, 9.0, t) * (1.0 - smooth(19.0, 22.0, t));
    set_camera_position(cam[0], cam[1], cam[2]);
    set_camera_target(lerp(page.x - 0.35, x, follow * 0.7), lerp(1.0, 0.95, follow), lerp(page.z, z, follow * 0.7));

    // --- The beats: one call each, on the frame the beat starts ---
    let beat = if t < 6.0 { 0 } else if t < 6.6 { 1 } else if t < 18.6 { 2 }
        else if t < 21.6 { 3 } else if t < 23.0 { 4 } else { 5 };
    if beat == get_field(me, "story", "beat") {
        return;
    }
    set_field(me, "story", "beat", beat);
    if beat == 0 {
        // The top of the loop: no drawing anywhere
        clear_motion_hold(mq);
        set_post_style_param("pencil", "reveal", 0.0);
        set_post_style_param("pencil", "reveal_shape", "tear");
    } else if beat == 1 {
        // The touch. The tear starts where the page is on screen
        let s = world_to_screen(page.x, page.y, page.z);
        set_post_style_param("pencil", "reveal_shape", "tear");
        set_post_style_param("pencil", "reveal_x", s.x / screen_width());
        set_post_style_param("pencil", "reveal_y", s.y / screen_height());
        // ease_out: a rip bursts, then spreads
        tween_post_style_param("pencil", "reveal", 1.0, 5.0, "ease_out");
    } else if beat == 2 {
        // Inside the drawing it is drawn as the lines are: on twos
        set_motion_hold(mq, 12.0);
    } else if beat == 3 {
        // Pulled back to life from the right
        set_post_style_param("pencil", "reveal_shape", "wipe");
        set_post_style_param("pencil", "reveal_angle", 0.0);
        tween_post_style_param("pencil", "reveal", 0.0, 3.0, "ease_in_out");
    } else if beat == 4 {
        clear_motion_hold(mq);
    } else if beat == 5 {
        // Mostly: the page keeps a little of the drawing
        let s = world_to_screen(page.x, page.y, page.z);
        set_post_style_param("pencil", "reveal_shape", "panel");
        set_post_style_param("pencil", "reveal_x", s.x / screen_width());
        set_post_style_param("pencil", "reveal_y", s.y / screen_height());
        tween_post_style_param("pencil", "reveal", 0.06, 1.6, "ease_out");
    }
}
