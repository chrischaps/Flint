// Showcase Stranger — reveals hidden passage on second interaction
// Demonstrates: persist_set(), persist_get(), persist_has()
// Behavior: unsettling freeze on approach, cryptic dialogue, passage reveal

let base_speed = 0.5;
let freeze_timer = 0.0;
let interact_count = 0;

fn on_init() {
    let me = self_entity();
    let spd = get_field(me, "animator", "speed");
    if type_of(spd) != "()" {
        base_speed = spd;
    }
}

fn on_update() {
    let me = self_entity();
    let player = get_entity("player");
    if player < 0 { return; }

    let dt = delta_time();
    let dist = distance(me, player);

    // Post-interact recovery
    if freeze_timer > 0.0 {
        freeze_timer -= dt;
        let recovery = clamp(1.0 - freeze_timer / 2.0, 0.0, 1.0);
        set_anim_speed(me, lerp(base_speed * 1.8, base_speed, recovery));
        return;
    }

    // Proximity freeze — closer = more frozen
    if dist < 5.0 {
        let closeness = clamp(1.0 - (dist - 2.0) / 3.0, 0.0, 1.0);
        let speed = lerp(base_speed, 0.05, closeness);
        set_anim_speed(me, speed);

        // Face player when very close
        if dist < 3.5 {
            let my_pos = get_position(me);
            let pl_pos = get_position(player);
            let dx = pl_pos.x - my_pos.x;
            let dz = pl_pos.z - my_pos.z;
            let angle = atan2(dx, dz) * 57.2958;
            let rot = get_rotation(me);
            let new_y = lerp(rot.y, angle, dt * 2.5);
            set_rotation(me, rot.x, new_y, rot.z);
        }

        // Chromatic aberration pulse when near
        set_chromatic_aberration(closeness * 0.15);
    } else {
        set_anim_speed(me, base_speed);
        set_chromatic_aberration(0.0);
    }
}

fn on_interact() {
    let me = self_entity();
    interact_count += 1;
    freeze_timer = 2.0;

    play_sound("audio/npc_greeting.wav");

    if interact_count == 1 {
        log("Stranger: \"The walls of this tavern are older than you know...\"");
        log("Stranger: \"They remember things. Things that burn.\"");
    } else if interact_count == 2 {
        log("Stranger: \"Look near the fireplace. The stone remembers...\"");
        log("Stranger: \"Press hard where the mortar has crumbled.\"");
        persist_set("passage_revealed", true);

        // Award sparks for the revelation
        let sparks = persist_get("sparks_collected");
        if type_of(sparks) == "()" {
            sparks = 0;
        }
        sparks += 1;
        persist_set("sparks_collected", sparks);

        log("[A hidden passage has been revealed near the fireplace]");
    } else {
        let r = floor(random() * 4.0);
        if r < 1.0 {
            log("Stranger: \"Every flame starts with a single spark...\"");
        } else if r < 2.0 {
            log("Stranger: \"The crucible waits below. It always has.\"");
        } else if r < 3.0 {
            log("Stranger: \"You carry more light than you know.\"");
        } else {
            log("Stranger: \"...\"");
        }
    }

    set_anim_speed(me, base_speed * 1.8);
}
