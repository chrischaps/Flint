// Enemy AI — hitscan ranged attacker (Zombieman / Shotgun Guy)
//
// States: idle → alert → chase → attack → pain → dead
// Uses raycast for line-of-sight checks AND ranged hitscan attacks.
// Direct movement toward player (no pathfinding — authentic Doom AI).
// Supports multi-pellet spread (shotgun guy fires 3 pellets).
// Infighting: rays hit any entity with health, not just the player.

// alert_timer and death_timer stored in enemy ECS component (not local vars)
// to allow access from helper functions called within on_update.
let walk_frame_timer = 0.0;

// ─── Combat helpers (game-level, using generic ECS access) ──

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

fn is_dead(entity_id) {
    let d = get_field(entity_id, "health", "dead");
    if d == () { return false; }
    return d;
}

fn on_init() {
    // Start idle
    set_field(this_entity(), "enemy", "state", "idle");
}

fn on_update(dt) {
    let enemy = get_component(this_entity(), "enemy");
    if enemy == () { return; }

    let state = enemy.state;
    if type_of(state) != "string" { state = "idle"; }

    // Get config values with type safety
    let sight_range = to_float(enemy.sight_range);
    let move_speed = to_float(enemy.move_speed);
    let attack_range = to_float(enemy.attack_range);
    let attack_damage = to_float(enemy.attack_damage);
    let attack_rate = to_float(enemy.attack_rate);
    let despawn_time = to_float(enemy.despawn_timer);

    // Tick attack cooldown
    let attack_cd = to_float(enemy.attack_cooldown);
    if attack_cd > 0.0 {
        attack_cd -= dt;
        if attack_cd < 0.0 { attack_cd = 0.0; }
        set_field(this_entity(), "enemy", "attack_cooldown", attack_cd);
    }

    // Tick pain timer
    let pain_t = to_float(enemy.pain_timer);
    if pain_t > 0.0 {
        pain_t -= dt;
        if pain_t < 0.0 { pain_t = 0.0; }
        set_field(this_entity(), "enemy", "pain_timer", pain_t);
    }

    if state == "dead" {
        update_dead(dt, despawn_time);
        return;
    }

    if state == "pain" {
        update_pain(dt, pain_t);
        return;
    }

    // Check if we should enter pain state
    let health_comp = get_component(this_entity(), "health");
    if health_comp != () {
        let dmg_flash = to_float(health_comp.damage_flash_timer);
        if dmg_flash > 0.25 {
            // Just took damage — enter pain
            enter_pain();
            return;
        }

        // Check for death
        if is_dead(this_entity()) {
            enter_dead();
            return;
        }
    }

    // Find player — look for entity with character_controller
    let player_id = find_player();
    if player_id < 0 { return; }

    let my_pos = get_position(this_entity());
    let player_pos = get_position(player_id);

    let dx = player_pos.x - my_pos.x;
    let dy = player_pos.y - my_pos.y;
    let dz = player_pos.z - my_pos.z;
    let dist = sqrt(dx*dx + dy*dy + dz*dz);

    if state == "idle" {
        update_idle(dist, sight_range, my_pos, player_pos, player_id);
    } else if state == "alert" {
        update_alert(dt);
    } else if state == "chase" {
        update_chase(dt, dist, dx, dy, dz, move_speed, attack_range, my_pos);
    } else if state == "attack" {
        update_attack(dt, dist, attack_range, attack_damage, attack_rate, attack_cd, player_id, sight_range, my_pos, player_pos);
    }

    // Animate sprite walk cycle during chase
    if state == "chase" {
        walk_frame_timer += dt;
        if walk_frame_timer > 0.2 {
            walk_frame_timer -= 0.2;
            let sprite = get_component(this_entity(), "sprite");
            if sprite != () {
                let frame = sprite.frame;
                if type_of(frame) == "f64" { frame = to_int(frame); }
                if type_of(frame) != "i64" { frame = 0; }
                frame = (frame + 1) % 4; // cycle through 4 walk frames
                set_field(this_entity(), "sprite", "frame", frame);
            }
        }
    }
}

fn to_float(val) {
    if type_of(val) == "f64" { return val; }
    if type_of(val) == "i64" { return val + 0.0; }
    return 0.0;
}

fn to_int(val) {
    if type_of(val) == "i64" { return val; }
    if type_of(val) == "f64" { return val.to_int(); }
    return 0;
}

fn find_player() {
    let id = get_entity("player");
    if id < 0 { return -1; }
    if !has_component(id, "character_controller") { return -1; }
    return id;
}

