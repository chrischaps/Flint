// Recolouring an imported model's glow (#624). Every figure is the same
// YBotWithIdle.glb, whose Alpha_Joints_MAT has no emission at all; the
// lanterns are Lantern.glb, whose glass glows white through its emissive
// texture. Nothing is re-imported: each stage writes
// material_overrides.<material> (set_material_emissive), which the
// renderer reads every frame, under a moon that still lights everything.
//
//   0 s     cyan        the joints and the glass both
//   2.5 s   magenta     each stage eases in over its last 0.8 s
//   5 s     amber
//   7.5 s   green, then cyan again: a ten-second loop
//
// The figures run a step behind one another, so a stage walks along the
// row. Their bodies are painted dark in the scene, with the array form of
// the same component; a light and a paint on different materials of one
// model, or on the same one, don't disturb each other.

fn palette(i) {
    let stages = [
        [0.05, 0.95, 1.0],   // cyan
        [1.0, 0.12, 0.75],   // magenta
        [1.0, 0.55, 0.08],   // amber
        [0.35, 1.0, 0.25],   // green
    ];
    stages[i % stages.len()]
}

fn smooth(x) {
    let t = if x < 0.0 { 0.0 } else if x > 1.0 { 1.0 } else { x };
    t * t * (3.0 - 2.0 * t)
}

// The colour at time t: stage n held, easing into stage n+1 over 0.8 s.
fn colour_at(t) {
    let stage_len = 2.5;
    let n = (t / stage_len).floor().to_int();
    let into = smooth((t - n * stage_len - (stage_len - 0.8)) / 0.8);
    let a = palette(n);
    let b = palette(n + 1);
    [
        a[0] + (b[0] - a[0]) * into,
        a[1] + (b[1] - a[1]) * into,
        a[2] + (b[2] - a[2]) * into,
    ]
}

fn on_update() {
    // A loop ahead, so the figures running behind never ask for stage -1.
    let t = total_time() + 10.0;
    for i in 0..5 {
        let figure = get_entity("figure_" + i);
        if figure < 0 {
            continue;
        }
        let c = colour_at(t - i * 0.35);
        set_material_emissive(figure, "Alpha_Joints_MAT.002", c[0], c[1], c[2], 6.0);
    }
    let c = colour_at(t - 0.7);
    for name in ["lantern_left", "lantern_right"] {
        let lantern = get_entity(name);
        if lantern >= 0 {
            set_material_emissive(lantern, "LanternPost_Mat", c[0], c[1], c[2], 3.0);
        }
    }
}
