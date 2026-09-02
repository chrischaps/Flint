// Showcase Anvil — spark bursts on interaction with persistent counter
// Demonstrates: emit_burst(), persist_get/set(), set_bloom_intensity()

let sparks_emitter = -1;
let bloom_flash_timer = 0.0;

fn on_init() {
    // Find the forge_sparks emitter entity for burst emission
    sparks_emitter = get_entity("forge_sparks");
}

fn on_update() {
    let dt = delta_time();

    // Brief bloom flash after anvil strike
    if bloom_flash_timer > 0.0 {
        bloom_flash_timer -= dt;
        let flash = clamp(bloom_flash_timer / 0.3, 0.0, 1.0);
        set_bloom_intensity(0.10 + flash * 0.25);
    }
}

fn on_interact() {
    // Spark burst from forge
    if sparks_emitter >= 0 {
        emit_burst(sparks_emitter, 40);
    }

    // Metallic clang
    let me = self_entity();
    let pos = get_position(me);
    play_sound_at("audio/door_locked.wav", pos.x, pos.y, pos.z, 0.7);

    // Increment spark counter
    let sparks = persist_get("sparks_collected");
    if type_of(sparks) == "()" {
        sparks = 0;
    }
    sparks += 3;
    persist_set("sparks_collected", sparks);

    // Trigger bloom flash
    bloom_flash_timer = 0.3;

    log("[Sparks fly from the anvil! +" + 3 + " sparks collected]");
}