fn update_idle(dist, sight_range, my_pos, player_pos, player_id) {
    // Check if player is within sight range
    if dist > sight_range { return; }

    // Check line of sight via raycast
    let dx = player_pos.x - my_pos.x;
    let dy = player_pos.y - my_pos.y;
    let dz = player_pos.z - my_pos.z;
    let len = sqrt(dx*dx + dy*dy + dz*dz);
    if len < 0.01 { return; }

    let hit = raycast(
        my_pos.x, my_pos.y + 0.8, my_pos.z,
        dx/len, dy/len, dz/len,
        sight_range
    );

    if hit != () && hit.entity == player_id {
        // Player spotted!
        set_field(this_entity(), "enemy", "state", "alert");
        set_field(this_entity(), "enemy", "alert_timer", 0.5); // brief alert pause before chasing
        set_field(this_entity(), "sprite", "frame", 4); // alert frame

        // Play alert sound
        let enemy = get_component(this_entity(), "enemy");
        if enemy != () {
            let snd = enemy.alert_sound;
            if type_of(snd) == "string" && snd != "" {
                play_sound_at(snd, my_pos.x, my_pos.y, my_pos.z, 0.7);
            }
        }
    }
}

fn update_alert(dt) {
    let at = to_float(get_field(this_entity(), "enemy", "alert_timer"));
    at -= dt;
    if at <= 0.0 {
        set_field(this_entity(), "enemy", "state", "chase");
        set_field(this_entity(), "sprite", "frame", 0);
    }
    set_field(this_entity(), "enemy", "alert_timer", at);
}

fn update_chase(dt, dist, dx, dy, dz, move_speed, attack_range, my_pos) {
    // Close enough to attack?
    if dist <= attack_range {
        set_field(this_entity(), "enemy", "state", "attack");
        set_field(this_entity(), "sprite", "frame", 5); // attack frame
        return;
    }

    // Move toward player
    let len = sqrt(dx*dx + dz*dz); // horizontal only
    if len < 0.01 { return; }

    let step = move_speed * dt;
    let new_x = my_pos.x + (dx/len) * step;
    let new_z = my_pos.z + (dz/len) * step;

    set_position(this_entity(), new_x, my_pos.y, new_z);

    // Face player
    let angle = atan2(dx, dz);
    set_rotation(this_entity(), 0.0, angle, 0.0);
}

fn update_attack(dt, dist, attack_range, attack_damage, attack_rate, attack_cd, player_id, sight_range, my_pos, player_pos) {
    // Out of range? Chase again
    if dist > attack_range * 1.5 {
        set_field(this_entity(), "enemy", "state", "chase");
        return;
    }

    // Face the player while attacking
    let dx = player_pos.x - my_pos.x;
    let dz = player_pos.z - my_pos.z;
    let angle = atan2(dx, dz);
    set_rotation(this_entity(), 0.0, angle, 0.0);

    // Attack on cooldown
    if attack_cd > 0.0 { return; }

    // Check line-of-sight before firing — don't shoot through walls
    let los_dx = player_pos.x - my_pos.x;
    let los_dy = player_pos.y - my_pos.y;
    let los_dz = player_pos.z - my_pos.z;
    let los_len = sqrt(los_dx*los_dx + los_dy*los_dy + los_dz*los_dz);
    if los_len < 0.01 { return; }

    let los_hit = raycast(
        my_pos.x, my_pos.y + 0.8, my_pos.z,
        los_dx/los_len, los_dy/los_len, los_dz/los_len,
        sight_range
    );

    if los_hit == () || los_hit.entity != player_id {
        // LOS blocked by wall — chase instead
        set_field(this_entity(), "enemy", "state", "chase");
        set_field(this_entity(), "sprite", "frame", 0);
        return;
    }

    // Read hitscan config from enemy component
    let enemy = get_component(this_entity(), "enemy");
    let spread = 0.0;
    let pellets = 1;
    let attack_snd = "";
    if enemy != () {
        spread = to_float(enemy.attack_spread);
        pellets = to_int(enemy.pellet_count);
        if pellets < 1 { pellets = 1; }
        let s = enemy.attack_sound;
        if type_of(s) == "string" { attack_snd = s; }
    }

    // Set cooldown
    set_field(this_entity(), "enemy", "attack_cooldown", attack_rate);

    // Play attack sound
    if attack_snd != "" {
        play_sound_at(attack_snd, my_pos.x, my_pos.y, my_pos.z, 0.6);
    }

    set_field(this_entity(), "sprite", "frame", 5); // attack frame

    // Fire hitscan rays toward player
    let eye_x = my_pos.x;
    let eye_y = my_pos.y + 0.8;
    let eye_z = my_pos.z;

    // Direction toward player center mass
    let fx = player_pos.x - eye_x;
    let fy = (player_pos.y + 0.5) - eye_y; // aim at player center mass
    let fz = player_pos.z - eye_z;
    let flen = sqrt(fx*fx + fy*fy + fz*fz);
    if flen < 0.01 { return; }
    fx = fx / flen;
    fy = fy / flen;
    fz = fz / flen;

    if pellets == 1 && spread > 0.0 {
        // Single pellet with inaccuracy spread
        let h_off = random_range(-spread, spread);
        let v_off = random_range(-spread, spread);

        // Compute right vector: forward × (0,1,0)
        let rx = -fz;
        let ry = 0.0;
        let rz = fx;
        let rlen = sqrt(rx*rx + rz*rz);
        if rlen > 0.001 { rx = rx / rlen; rz = rz / rlen; }

        // Up vector: right × forward
        let ux = ry * fz - rz * fy;
        let uy = rz * fx - rx * fz;
        let uz = rx * fy - ry * fx;

        let dx = fx + rx * h_off + ux * v_off;
        let dy = fy + ry * h_off + uy * v_off;
        let dz = fz + rz * h_off + uz * v_off;
        let dlen = sqrt(dx*dx + dy*dy + dz*dz);
        if dlen > 0.001 { dx = dx / dlen; dy = dy / dlen; dz = dz / dlen; }

        fire_hitscan_ray(eye_x, eye_y, eye_z, dx, dy, dz, sight_range, attack_damage);
    } else if pellets > 1 {
        // Multi-pellet spread (shotgun style)
        // Compute right vector: forward × (0,1,0)
        let rx = -fz;
        let ry = 0.0;
        let rz = fx;
        let rlen = sqrt(rx*rx + rz*rz);
        if rlen > 0.001 { rx = rx / rlen; rz = rz / rlen; }

        // Up vector: right × forward
        let ux = ry * fz - rz * fy;
        let uy = rz * fx - rx * fz;
        let uz = rx * fy - ry * fx;

        let i = 0;
        while i < pellets {
            let h_off = random_range(-spread, spread);
            let v_off = random_range(-spread, spread);

            let dx = fx + rx * h_off + ux * v_off;
            let dy = fy + ry * h_off + uy * v_off;
            let dz = fz + rz * h_off + uz * v_off;
            let dlen = sqrt(dx*dx + dy*dy + dz*dz);
            if dlen > 0.001 { dx = dx / dlen; dy = dy / dlen; dz = dz / dlen; }

            fire_hitscan_ray(eye_x, eye_y, eye_z, dx, dy, dz, sight_range, attack_damage);
            i += 1;
        }
    } else {
        // Single pellet, no spread — perfect accuracy
        fire_hitscan_ray(eye_x, eye_y, eye_z, fx, fy, fz, sight_range, attack_damage);
    }
}

