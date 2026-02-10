// Hazard zone — periodic environmental damage (nukage, lava, etc.)
//
// Attached to a logic entity with a `hazard` component defining rectangular
// bounds and damage parameters. Checks each frame whether the player is inside
// the zone and below `surface_y`, then applies periodic damage ticks.

// ─── Helpers ──

fn to_float(val) {
    if type_of(val) == "f64" { return val; }
    if type_of(val) == "i64" { return val + 0.0; }
    return 0.0;
}

fn deal_damage(target_id, amount) {
    let current = get_field(target_id, "health", "current");
    if current == () { return false; }
    let invulnerable = get_field(target_id, "health", "invulnerable");
    if invulnerable == true { return false; }

    // Armor absorption
    if has_component(target_id, "armor") {
        let armor = get_component(target_id, "armor");
        let armor_current = to_float(armor.current);
        if armor_current > 0.0 {
            let absorption = to_float(armor.absorption);
            let absorbed = amount * absorption;
            let armor_damage = if absorbed < armor_current { absorbed } else { armor_current };
            set_field(target_id, "armor", "current", armor_current - armor_damage);
            amount -= armor_damage;
        }
    }

    let new_health = if current - amount > 0.0 { current - amount } else { 0.0 };
    set_field(target_id, "health", "current", new_health);
    set_field(target_id, "health", "damage_flash_timer", 0.3);
    if new_health <= 0.0 {
        set_field(target_id, "health", "dead", true);
        return true;
    }
    return false;
}

fn find_player() {
    let id = get_entity("player");
    if id < 0 { return -1; }
    if !has_component(id, "character_controller") { return -1; }
    return id;
}

// ─── Per-frame update ──

fn on_update(dt) {
    let me = this_entity();
    let hz = get_component(me, "hazard");
    if hz == () { return; }

    let player_id = find_player();
    if player_id < 0 { return; }

    // Check if player is already dead
    let dead = get_field(player_id, "health", "dead");
    if dead == true { return; }

    let pos = get_position(player_id);
    if pos == () { return; }

    let px = to_float(pos.x);
    let py = to_float(pos.y);
    let pz = to_float(pos.z);

    let min_x = to_float(hz.min_x);
    let max_x = to_float(hz.max_x);
    let min_z = to_float(hz.min_z);
    let max_z = to_float(hz.max_z);
    let surface_y = to_float(hz.surface_y);

    // Player must be within XZ bounds and below the surface threshold
    if px < min_x || px > max_x { return; }
    if pz < min_z || pz > max_z { return; }
    if py > surface_y { return; }

    // Player is in the hazard — count down tick timer
    let timer = to_float(hz.tick_timer);
    timer -= dt;

    if timer <= 0.0 {
        // Apply damage tick
        let dps = to_float(hz.dps);
        let interval = to_float(hz.tick_interval);
        let tick_damage = dps * interval;

        deal_damage(player_id, tick_damage);

        // Reset timer for next tick
        timer = interval;
    }

    set_field(me, "hazard", "tick_timer", timer);
}
