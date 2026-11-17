// Lamplighting (#734): tween(entity, "component.field", target, seconds[,
// easing]) eases any number or list of numbers a component holds, on game
// time, and fires `tween_done` with #{ entity, path } when it arrives. No
// on_update lerps: the engine moves every field, and the script only says
// where each is going.

let flame = -1;
let glass = -1;
let lantern = -1;
let brighter = false;   // which way the next breath goes

fn on_init() {
    flame = get_entity("flame");
    glass = get_entity("glass");
    lantern = get_entity("lantern");

    // Dusk to lamplight, over three seconds
    tween(flame, "light.intensity", 14.0, 3.0, "ease_in_out");
    tween(flame, "light.color", [1.0, 0.68, 0.38], 3.0);
    tween(glass, "material.emissive", [3.2, 1.5, 0.5], 3.0, "ease_in");
    tween(lantern, "transform.rotation", [0.0, 10.0, 0.0], 3.0, "ease_out");
}

// Each arrival starts the next breath, from wherever the flame stands
fn on_event(name, data) {
    if name == "tween_done" && data.path == "light.intensity" {
        brighter = !brighter;
        let to = if brighter { 17.0 } else { 11.0 };
        tween(flame, "light.intensity", to, 1.4);
    }
}
