// ================================================================
//  HUD Script — Doom-style FPS combat overlay
// ================================================================
//
//  Draws crosshair, health panel, ammo/weapon panel, damage flash,
//  and interaction prompts using the engine's 2D draw API.
//
//  NOTE: Rhai's call_fn() only gives Scope access to the directly
//  called function. Sub-functions CANNOT see module-level variables.
//  All draw logic must live directly in on_draw_ui(), or state must
//  be passed as parameters to helpers.
// ================================================================

let prompt_alpha = 0.0;
let prev_health = 100.0;
let damage_flash = 0.0;
let secret_flash = 0.0;
let secret_was_found = false;

// ─── Pure helpers (no scope vars needed) ─────────────

fn find_player() {
    let id = get_entity("player");
    if id < 0 { return -1; }
    if !has_component(id, "character_controller") { return -1; }
    return id;
}

fn to_float(v) {
    if type_of(v) == "f64" { return v; }
    if type_of(v) == "i64" { return v + 0.0; }
    return 0.0;
}

fn to_int_val(v) {
    if type_of(v) == "i64" { return v; }
    if type_of(v) == "f64" { return v.floor(); }
    return 0;
}

// ─── on_update: track health changes for damage flash ──

fn on_update() {
    let dt = delta_time();
    let player = find_player();
    if player < 0 { return; }

    let health_comp = get_component(player, "health");
    if type_of(health_comp) == "()" { return; }

    let current = to_float(health_comp.current);

    if current < prev_health {
        damage_flash = 1.0;
    }
    prev_health = current;

    if damage_flash > 0.0 {
        damage_flash = max(damage_flash - dt * 3.0, 0.0);
    }

    // Poll secret wall discovery state
    let secret_id = get_entity("sr_secret_wall");
    if secret_id >= 0 && has_component(secret_id, "secret") {
        let secret_comp = get_component(secret_id, "secret");
        if type_of(secret_comp) != "()" {
            let disc = secret_comp.discovered;
            if type_of(disc) == "bool" && disc && !secret_was_found {
                secret_flash = 3.0;
                secret_was_found = true;
            }
        }
    }
    if secret_flash > 0.0 {
        secret_flash = max(secret_flash - dt, 0.0);
    }

    let nearest = find_nearest_interactable();
    if type_of(nearest) != "()" {
        // Suppress interaction prompt for secret walls — don't give away the secret
        let itype = nearest.interaction_type;
        if itype == "secret" {
            prompt_alpha = max(prompt_alpha - 5.0 * dt, 0.0);
        } else {
            prompt_alpha = min(prompt_alpha + 5.0 * dt, 1.0);
        }
    } else {
        prompt_alpha = max(prompt_alpha - 5.0 * dt, 0.0);
    }
}

