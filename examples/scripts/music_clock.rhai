// Music clock (#745): a scene kept in time by its music, with no chart.
// Four pads light on the beats of the bar and fade through each beat;
// eight tiles fill with the bars, amber in the dawn, teal in the rise. Every
// number comes from music_beat(), music_bar() and music_section(), which the
// music subsystem sets before each frame's scripts.
//
// Top-level lets reach the callbacks, not helper fns, so the helpers below
// take what they need as arguments.

let pads = [];
let tiles = [];

// The sections' colours. Functions, since a helper fn can't see a
// top-level const.
fn dawn() { [1.0, 0.62, 0.28] }
fn rise() { [0.32, 0.86, 0.9] }

fn on_init() {
    pads = [get_entity("beat_1"), get_entity("beat_2"), get_entity("beat_3"), get_entity("beat_4")];
    tiles = [];
    for i in 1..9 {
        tiles.push(get_entity(`bar_${i}`));
    }
}

fn tint(c, k) {
    [c[0] * k, c[1] * k, c[2] * k, 1.0]
}

// The pad of the beat lit, fading through the beat; the one brighter.
fn light_the_pads(pads, beat, colour) {
    let in_bar = beat % 4.0;
    let now = in_bar.floor();
    let glow = (-(in_bar - now) * 2.2).exp();
    for i in 0..4 {
        let on = if i == now.to_int() { glow } else { 0.0 };
        let peak = if i == 0 { 1.0 } else { 0.7 };
        set_field(pads[i], "material", "color", tint(colour, 0.1 + 0.9 * peak * on));
        let s = 1.0 + 0.16 * on;
        set_scale(pads[i], s, s, s);
    }
}

// Every bar so far lit in its section's colour, the bar now brightest.
fn fill_the_tiles(tiles, bar, beat) {
    for i in 0..8 {
        let colour = if i < 4 { dawn() } else { rise() };
        let k = if i < bar { 0.35 } else if i == bar { 0.35 + 0.65 * (beat % 4.0) / 4.0 } else { -1.0 };
        let c = if k < 0.0 { [0.03, 0.03, 0.045, 1.0] } else { tint(colour, k) };
        set_field(tiles[i], "material", "color", c);
    }
}

fn on_update() {
    let beat = music_beat();
    if beat < 0.0 {
        // The count-in before the first beat, or no music: everything waits
        light_the_pads(pads, 0.0, [0.0, 0.0, 0.0]);
        return;
    }
    let colour = if music_section() == "rise" { rise() } else { dawn() };
    light_the_pads(pads, beat, colour);
    fill_the_tiles(tiles, music_bar(), beat);
}

fn on_draw_ui() {
    let sw = screen_width();
    let ink = [0.92, 0.9, 0.86, 1.0];
    let dim = [0.6, 0.6, 0.66, 1.0];
    draw_text(sw * 0.5, 34.0, "MUSIC CLOCK", 20.0, #{ color: ink, align: "center", spacing: 4.0 });
    let beat = music_beat();
    let line = if music_bar() < 0 && beat == -1.0 {
        "no music in this scene"
    } else if beat < 0.0 {
        "counting in"
    } else {
        let section = music_section();
        `bar ${music_bar() + 1}   beat ${(beat % 4.0 + 1.0).floor().to_int()}   ${section}`
    };
    draw_text(sw * 0.5, screen_height() - 44.0, line, 16.0, #{ color: dim, align: "center", spacing: 2.0 });
}
