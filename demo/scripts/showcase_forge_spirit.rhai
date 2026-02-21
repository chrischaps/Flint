// Forge Spirit NPC — reads persistent spark count for varied dialogue
// Demonstrates: persist_get() for cross-scene persistent state

let base_speed = 0.6;
let greeted = false;

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
    let t = total_time();

    // Breathing idle variation
    let breathe = sin(t * 1.0 + 3.7) * 0.06;
    let speed = base_speed + breathe;

    // Face player when close
    if dist < 6.0 {
        let alertness = clamp(1.0 - (dist - 3.0) / 3.0, 0.0, 1.0);
        speed = lerp(speed, speed * 1.3, alertness);

        if dist < 4.0 {
            let my_pos = get_position(me);
            let pl_pos = get_position(player);
            let dx = pl_pos.x - my_pos.x;
            let dz = pl_pos.z - my_pos.z;
            let angle = atan2(dx, dz) * 57.2958;
            let rot = get_rotation(me);
            let new_y = lerp(rot.y, angle, dt * 3.0);
            set_rotation(me, rot.x, new_y, rot.z);
        }
    }

    set_anim_speed(me, speed);
}

fn on_interact() {
    let me = self_entity();
    play_sound("audio/npc_greeting.wav");

    let sparks = persist_get("sparks_collected");
    if type_of(sparks) == "()" {
        sparks = 0;
    }

    if !greeted {
        greeted = true;
        if sparks < 3 {
            log("Forge Spirit: \"Another wanderer stumbles into my crucible...\"");
            log("Forge Spirit: \"You carry so few sparks. The forge demands more.\"");
        } else if sparks < 6 {
            log("Forge Spirit: \"Ah, you bring " + sparks + " sparks with you.\"");
            log("Forge Spirit: \"The fire recognizes a kindred soul. Strike the anvil — feed the flame.\"");
        } else {
            log("Forge Spirit: \"" + sparks + " sparks! The crucible blazes at your approach!\"");
            log("Forge Spirit: \"You have the heart of a true forgemaster.\"");
        }
    } else {
        if sparks < 3 {
            let r = floor(random() * 3.0);
            if r < 1.0 {
                log("Forge Spirit: \"The anvil waits. Strike it, and sparks will come.\"");
            } else if r < 2.0 {
                log("Forge Spirit: \"This place was ancient before the tavern above was built.\"");
            } else {
                log("Forge Spirit: \"Flint and steel. That is all creation requires.\"");
            }
        } else if sparks < 6 {
            let r = floor(random() * 3.0);
            if r < 1.0 {
                log("Forge Spirit: \"" + sparks + " sparks now. The metal grows warm.\"");
            } else if r < 2.0 {
                log("Forge Spirit: \"Each spark remembers the fire it came from.\"");
            } else {
                log("Forge Spirit: \"Keep striking. The best steel is forged in repetition.\"");
            }
        } else {
            let r = floor(random() * 3.0);
            if r < 1.0 {
                log("Forge Spirit: \"" + sparks + " sparks! You illuminate the darkness.\"");
            } else if r < 2.0 {
                log("Forge Spirit: \"The crucible sings with your fire.\"");
            } else {
                log("Forge Spirit: \"When enough sparks gather, even stone must yield.\"");
            }
        }
    }

    set_anim_speed(me, base_speed * 1.5);
}
