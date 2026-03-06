// ─── game_controller.rhai ────────────────────────────────
// Pause menu + zone unlock progression for The Overgrown Ruins.
// Tracks essence count and unlocks zone doors at thresholds.

let paused = false;

fn on_init() {
    register_state("paused", #{
        physics: "pause",
        scripts: "pause",
        animation: "pause",
        particles: "pause",
        audio: "run",
        rendering: "run",
        transparent: true
    });

    // Crypt is always unlocked
    persist_set("zone_crypt_unlocked", true);

    // Initialize essence counter if not set
    let essence = persist_get("essence_collected");
    if type_of(essence) == "()" {
        persist_set("essence_collected", 0);
    }

    log("Game controller active — The Overgrown Ruins");
}

fn on_update() {
    let essence = persist_get("essence_collected");
    if type_of(essence) == "()" {
        essence = 0;
    }

    // Unlock garden at 3 essence
    if essence >= 3 {
        persist_set("zone_garden_unlocked", true);
    }
    // Unlock library at 7 essence
    if essence >= 7 {
        persist_set("zone_library_unlocked", true);
    }
}

fn on_action(action) {
    if action == "pause" {
        if paused {
            pop_state();
            paused = false;
        } else {
            push_state("paused");
            paused = true;
        }
    }
}

fn on_draw_ui() {
    if paused {
        let sw = screen_width();
        let sh = screen_height();

        // Frosted dark overlay
        set_radial_blur(0.3);
        set_audio_lowpass(800.0);

        // Dim background
        draw_rect(0.0, 0.0, sw, sh, 0.0, 0.0, 0.0, 0.55);

        // Title
        let title = "PAUSED";
        let tm = measure_text(title, 48.0);
        let tx = (sw - tm.width) / 2.0;
        let ty = sh * 0.38;
        draw_text_ex(tx + 2.0, ty + 2.0, title, 48.0, 0.0, 0.0, 0.0, 0.6, 5);
        draw_text_ex(tx, ty, title, 48.0, 0.95, 0.85, 0.5, 1.0, 5);

        // Separator line
        let line_w = 180.0;
        let line_y = ty + 50.0;
        draw_rect_ex((sw - line_w) / 2.0, line_y, line_w, 2.0, 0.7, 0.55, 0.25, 0.5, 0.0, 5);

        // Subtitle
        let sub = "Press [ESC] to resume";
        let sm = measure_text(sub, 18.0);
        let sx = (sw - sm.width) / 2.0;
        draw_text_ex(sx, line_y + 24.0, sub, 18.0, 0.8, 0.75, 0.65, 0.7, 5);

        // Decorative corners
        let m = 40.0;
        let cl = 30.0;
        let ca = 0.25;
        let cr = 0.4;
        let cg = 0.8;
        let cb = 0.9;
        draw_line(m, m, m + cl, m, cr, cg, cb, ca);
        draw_line(m, m, m, m + cl, cr, cg, cb, ca);
        draw_line(sw - m, m, sw - m - cl, m, cr, cg, cb, ca);
        draw_line(sw - m, m, sw - m, m + cl, cr, cg, cb, ca);
        draw_line(m, sh - m, m + cl, sh - m, cr, cg, cb, ca);
        draw_line(m, sh - m, m, sh - m - cl, cr, cg, cb, ca);
        draw_line(sw - m, sh - m, sw - m - cl, sh - m, cr, cg, cb, ca);
        draw_line(sw - m, sh - m, sw - m, sh - m - cl, cr, cg, cb, ca);
    } else {
        set_radial_blur(0.0);
        set_audio_lowpass(20000.0);
    }
}
