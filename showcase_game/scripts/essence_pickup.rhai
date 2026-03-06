// ─── essence_pickup.rhai ─────────────────────────────────
// Collectible essence spark. Bobs gently in place.
// On interact, increments the essence counter and logs collection.

let base_y = 0.0;
let collected = false;

fn on_init() {
    let me = self_entity();
    let pos = get_position(me);
    base_y = pos.y;
}

fn on_update() {
    if collected { return; }
    let me = self_entity();
    let t = total_time();
    let pos = get_position(me);
    let new_y = base_y + sin(t * 2.0) * 0.15;
    set_position(me, pos.x, new_y, pos.z);
}

fn on_interact() {
    if collected { return; }
    collected = true;

    let me = self_entity();
    let essence = persist_get("essence_collected");
    if type_of(essence) == "()" {
        essence = 0;
    }
    essence += 1;
    persist_set("essence_collected", essence);

    play_sound("audio/glasses_clink.ogg");
    log("Essence collected! Total: " + essence);

    // Hide the pickup
    set_position(me, 0.0, -100.0, 0.0);
    set_field(me, "interactable", "enabled", false);
}
