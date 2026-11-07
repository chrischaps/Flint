// Beat 02, the world in type (#265, #276, #277). One slow orbit; every
// 2.4 s the scene's ASCII table changes how it colours its glyphs:
//
//   0.0  source   each glyph the colour of what it covers
//   2.4  tint     phosphor green, glowing
//   4.8  palette  CGA, on black
//   7.2  palette  amber, block glyphs
//   9.6  mono     ink alone, hard contrast
//  12.0  source   again, the cells settling finer, 8 -> 5 points
//
// set_scene_style_param edits [post_process.styles.ascii] as the scene
// file would; a step sets every key it cares about, so each look is whole.

import "film" as film;

fn step_at(t) {
    if t < 2.4 { 0 } else if t < 4.8 { 1 } else if t < 7.2 { 2 } else if t < 9.6 { 3 }
    else if t < 12.0 { 4 } else { 5 }
}

fn apply(step) {
    let a = "ascii";
    // The defaults every step starts from
    set_scene_style_param(a, "ramp", "classic");
    set_scene_style_param(a, "ground", "dim");
    set_scene_style_param(a, "glow", 0.0);
    set_scene_style_param(a, "background", 0.15);
    set_scene_style_param(a, "brightness", 0.2);
    set_scene_style_param(a, "contrast", 1.2);
    if step == 1 {
        set_scene_style_param(a, "color", "tint");
        set_scene_style_param(a, "tint", [0.35, 1.0, 0.45]);
        set_scene_style_param(a, "glow", 0.6);
        set_scene_style_param(a, "background", 0.05);
    } else if step == 2 {
        set_scene_style_param(a, "color", "palette");
        set_scene_style_param(a, "palette", "cga");
        set_scene_style_param(a, "ground", "black");
        set_scene_style_param(a, "brightness", 0.35);
    } else if step == 3 {
        set_scene_style_param(a, "color", "palette");
        set_scene_style_param(a, "palette", "amber");
        set_scene_style_param(a, "ramp", "blocks");
        set_scene_style_param(a, "glow", 0.4);
    } else if step == 4 {
        set_scene_style_param(a, "color", "mono");
        set_scene_style_param(a, "contrast", 1.6);
        set_scene_style_param(a, "brightness", 0.0);
    } else {
        set_scene_style_param(a, "color", "source");
        set_scene_style_param(a, "ramp", "detailed");
    }
}

fn on_update() {
    let t = total_time();
    film::orbit(0.0, 0.75, 0.0, 5.2, 1.6, film::lerp(0.85, 1.6, t / 14.0));

    let step = step_at(t);
    if step != get_field(self_entity(), "film", "step") {
        apply(step);
        set_field(self_entity(), "film", "step", step);
    }
    // The last step's cells settle finer: a uniform write a frame
    set_scene_style_param("ascii", "cell", film::lerp(8.0, 5.0, film::ramp(t, 12.0, 1.8)));
}