fn fire_hitscan_ray(ox, oy, oz, dx, dy, dz, max_dist, damage) {
    let hit = raycast(ox, oy, oz, dx, dy, dz, max_dist);
    if hit != () {
        let target_id = hit.entity;
        // Hit any entity with health — enables infighting
        let target_health = get_field(target_id, "health", "current");
        if target_health != () && target_health > 0.0 {
            deal_damage(target_id, damage);
        }
    }
}

fn enter_pain() {
    set_field(this_entity(), "enemy", "state", "pain");
    set_field(this_entity(), "enemy", "pain_timer", 0.4);
    set_field(this_entity(), "sprite", "frame", 6); // pain frame
    set_field(this_entity(), "enemy", "accumulated_damage", 0.0);
}

fn update_pain(dt, pain_t) {
    if pain_t <= 0.0 {
        // Recover — go back to chasing
        set_field(this_entity(), "enemy", "state", "chase");
        set_field(this_entity(), "sprite", "frame", 0);
    }
}

fn enter_dead() {
    set_field(this_entity(), "enemy", "state", "dead");
    set_field(this_entity(), "sprite", "frame", 7); // death frame
    set_field(this_entity(), "enemy", "death_timer", 0.0);

    // Make collider a sensor (non-solid)
    set_field(this_entity(), "collider", "is_sensor", true);

    // Play death sound
    let enemy = get_component(this_entity(), "enemy");
    if enemy != () {
        let snd = enemy.death_sound;
        if type_of(snd) == "string" && snd != "" {
            let pos = get_position(this_entity());
            play_sound_at(snd, pos.x, pos.y, pos.z, 0.7);
        }
    }
}

fn update_dead(dt, despawn_time) {
    let dt_val = to_float(get_field(this_entity(), "enemy", "death_timer"));
    dt_val += dt;
    set_field(this_entity(), "enemy", "death_timer", dt_val);

    // Sink into the ground after a delay
    if dt_val > despawn_time * 0.7 {
        let pos = get_position(this_entity());
        let sink_rate = 0.5 * dt;
        set_position(this_entity(), pos.x, pos.y - sink_rate, pos.z);
    }

    // Hide sprite after full despawn
    if dt_val > despawn_time {
        set_field(this_entity(), "sprite", "visible", false);
    }
}
