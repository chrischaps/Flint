// ─── nav_lamplighter.rhai ─────────────────────────────────
// The lamplighter of examples/nav_lamplighter.scene.toml (#969). It asks
// find_path for the way to the next unlit lantern (round the hedges, up the
// terrace's ramp), walks it with move_along_path, lights the lantern, rests
// a moment and goes on. The way ahead is drawn on the ground as it walks.
//
// Under `flint playtest` it passes once the fourth lantern is lit, and fails
// if any lantern has no way to it.

let lanterns = ["lantern_sw", "lantern_se", "lantern_nw", "lantern_terrace"];
let at = 0;
let path = [];
let next = 0;
let rest = 0.6;

fn on_update() {
    if at >= lanterns.len() {
        return;
    }
    let me = self_entity();
    if rest > 0.0 {
        rest -= delta_time();
        return;
    }
    let lantern = get_entity(lanterns[at]);
    if path.is_empty() {
        let p = get_position(me);
        let to = get_position(lantern);
        path = find_path(p.x, p.y, p.z, to.x, to.y, to.z);
        test_assert(path.len() >= 2, "a way to " + lanterns[at]);
        test_log("points to " + lanterns[at], path.len());
        if path.is_empty() {
            at += 1;
            return;
        }
    }

    let step = move_along_path(me, path, 2.6);
    next = step.next;
    if step.dir_x != 0.0 || step.dir_z != 0.0 {
        set_rotation(me, 0.0, atan2(step.dir_x, step.dir_z) * 180.0 / PI(), 0.0);
    }
    if step.arrived {
        // The glass's light comes up over most of a second
        tween(get_entity(lanterns[at] + "_glass"), "material.emissive_scale", 1.0, 0.8, "ease_out");
        test_log("lit", at + 1);
        at += 1;
        path = [];
        rest = 0.9;
        if at == lanterns.len() {
            test_pass();
        }
    }
}

fn on_draw_ui() {
    // The way still to walk, a hand's width above the ground: from where
    // it stands (its middle is 0.75 m up) to the point it walks to, and on
    if path.len() < 2 || next < 1 {
        return;
    }
    let p = get_position(self_entity());
    let from = #{ x: p.x, y: p.y - 0.75, z: p.z };
    for i in next..path.len() {
        let q = path[i];
        draw_world_line(from.x, from.y + 0.06, from.z, q.x, q.y + 0.06, q.z,
            #{ color: [1.0, 0.72, 0.35, 0.6], thickness: 3.0 });
        from = q;
    }
}
