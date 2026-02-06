// ─── npc_mysterious_stranger.rhai ────────────────────────
// The mysterious stranger: unsettling presence in the corner.
// Very slow idle. Freezes when player approaches. Cryptic on interact.

let base_speed = 0.5;
let freeze_timer = 0.0;
let interacted = false;

fn on_init() {
    let me = self_entity();
    set_anim_speed(me, base_speed);
    log("The stranger watches from the shadows...");
}

fn on_update(dt) {
    let me = self_entity();
    let player = get_entity("player");
    if player < 0 { return; }

    let dist = distance(me, player);

    // Post-interact recovery
    if freeze_timer > 0.0 {
        freeze_timer -= dt;
        let recovery = clamp(1.0 - freeze_timer / 2.0, 0.0, 1.0);
        set_anim_speed(me, lerp(base_speed * 1.8, base_speed, recovery));
        return;
    }

    // Proximity effect: the closer you get, the more the stranger freezes
    if dist < 5.0 {
        let closeness = clamp(1.0 - (dist - 2.0) / 3.0, 0.0, 1.0);
        // Animation slows to near-zero — the stranger is watching
        let speed = lerp(base_speed, 0.05, closeness);
        set_anim_speed(me, speed);
    } else {
        set_anim_speed(me, base_speed);
    }
}

fn on_interact() {
    let me = self_entity();
    play_sound("audio/npc_greeting.wav");

    if !interacted {
        log("\"...You see it too, don't you? The flicker in the walls.\"");
        interacted = true;
    } else {
        let r = floor(random() * 3.0);
        if r < 1.0 {
            log("\"The fire remembers things the stone has forgotten.\"");
        } else if r < 2.0 {
            log("\"...\"");
        } else {
            log("\"Some doors are better left closed, friend.\"");
        }
    }

    // Brief speed burst before returning to slow
    freeze_timer = 2.0;
}
