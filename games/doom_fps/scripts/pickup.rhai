// Pickup item script — health and ammo collection
//
// Checks distance to player each frame. When player is within range
// and needs the resource, applies the effect and deactivates.

let bob_offset = 0.0;
let initial_y = 0.0;
let initialized = false;

// ─── Combat helpers (game-level, using generic ECS access) ──

fn to_int_val(v) {
    if type_of(v) == "i64" { return v; }
    if type_of(v) == "f64" { return v.floor(); }
    return 0;
}

fn get_health(entity_id) {
    let h = get_field(entity_id, "health", "current");
    if h == () { return 0.0; }
    return h;
}

fn heal(entity_id, amount) {
    let current = get_field(entity_id, "health", "current");
    if current == () { return; }
    let max_hp = get_field(entity_id, "health", "max");
    if max_hp == () { max_hp = 100.0; }
    let new_hp = current + amount;
    if new_hp > max_hp { new_hp = max_hp; }
    set_field(entity_id, "health", "current", new_hp);
}

fn on_init() {
    let pos = get_position(this_entity());
    initial_y = pos.y;
}

fn on_update(dt) {
    let pickup = get_component(this_entity(), "pickup");
    if pickup == () { return; }

    let active = pickup.active;
    if type_of(active) != "bool" { active = true; }

    if !active {
        // Handle respawn timer
        handle_respawn(dt, pickup);
        return;
    }

    // Gentle bobbing animation
    bob_offset += dt * 2.0;
    let bob = sin(bob_offset) * 0.1;
    let pos = get_position(this_entity());
    set_position(this_entity(), pos.x, initial_y + bob, pos.z);

    // Slow rotation
    let rot = get_rotation(this_entity());
    set_rotation(this_entity(), rot.x, rot.y + dt * 1.5, rot.z);

    // Check distance to player
    let player_id = find_player();
    if player_id < 0 { return; }

    let pickup_range = pickup.range;
    if type_of(pickup_range) == "i64" { pickup_range = pickup_range + 0.0; }

    let my_pos = get_position(this_entity());
    let player_pos = get_position(player_id);

    let dx = player_pos.x - my_pos.x;
    let dy = player_pos.y - my_pos.y;
    let dz = player_pos.z - my_pos.z;
    let dist = sqrt(dx*dx + dy*dy + dz*dz);

    if dist <= pickup_range {
        try_collect(pickup, player_id);
    }
}

fn find_player() {
    let id = get_entity("player");
    if id < 0 { return -1; }
    if !has_component(id, "character_controller") { return -1; }
    return id;
}

