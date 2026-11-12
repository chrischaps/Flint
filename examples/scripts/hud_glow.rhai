// HUD glow (#626). Three numerals, one per column:
//   before: a 12 pt translucent stroke as the engine drew one until #626,
//           the text again at eight compass offsets (drawn here by hand)
//   after:  the same `stroke`, now one round band
//   glow:   a thin stroke and a `glow` of the text's own hue
// and a neon title along the bottom.

fn column(cx, label) {
    draw_text(cx, 92.0, label, 15.0, #{ color: [0.62, 0.6, 0.75, 1.0], align: "center", spacing: 2.0 });
}

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();
    let size = 112.0;
    let y = 150.0;
    let face = [1.0, 0.97, 0.9, 1.0];
    let band = [0.25, 0.85, 1.0, 0.3];
    let w = 12.0;

    // Before: eight copies, exactly the old code's offsets and order.
    let cx = sw * 0.2;
    column(cx, "EIGHT COPIES (BEFORE)");
    for d in [[-w, 0.0], [w, 0.0], [0.0, -w], [0.0, w], [-w, -w], [w, -w], [-w, w], [w, w]] {
        draw_text(cx + d[0], y + d[1], "58", size, #{ color: band, align: "center" });
    }
    draw_text(cx, y, "58", size, #{ color: face, align: "center" });

    // After: the same stroke.
    let cx = sw * 0.5;
    column(cx, "STROKE 12 PT (AFTER)");
    draw_text(cx, y, "58", size, #{ color: face, align: "center", stroke: [0.25, 0.85, 1.0, 0.3, w] });

    // Glow: neon, a bright face with a soft halo of its hue.
    let cx = sw * 0.8;
    column(cx, "GLOW 18 PT");
    draw_text(cx, y, "58", size, #{
        color: [1.0, 0.93, 0.97, 1.0],
        align: "center",
        stroke: [1.0, 0.35, 0.7, 0.85, 1.5],
        glow: [1.0, 0.2, 0.6, 0.9, 18.0],
    });

    draw_text(sw * 0.5, sh - 190.0, "AFTERGLOW", 64.0, #{
        color: [0.85, 1.0, 0.98, 1.0],
        align: "center",
        spacing: 10.0,
        stroke: [0.3, 1.0, 0.85, 0.9, 1.0],
        glow: [0.1, 0.9, 0.8, 0.85, 22.0],
    });
    // World text takes the same keys, depth-tested in the scene.
    draw_world_text(0.0, 1.05, 0.0, "world text glows too", 26.0, #{
        color: [1.0, 0.95, 0.8, 1.0],
        stroke: [1.0, 0.6, 0.2, 0.35, 6.0],
        glow: [1.0, 0.5, 0.1, 0.8, 14.0],
    });

    draw_text(sw * 0.5, sh - 96.0,"draw_text(..., #{ glow: [r, g, b, a, radius] })", 16.0, #{
        color: [0.55, 0.55, 0.68, 1.0],
        align: "center",
    });
}
