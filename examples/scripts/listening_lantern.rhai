// A lantern that answers (#583): each expression state eases the flame toward
// a colour and a brightness. The words are this script's, not Flint's: an
// unknown state keeps the light where it is.

let flame = -1;
let glass = -1;
let mode = "";          // the state the flame is easing toward
let base = 0.4;         // its brightness there, before the thinking breath
let brighter = false;   // which way the next breath goes

fn on_init() {
    flame = get_entity("flame");
    glass = get_entity("glass");
}

fn on_expression(state) {
    if !("state" in state) { return; }
    let energy = if "energy" in state { state.energy } else { 0.5 };
    let colour = [1.0, 0.82, 0.62];
    let glow = [2.2, 1.3, 0.6];
    let bright = 10.0;
    if state.state == "listening" {
        colour = [0.55, 0.7, 1.0];
        glow = [0.5, 0.8, 1.5];
        bright = 5.0;
    } else if state.state == "thinking" {
        colour = [0.78, 0.6, 1.0];
        glow = [1.4, 0.9, 2.4];
        bright = 8.0;
    } else if state.state == "speaking" {
        colour = [1.0, 0.68, 0.38];
        glow = [3.2, 1.5, 0.5];
        bright = 16.0;
    } else if state.state != "settled" {
        return;
    }
    mode = state.state;
    base = bright * (0.5 + energy);
    tween(flame, "light.color", colour, 0.6, "ease_in_out");
    tween(glass, "material.emissive", glow, 0.6, "ease_in_out");
    tween(flame, "light.intensity", base, 0.6, "ease_in_out");
}

// Thinking breathes: each arrival starts the next breath
fn on_event(name, data) {
    if name == "tween_done" && data.path == "light.intensity" && mode == "thinking" {
        brighter = !brighter;
        let to = if brighter { base * 1.35 } else { base * 0.7 };
        tween(flame, "light.intensity", to, 0.9, "ease_in_out");
    }
}

fn on_draw_ui() {
    if mode == "" { return; }
    draw_text(28.0, 24.0, mode, 22.0, #{ color: [0.92, 0.9, 0.86, 0.85] });
}
