// ─── tavern_bartender.rhai ───────────────────────────────
// The bartender gives hints about the ruins beyond.
// Proximity awareness + dialog about expeditions.

let base_speed = 1.0;
let greeted = false;

fn on_init() {
    let me = self_entity();
    let spd = get_field(me, "animator", "speed");
    if spd != () {
        base_speed = spd;
    }
}

fn on_update() {
    let dt = delta_time();
    let me = self_entity();
    let player = get_entity("player");
    if player < 0 { return; }

    let dist = distance(me, player);

    if dist < 6.0 {
        let alertness = clamp(1.0 - (dist - 3.0) / 3.0, 0.0, 1.0);
        let speed = lerp(base_speed, base_speed * 1.4, alertness);
        set_anim_speed(me, speed);

        if dist < 3.0 {
            let my_pos = get_position(me);
            let pl_pos = get_position(player);
            let dx = pl_pos.x - my_pos.x;
            let dz = pl_pos.z - my_pos.z;
            let angle = atan2(dx, dz) * 57.2958;
            let rot = get_rotation(me);
            let new_y = lerp(rot.y, angle, dt * 3.0);
            set_rotation(me, rot.x, new_y, rot.z);
        }
    } else {
        set_anim_speed(me, base_speed);
    }
}

fn on_interact() {
    let me = self_entity();
    play_sound("audio/npc_greeting.wav");

    if !greeted {
        log("\"Welcome, explorer! The ruins beyond hold ancient essence -- glowing sparks of power.\"");
        greeted = true;
    } else {
        let essence = persist_get("essence_collected");
        if type_of(essence) == "()" { essence = 0; }

        if essence < 3 {
            let r = floor(random() * 3.0);
            if r < 1.0 {
                log("\"The Sunken Crypt is open -- dark, but full of essence. Watch for beetles.\"");
            } else if r < 2.0 {
                log("\"Collect more essence to unlock the deeper ruins. The garden awaits...\"");
            } else {
                log("\"Those mushrooms in the crypt... they glow like nothing I've seen before.\"");
            }
        } else if essence < 7 {
            let r = floor(random() * 3.0);
            if r < 1.0 {
                log("\"The Flooded Garden is open now! Nature has truly reclaimed it.\"");
            } else if r < 2.0 {
                log("\"Watch the water striders in the garden -- strange creatures.\"");
            } else {
                log("\"A few more essence and the library doors should yield.\"");
            }
        } else {
            let r = floor(random() * 2.0);
            if r < 1.0 {
                log("\"All three ruins are open. You're a true explorer!\"");
            } else {
                log("\"The Collapsed Library holds the oldest secrets. The roots have broken through...\"");
            }
        }
    }

    set_anim_speed(me, base_speed * 1.6);
}
