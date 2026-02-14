// Player weapon system — hitscan firing, ammo management, weapon switching
//
// Attached to the player entity. Supports pistol (slot 1) and shotgun (slot 2).
// Weapon stats are written into the ECS `weapon` component on switch.
// Cross-script communication via `weapon_inventory` ECS component:
//   - pickup.rhai writes has_shotgun / ammo / switch_pending
//   - This script reads the flag each frame and syncs local state
//
// Module-level vars are authoritative during gameplay; inventory component
// is the shared bus for cross-script changes.

let current_slot = 1;       // 1=pistol, 2=shotgun
let has_shotgun = false;
let pistol_ammo = 50;
let shotgun_ammo = 0;

// ─── Combat helpers (game-level, using generic ECS access) ──

fn to_float(val) {
    if type_of(val) == "f64" { return val; }
    if type_of(val) == "i64" { return val + 0.0; }
    return 0.0;
}

fn to_int_val(val) {
    if type_of(val) == "i64" { return val; }
    if type_of(val) == "f64" { return val.floor(); }
    return 0;
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

fn get_health(entity_id) {
    let h = get_field(entity_id, "health", "current");
    if h == () { return 0.0; }
    return h;
}

// ─── Initialization ──

fn on_init() {
    // Sync from weapon_inventory component if present
    if has_component(this_entity(), "weapon_inventory") {
        let inv = get_component(this_entity(), "weapon_inventory");
        current_slot = to_int_val(inv.current_weapon);
        has_shotgun = inv.has_shotgun;
        pistol_ammo = to_int_val(inv.pistol_ammo);
        shotgun_ammo = to_int_val(inv.shotgun_ammo);
    }
    log_info("Weapon system initialized — slot " + current_slot);
}

// ─── Per-frame update ──

fn on_update() {
    let dt = delta_time();
    let weapon = get_component(this_entity(), "weapon");
    if weapon == () { return; }

    // Tick down weapon cooldown
    let cooldown = weapon.cooldown_remaining;
    if type_of(cooldown) != "()" {
        let cd = cooldown;
        if type_of(cd) == "i64" { cd = cd + 0.0; }
        if cd > 0.0 {
            cd -= dt;
            if cd < 0.0 { cd = 0.0; }
            set_field(this_entity(), "weapon", "cooldown_remaining", cd);
        }
    }

    // Check for cross-script inventory changes (pickup gave us something)
    if has_component(this_entity(), "weapon_inventory") {
        let inv = get_component(this_entity(), "weapon_inventory");
        let pending = inv.switch_pending;
        if pending == true {
            // Sync from inventory
            has_shotgun = inv.has_shotgun;
            pistol_ammo = to_int_val(inv.pistol_ammo);
            shotgun_ammo = to_int_val(inv.shotgun_ammo);

            // If the active weapon's ammo changed, update the weapon component too
            if current_slot == 1 {
                set_field(this_entity(), "weapon", "ammo", pistol_ammo);
            } else {
                set_field(this_entity(), "weapon", "ammo", shotgun_ammo);
            }

            // Clear the flag
            set_field(this_entity(), "weapon_inventory", "switch_pending", false);
        }
    }

    // Sync current weapon ammo back to inventory each frame
    // (so HUD/pickup scripts can read it)
    let current_ammo = to_int_val(weapon.ammo);
    if has_component(this_entity(), "weapon_inventory") {
        if current_slot == 1 {
            pistol_ammo = current_ammo;
            set_field(this_entity(), "weapon_inventory", "pistol_ammo", pistol_ammo);
        } else {
            shotgun_ammo = current_ammo;
            set_field(this_entity(), "weapon_inventory", "shotgun_ammo", shotgun_ammo);
        }
        set_field(this_entity(), "weapon_inventory", "current_weapon", current_slot);
    }
}

// ─── Action handling ──
// NOTE: All switch logic is inlined here because Rhai's call_fn() only
// gives Scope (module-level var) access to the directly-called function.
// Sub-functions would get "Variable not found" errors.

fn on_action(action) {
    if action == "fire" {
        try_fire();
    } else if action == "weapon_1" {
        // ── Switch to pistol (inline) ──
        if current_slot != 1 {
            // Save current (shotgun) ammo from weapon component
            let weapon = get_component(this_entity(), "weapon");
            if weapon != () {
                shotgun_ammo = to_int_val(weapon.ammo);
            }

            current_slot = 1;

            // Load pistol stats into weapon component
            set_field(this_entity(), "weapon", "name", "Pistol");
            set_field(this_entity(), "weapon", "damage", 15.0);
            set_field(this_entity(), "weapon", "fire_rate", 0.4);
            set_field(this_entity(), "weapon", "ammo", pistol_ammo);
            set_field(this_entity(), "weapon", "max_ammo", 200);
            set_field(this_entity(), "weapon", "range", 100.0);
            set_field(this_entity(), "weapon", "spread", 0.0);
            set_field(this_entity(), "weapon", "pellet_count", 1);
            set_field(this_entity(), "weapon", "fire_sound", "audio/gunshot.ogg");
            set_field(this_entity(), "weapon", "cooldown_remaining", 0.0);

            // Update inventory
            if has_component(this_entity(), "weapon_inventory") {
                set_field(this_entity(), "weapon_inventory", "current_weapon", 1);
                set_field(this_entity(), "weapon_inventory", "shotgun_ammo", shotgun_ammo);
                set_field(this_entity(), "weapon_inventory", "pistol_ammo", pistol_ammo);
            }

            log_info("Switched to Pistol");
        }
    } else if action == "weapon_2" {
        // ── Switch to shotgun (inline) ──
        if has_shotgun && current_slot != 2 {
            // Save current (pistol) ammo from weapon component
            let weapon = get_component(this_entity(), "weapon");
            if weapon != () {
                pistol_ammo = to_int_val(weapon.ammo);
            }

            current_slot = 2;

            // Load shotgun stats into weapon component
            set_field(this_entity(), "weapon", "name", "Shotgun");
            set_field(this_entity(), "weapon", "damage", 5.0);
            set_field(this_entity(), "weapon", "fire_rate", 0.8);
            set_field(this_entity(), "weapon", "ammo", shotgun_ammo);
            set_field(this_entity(), "weapon", "max_ammo", 50);
            set_field(this_entity(), "weapon", "range", 50.0);
            set_field(this_entity(), "weapon", "spread", 0.08);
            set_field(this_entity(), "weapon", "pellet_count", 7);
            set_field(this_entity(), "weapon", "fire_sound", "audio/shotgun_fire.ogg");
            set_field(this_entity(), "weapon", "cooldown_remaining", 0.0);

            // Update inventory
            if has_component(this_entity(), "weapon_inventory") {
                set_field(this_entity(), "weapon_inventory", "current_weapon", 2);
                set_field(this_entity(), "weapon_inventory", "pistol_ammo", pistol_ammo);
                set_field(this_entity(), "weapon_inventory", "shotgun_ammo", shotgun_ammo);
            }

            log_info("Switched to Shotgun");
        }
    }
}

// ─── Firing logic (supports multi-pellet spread) ──

fn try_fire() {
    let weapon = get_component(this_entity(), "weapon");
    if weapon == () { return; }

    // Check cooldown
    let cd = weapon.cooldown_remaining;
    if type_of(cd) != "()" {
        if type_of(cd) == "i64" { cd = cd + 0.0; }
        if cd > 0.0 { return; }
    }

    // Check ammo
    let ammo = weapon.ammo;
    if type_of(ammo) == "f64" { ammo = ammo.floor(); }
    if ammo <= 0 { return; }

    // Read weapon stats
    let fire_rate = to_float(weapon.fire_rate);
    let damage = to_float(weapon.damage);
    let range = to_float(weapon.range);
    let spread = to_float(weapon.spread);
    let pellet_count = to_int_val(weapon.pellet_count);
    if pellet_count < 1 { pellet_count = 1; }

    // Set cooldown and decrement ammo
    set_field(this_entity(), "weapon", "cooldown_remaining", fire_rate);
    set_field(this_entity(), "weapon", "ammo", ammo - 1);

    // Play fire sound
    let fire_sound = weapon.fire_sound;
    if type_of(fire_sound) == "string" && fire_sound != "" {
        play_sound(fire_sound, 0.8);
    }

    // Raycast from camera
    let cam_pos = get_camera_position();
    let cam_dir = get_camera_direction();

    let fx = to_float(cam_dir.x);
    let fy = to_float(cam_dir.y);
    let fz = to_float(cam_dir.z);

    if pellet_count == 1 {
        // Single pellet — straight shot (pistol)
        fire_single_ray(cam_pos, fx, fy, fz, range, damage);
    } else {
        // Multi-pellet spread (shotgun)
        // Compute right and up vectors from forward direction
        // right = normalize(forward × (0,1,0))
        let rx = -fz;
        let ry = 0.0;
        let rz = fx;
        let rlen = sqrt(rx*rx + rz*rz);
        if rlen > 0.001 {
            rx = rx / rlen;
            rz = rz / rlen;
        }

        // up = right × forward
        let ux = ry * fz - rz * fy;
        let uy = rz * fx - rx * fz;
        let uz = rx * fy - ry * fx;

        let i = 0;
        while i < pellet_count {
            let h_spread = random_range(-spread, spread);
            let v_spread = random_range(-spread, spread);

            let dx = fx + rx * h_spread + ux * v_spread;
            let dy = fy + ry * h_spread + uy * v_spread;
            let dz = fz + rz * h_spread + uz * v_spread;

            // Normalize the perturbed direction
            let dlen = sqrt(dx*dx + dy*dy + dz*dz);
            if dlen > 0.001 {
                dx = dx / dlen;
                dy = dy / dlen;
                dz = dz / dlen;
            }

            fire_single_ray(cam_pos, dx, dy, dz, range, damage);
            i += 1;
        }
    }
}

fn fire_single_ray(cam_pos, dx, dy, dz, range, damage) {
    let hit = raycast(
        cam_pos.x, cam_pos.y, cam_pos.z,
        dx, dy, dz,
        range
    );

    if hit != () {
        let target_id = hit.entity;
        let target_health = get_health(target_id);
        if target_health > 0.0 {
            let killed = deal_damage(target_id, damage);
            if killed {
                log_info("Target eliminated!");
            }
        }
    }
}
