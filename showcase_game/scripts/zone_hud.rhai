// ─── zone_hud.rhai ──────────────────────────────────────
// Crosshair, essence counter, zone title, interaction prompts,
// and scene transition visuals for The Overgrown Ruins.

let prompt_alpha = 0.0;
let title_alpha = 1.0;
let title_timer = 3.5;
let scene_title = "";
let transition_timer = 0.0;
let entered = false;

fn get_zone_title(scene) {
    if scene.contains("tavern_hub") {
        return "THE OVERGROWN RUINS";
    } else if scene.contains("sunken_crypt") {
        return "THE SUNKEN CRYPT";
    } else if scene.contains("flooded_garden") {
        return "THE FLOODED GARDEN";
    } else if scene.contains("collapsed_library") {
        return "THE COLLAPSED LIBRARY";
    }
    return "THE OVERGROWN RUINS";
}

fn on_init() {
    let scene = current_scene();
    scene_title = get_zone_title(scene);
}

fn on_scene_enter() {
    let scene = current_scene();
    scene_title = get_zone_title(scene);
    title_alpha = 1.0;
    title_timer = 3.5;
    transition_timer = 0.0;
    entered = false;
}

fn on_update() {
    let dt = delta_time();

    // Scene title fade
    if title_timer > 0.0 {
        title_timer -= dt;
        if title_timer < 1.0 {
            title_alpha = max(title_timer, 0.0);
        }
    }

    // Interaction prompt fade
    let nearest = find_nearest_interactable();
    if type_of(nearest) != "()" {
        prompt_alpha = min(prompt_alpha + 5.0 * dt, 1.0);
    } else {
        prompt_alpha = max(prompt_alpha - 5.0 * dt, 0.0);
    }

    // Handle transition phases
    let phase = transition_phase();
    if phase == "exiting" {
        transition_timer += dt;
        let progress = clamp(transition_timer / 1.5, 0.0, 1.0);
        set_vignette(0.35 + progress * 1.5);
        set_exposure(1.1 - progress * 0.8);
        set_audio_lowpass(20000.0 - progress * 18000.0);
        if transition_timer >= 1.5 {
            complete_transition();
        }
    } else if phase == "entering" {
        if !entered {
            transition_timer = 0.0;
            entered = true;
            let scene = current_scene();
            scene_title = get_zone_title(scene);
            title_alpha = 1.0;
            title_timer = 3.5;
        }
        transition_timer += dt;
        let progress = clamp(transition_timer / 2.0, 0.0, 1.0);
        set_vignette(1.85 - progress * 1.5);
        set_exposure(0.3 + progress * 0.8);
        set_audio_lowpass(2000.0 + progress * 18000.0);
        if transition_timer >= 2.0 {
            complete_transition();
            set_vignette(0.35);
            set_exposure(1.1);
            set_audio_lowpass(20000.0);
        }
    }
}

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();
    let cx = sw / 2.0;
    let cy = sh / 2.0;

    // --- Transition overlay ---
    let phase = transition_phase();
    if phase == "exiting" {
        let a = clamp(transition_timer / 1.5, 0.0, 1.0);
        draw_rect(0.0, 0.0, sw, sh, 0.02, 0.02, 0.02, a);
    } else if phase == "entering" {
        let a = 1.0 - clamp(transition_timer / 2.0, 0.0, 1.0);
        draw_rect(0.0, 0.0, sw, sh, 0.02, 0.02, 0.02, a);
    }

    // --- Crosshair ---
    draw_circle(cx, cy, 2.0, 1.0, 1.0, 1.0, 0.5);
    draw_circle_outline(cx, cy, 4.0, 0.0, 0.0, 0.0, 0.25, 0.8);

    // --- Essence counter ---
    let essence = persist_get("essence_collected");
    if type_of(essence) == "()" {
        essence = 0;
    }
    let essence_text = "ESSENCE: " + essence;
    let etm = measure_text(essence_text, 16.0);
    let ex = sw - etm.width - 30.0;
    let ey = 28.0;

    // Background pill
    draw_rect_ex(ex - 28.0, ey - 10.0, etm.width + 50.0, 28.0, 0.02, 0.04, 0.06, 0.6, 14.0, 1);
    // Essence icon (cyan glow)
    draw_circle(ex - 10.0, ey + 4.0, 6.0, 0.3, 0.8, 1.0, 0.9);
    draw_circle(ex - 10.0, ey + 2.0, 3.0, 0.6, 0.95, 1.0, 0.7);
    // Text
    draw_text_ex(ex + 4.0, ey, essence_text, 16.0, 0.4, 0.9, 1.0, 0.9, 2);

    // --- Interaction prompt ---
    if prompt_alpha > 0.01 {
        let nearest = find_nearest_interactable();
        if type_of(nearest) != "()" {
            let verb = "Use";
            if nearest.interaction_type == "talk" {
                verb = "Talk to";
            } else if nearest.interaction_type == "examine" {
                verb = "Examine";
            }
            let prompt = "[E] " + verb + " " + nearest.prompt_text;
            let pm = measure_text(prompt, 16.0);
            let px = (sw - pm.width) / 2.0;
            let py = sh * 0.62;

            draw_rect_ex(px - 14.0, py - 8.0, pm.width + 28.0, 30.0,
                         0.04, 0.04, 0.04, 0.7 * prompt_alpha, 12.0, -1);
            draw_text_ex(px, py, prompt, 16.0, 0.94, 0.92, 0.86, prompt_alpha, 1);
        }
    }

    // --- Scene title ---
    if title_alpha > 0.01 && scene_title != "" {
        let tm = measure_text(scene_title, 36.0);
        let tx = (sw - tm.width) / 2.0;
        let ty = sh * 0.25;
        draw_text_ex(tx + 2.0, ty + 2.0, scene_title, 36.0, 0.0, 0.0, 0.0, title_alpha * 0.5, 4);
        draw_text_ex(tx, ty, scene_title, 36.0, 0.4, 0.85, 0.95, title_alpha, 4);

        let sub = "A Procgen Showcase";
        let sm = measure_text(sub, 14.0);
        let subx = (sw - sm.width) / 2.0;
        draw_text_ex(subx, ty + 40.0, sub, 14.0, 0.5, 0.7, 0.75, title_alpha * 0.7, 4);
    }
}