fn try_collect(pickup, player_id) {
    let pickup_type = pickup.pickup_type;
    let amount = pickup.amount;
    if type_of(amount) == "i64" { amount = amount + 0.0; }

    if pickup_type == "weapon_shotgun" {
        // Shotgun weapon pickup — grant weapon + initial shells
        if !has_component(player_id, "weapon_inventory") { return; }
        let inv = get_component(player_id, "weapon_inventory");
        let already_has = inv.has_shotgun;

        if already_has == true {
            // Already have shotgun — just add shells
            let current_shells = to_int_val(inv.shotgun_ammo);
            let max_shells = to_int_val(inv.shotgun_max_ammo);
            if current_shells >= max_shells { return; } // full

            let new_shells = current_shells + amount.to_int();
            if new_shells > max_shells { new_shells = max_shells; }
            set_field(player_id, "weapon_inventory", "shotgun_ammo", new_shells);
            set_field(player_id, "weapon_inventory", "switch_pending", true);
            play_sound("audio/pickup_ammo.ogg", 0.7);
            log_info("Picked up shells +" + amount.to_int());
        } else {
            // First shotgun pickup!
            set_field(player_id, "weapon_inventory", "has_shotgun", true);
            let new_shells = amount.to_int();
            let max_shells = to_int_val(inv.shotgun_max_ammo);
            if new_shells > max_shells { new_shells = max_shells; }
            set_field(player_id, "weapon_inventory", "shotgun_ammo", new_shells);
            set_field(player_id, "weapon_inventory", "switch_pending", true);
            play_sound("audio/pickup_ammo.ogg", 0.8);
            log_info("Picked up Shotgun!");
        }
    } else if pickup_type == "shells" {
        // Shotgun shell ammo pickup
        if !has_component(player_id, "weapon_inventory") { return; }
        let inv = get_component(player_id, "weapon_inventory");

        let current_shells = to_int_val(inv.shotgun_ammo);
        let max_shells = to_int_val(inv.shotgun_max_ammo);
        if current_shells >= max_shells { return; } // full

        let new_shells = current_shells + amount.to_int();
        if new_shells > max_shells { new_shells = max_shells; }
        set_field(player_id, "weapon_inventory", "shotgun_ammo", new_shells);
        set_field(player_id, "weapon_inventory", "switch_pending", true);
        play_sound("audio/pickup_ammo.ogg", 0.7);
        log_info("Picked up shells +" + amount.to_int());
    } else if pickup_type == "health" {
        // Check if player needs health
        let current_health = get_health(player_id);
        if current_health <= 0.0 { return; } // dead player can't pickup

        let health_comp = get_component(player_id, "health");
        if health_comp == () { return; }
        let max_health = health_comp.max;
        if type_of(max_health) == "i64" { max_health = max_health + 0.0; }

        if current_health >= max_health { return; } // already full

        heal(player_id, amount);
        play_sound("audio/pickup_health.ogg", 0.7);
        log_info("Picked up health +" + amount);
    } else if pickup_type == "ammo" {
        // Pistol ammo pickup — always adds to pistol ammo via inventory
        if has_component(player_id, "weapon_inventory") {
            let inv = get_component(player_id, "weapon_inventory");
            let current_ammo = to_int_val(inv.pistol_ammo);
            let max_ammo = to_int_val(inv.pistol_max_ammo);
            if current_ammo >= max_ammo { return; } // already full

            let new_ammo = current_ammo + amount.to_int();
            if new_ammo > max_ammo { new_ammo = max_ammo; }
            set_field(player_id, "weapon_inventory", "pistol_ammo", new_ammo);
            set_field(player_id, "weapon_inventory", "switch_pending", true);
        } else {
            // Fallback: no inventory, write directly to weapon component
            let weapon = get_component(player_id, "weapon");
            if weapon == () { return; }

            let ammo = weapon.ammo;
            let max_ammo = weapon.max_ammo;
            if type_of(ammo) == "f64" { ammo = ammo.to_int(); }
            if type_of(max_ammo) == "f64" { max_ammo = max_ammo.to_int(); }

            if ammo >= max_ammo { return; }

            let new_ammo = ammo + amount.to_int();
            if new_ammo > max_ammo { new_ammo = max_ammo; }
            set_field(player_id, "weapon", "ammo", new_ammo);
        }
        play_sound("audio/pickup_ammo.ogg", 0.7);
        log_info("Picked up ammo +" + amount.to_int());
    } else if pickup_type == "armor" {
        // Check if player needs armor
        if !has_component(player_id, "armor") { return; }
        let armor = get_component(player_id, "armor");
        let current_armor = armor.current;
        if type_of(current_armor) == "i64" { current_armor = current_armor + 0.0; }
        let max_armor = armor.max;
        if type_of(max_armor) == "i64" { max_armor = max_armor + 0.0; }

        if current_armor >= max_armor { return; } // already full

        let new_armor = current_armor + amount;
        if new_armor > max_armor { new_armor = max_armor; }
        set_field(player_id, "armor", "current", new_armor);
        play_sound("audio/pickup_health.ogg", 0.7);
        log_info("Picked up armor +" + amount);
    }

    // Deactivate pickup
    set_field(this_entity(), "pickup", "active", false);
    set_field(this_entity(), "sprite", "visible", false);
}

fn handle_respawn(dt, pickup) {
    let respawn_time = pickup.respawn_time;
    if type_of(respawn_time) == "i64" { respawn_time = respawn_time + 0.0; }

    if respawn_time <= 0.0 { return; } // no respawn

    let timer = pickup.respawn_timer;
    if type_of(timer) == "()" { timer = 0.0; }
    if type_of(timer) == "i64" { timer = timer + 0.0; }

    timer += dt;

    if timer >= respawn_time {
        // Respawn!
        set_field(this_entity(), "pickup", "active", true);
        set_field(this_entity(), "pickup", "respawn_timer", 0.0);
        set_field(this_entity(), "sprite", "visible", true);
        play_sound("audio/pickup_respawn.ogg", 0.4);
    } else {
        set_field(this_entity(), "pickup", "respawn_timer", timer);
    }
}
