// Showcase HUD — crosshair, spark counter, interaction prompts, transition visuals
// Demonstrates: persist_get(), transition_progress(), transition_phase(), complete_transition(),
//               set_vignette(), set_exposure(), on_draw_ui()

let prompt_alpha = 0.0;
let title_alpha = 1.0;
let title_timer = 3.5;
let scene_title = "";
let transition_timer = 0.0;
let entered = false;

fn on_init() {
    let scene = current_scene();
    if scene == "" {
        scene_title = "THE RUSTY FLINT";
    } else if scene == "demo/showcase_crucible.scene.toml" {
        scene_title = "THE CRUCIBLE";
    } else {
        scene_title = "THE RUSTY FLINT";
    }
}

fn on_scene_enter() {
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
        // Ramp vignette and drop exposure as we fade out
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
            // Re-read scene title for destination
            let scene = current_scene();
            if scene == "demo/showcase_crucible.scene.toml" {
                scene_title = "THE CRUCIBLE";
            } else {
                scene_title = "THE RUSTY FLINT";
            }
            title_alpha = 1.0;
            title_timer = 3.5;
        }
        transition_timer += dt;
        let progress = clamp(transition_timer / 2.0, 0.0, 1.0);
        // Fade from black to scene
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
    // Dot
    draw_circle(cx, cy, 2.0, 1.0, 1.0, 1.0, 0.5);
    // Outer ring shadow
    draw_circle_outline(cx, cy, 4.0, 0.0, 0.0, 0.0, 0.25, 0.8);

    // --- Spark counter ---
    let sparks = persist_get("sparks_collected");
    if type_of(sparks) == "()" {
        sparks = 0;
    }
    let spark_text = "SPARKS: " + sparks;
    let stm = measure_text(spark_text, 16.0);
    let sx = sw - stm.width - 30.0;
    let sy = 28.0;

    // Background pill
    draw_rect_ex(sx - 28.0, sy - 10.0, stm.width + 50.0, 28.0, 0.05, 0.04, 0.02, 0.6, 14.0, 1);
    // Spark icon (small orange circle)
    draw_circle(sx - 10.0, sy + 4.0, 6.0, 0.95, 0.6, 0.15, 0.9);
    draw_circle(sx - 10.0, sy + 2.0, 3.0, 1.0, 0.9, 0.5, 0.7);
    // Text
    draw_text_ex(sx + 4.0, sy, spark_text, 16.0, 0.95, 0.85, 0.5, 0.9, 2);

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

            // Background pill
            draw_rect_ex(px - 14.0, py - 8.0, pm.width + 28.0, 30.0,
                         0.04, 0.04, 0.04, 0.7 * prompt_alpha, 12.0, -1);
            // Prompt text
            draw_text_ex(px, py, prompt, 16.0, 0.94, 0.92, 0.86, prompt_alpha, 1);
        }
    }

    // --- Scene title ---
    if title_alpha > 0.01 && scene_title != "" {
        let tm = measure_text(scene_title, 36.0);
        let tx = (sw - tm.width) / 2.0;
        let ty = sh * 0.25;
        // Shadow
        draw_text_ex(tx + 2.0, ty + 2.0, scene_title, 36.0, 0.0, 0.0, 0.0, title_alpha * 0.5, 4);
        // Title
        draw_text_ex(tx, ty, scene_title, 36.0, 0.95, 0.85, 0.5, title_alpha, 4);

        // Subtitle line
        let sub = "A Flint Engine Showcase";
        let sm = measure_text(sub, 14.0);
        let subx = (sw - sm.width) / 2.0;
        draw_text_ex(subx, ty + 40.0, sub, 14.0, 0.7, 0.65, 0.55, title_alpha * 0.7, 4);
    }
}