// ─── on_draw_ui: all drawing inline ────────────────────

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();

    // ── Crosshair ──
    let cx = sw / 2.0;
    let cy = sh / 2.0;
    draw_circle(cx, cy, 2.0, 1.0, 1.0, 1.0, 0.55);
    draw_circle_outline(cx, cy, 3.5, 0.0, 0.0, 0.0, 0.31, 0.8);

    // ── Damage flash (full-screen red tint) ──
    if damage_flash > 0.01 {
        let flash_a = damage_flash * 0.31;
        draw_rect_ex(0.0, 0.0, sw, sh, 0.78, 0.08, 0.04, flash_a, 0.0, -1);
    }

    // ── Secret discovered notification (top-center, golden fade-out) ──
    if secret_flash > 0.01 {
        let sa = min(secret_flash, 1.0);
        let msg = "A secret is revealed!";
        let msg_size = 22.0;
        let msg_m = measure_text(msg, msg_size);
        let msg_w = msg_m.width;
        let pill_w = msg_w + 40.0;
        let pill_h = msg_size + 20.0;
        let pill_x = (sw - pill_w) / 2.0;
        let pill_y = 40.0;
        draw_rect_ex(pill_x, pill_y, pill_w, pill_h, 0.04, 0.04, 0.04, 0.71 * sa, 12.0, 2);
        draw_text_ex(pill_x + 20.0, pill_y + 10.0, msg, msg_size, 0.94, 0.86, 0.31, sa, 2);
    }

    // ── Health panel (bottom-left) ──
    let player = find_player();
    if player >= 0 {
        let health_comp = get_component(player, "health");
        if type_of(health_comp) != "()" {
            let current = to_float(health_comp.current);
            let max_hp = to_float(health_comp.max);
            if max_hp <= 0.0 { max_hp = 100.0; }
            let pct = current / max_hp;

            let hr = 0.0; let hg = 0.0; let hb = 0.0;
            if pct > 0.6 {
                hr = 0.31; hg = 0.86; hb = 0.31;
            } else if pct > 0.3 {
                hr = 0.94; hg = 0.78; hb = 0.16;
            } else {
                hr = 0.86; hg = 0.16; hb = 0.16;
            }

            let px = 20.0;
            let py = sh - 60.0;

            draw_rect_ex(px - 2.0, py - 4.0, 120.0, 44.0, 0.04, 0.04, 0.04, 0.63, 6.0, -1);

            let hp_text = "" + to_int_val(current);
            draw_text_ex(px + 12.0, py + 4.0, hp_text, 28.0, hr, hg, hb, 1.0, 1);
            draw_text_ex(px + 82.0, py + 14.0, "HP", 14.0, 0.7, 0.7, 0.7, 0.78, 1);
        }

        // ── Armor panel (bottom-left, right of health) ──
        if has_component(player, "armor") {
            let armor_comp = get_component(player, "armor");
            let armor_val = to_float(armor_comp.current);
            let armor_max = to_float(armor_comp.max);
            if armor_max <= 0.0 { armor_max = 100.0; }
            let armor_pct = armor_val / armor_max;

            let ar_r = 0.0; let ar_g = 0.0; let ar_b = 0.0;
            if armor_val <= 0.0 {
                ar_r = 0.35; ar_g = 0.35; ar_b = 0.35;
            } else if armor_pct > 0.5 {
                ar_r = 0.24; ar_g = 0.78; ar_b = 0.39;
            } else if armor_pct > 0.25 {
                ar_r = 0.94; ar_g = 0.78; ar_b = 0.16;
            } else {
                ar_r = 0.86; ar_g = 0.16; ar_b = 0.16;
            }

            let apx = 148.0;
            let apy = sh - 60.0;

            draw_rect_ex(apx - 2.0, apy - 4.0, 120.0, 44.0, 0.04, 0.04, 0.04, 0.63, 6.0, -1);

            let armor_text = "" + to_int_val(armor_val);
            draw_text_ex(apx + 12.0, apy + 4.0, armor_text, 28.0, ar_r, ar_g, ar_b, 1.0, 1);
            draw_text_ex(apx + 82.0, apy + 14.0, "AR", 14.0, 0.7, 0.7, 0.7, 0.78, 1);
        }

        // ── Ammo + weapon panel (bottom-right) ──
        let weapon = get_component(player, "weapon");
        if type_of(weapon) != "()" {
            let weapon_name = weapon.name;
            if type_of(weapon_name) != "()" && weapon_name != "" {
                let ammo = to_int_val(weapon.ammo);
                let max_ammo = to_int_val(weapon.max_ammo);

                // Determine ammo type label from inventory
                let ammo_label = "BULLETS";
                if has_component(player, "weapon_inventory") {
                    let inv = get_component(player, "weapon_inventory");
                    let cur_weapon = to_int_val(inv.current_weapon);
                    if cur_weapon == 2 {
                        ammo_label = "SHELLS";
                    }
                }

                let ar = 0.78; let ag = 0.78; let ab = 0.86;
                if ammo <= 10 && ammo > 0 {
                    ar = 0.94; ag = 0.71; ab = 0.16;
                } else if ammo <= 0 {
                    ar = 0.86; ag = 0.16; ab = 0.16;
                }

                let apx = sw - 180.0;
                let apy = sh - 68.0;

                draw_rect_ex(apx - 2.0, apy - 4.0, 162.0, 52.0, 0.04, 0.04, 0.04, 0.63, 6.0, -1);

                let ammo_text = "" + ammo;
                draw_text_ex(apx + 12.0, apy + 4.0, ammo_text, 28.0, ar, ag, ab, 1.0, 1);
                draw_text_ex(apx + 80.0, apy + 6.0, weapon_name, 12.0, 0.7, 0.7, 0.7, 0.78, 1);

                let max_text = "/ " + max_ammo;
                draw_text_ex(apx + 80.0, apy + 20.0, max_text, 11.0, 0.47, 0.47, 0.47, 0.71, 1);

                draw_text_ex(apx + 80.0, apy + 34.0, ammo_label, 9.0, 0.55, 0.55, 0.55, 0.63, 1);
            }
        }
    }

    // ── Interaction prompt (bottom-center, fades) ──
    if prompt_alpha > 0.01 {
        let nearest = find_nearest_interactable();

        let prompt = "[E] Interact";
        if type_of(nearest) != "()" {
            let verb = "Use";
            let itype = nearest.interaction_type;
            if itype == "talk" { verb = "Talk to"; }
            else if itype == "examine" { verb = "Examine"; }
            else if itype == "door" {
                prompt = "[E] Open";
            }
            if itype != "door" {
                prompt = "[E] " + verb + " " + nearest.prompt_text;
            }
        }

        let text_size = 16.0;
        let measurement = measure_text(prompt, text_size);
        let tw = measurement.width;
        let pad_x = 20.0;
        let pad_y = 10.0;

        let box_w = tw + pad_x * 2.0;
        let box_h = text_size + pad_y * 2.0;
        let box_x = (sw - box_w) / 2.0;
        let box_y = sh - 80.0;

        draw_rect_ex(box_x, box_y, box_w, box_h, 0.04, 0.04, 0.04, 0.71 * prompt_alpha, 12.0, -1);

        let tx = box_x + pad_x;
        let ty = box_y + pad_y;
        draw_text_ex(tx, ty, prompt, text_size, 0.94, 0.92, 0.86, prompt_alpha, 1);
    }
}
