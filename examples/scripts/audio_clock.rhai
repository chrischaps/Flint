// Audio clock (#658): four pads lit by where the click track is, read with
// audio_position(), beside the clock a script can keep itself (the sum of
// delta_time()). Space stalls the frame for half a second; E pauses.
// The music clock keeps its place through both, and through the web's
// late start; the summed clock falls behind at each stall and never
// catches up.
//
// Top-level lets reach the callbacks, not helper fns, so the helpers below
// take what they need as arguments.

let track = -1;
let pads = [];
let last_pos = -1.0;
let laps = 0.0;
let music = -1.0;        // seconds of music heard so far, unwrapped
let summed = -1.0;       // the script's own clock, started with the music
let wall0 = ();          // the wall clock, started with the music
let music0 = 0.0;
let next_log = 5.0;      // wall seconds
let has_wall = true;     // Rhai's timestamp() isn't there on the web
let stalls = 0;
let paused = false;

fn on_init() {
    track = get_entity("click_track");
    pads = [get_entity("pad_1"), get_entity("pad_2"), get_entity("pad_3"), get_entity("pad_4")];
}

fn on_update() {
    if summed >= 0.0 {
        summed += delta_time();
    }
}

// Light the pad of the beat at `pos` (seconds into the two-second loop,
// four beats of half a second), fading through the beat.
fn light_the_pads(pads, pos) {
    let beat = (pos / 0.5).floor();
    let glow = (-(pos - beat * 0.5) * 9.0).exp();
    for i in 0..4 {
        let on = if i == beat.to_int() { glow } else { 0.0 };
        let base = if i == 0 { [1.0, 0.55, 0.25] } else { [0.35, 0.75, 1.0] };
        let k = 0.08 + 0.92 * on;
        set_field(pads[i], "material", "color", [base[0] * k, base[1] * k, base[2] * k, 1.0]);
        let s = 1.0 + 0.18 * on;
        set_scale(pads[i], s, s, s);
    }
}

fn ms(x) {
    `${(x * 1000.0).round().to_int()} ms`
}

fn secs(x) {
    `${(x * 100.0).round() / 100.0} s`
}

// Every frame, paused or not: on_draw_ui always runs.
fn on_draw_ui() {
    let sw = screen_width();
    let ink = [0.92, 0.9, 0.86, 1.0];
    let dim = [0.6, 0.6, 0.66, 1.0];
    draw_text(sw * 0.5, 40.0, "AUDIO CLOCK", 22.0, #{ color: ink, align: "center", spacing: 4.0 });
    draw_text(sw * 0.5, screen_height() - 36.0, "SPACE  stall the frame 500 ms        E  pause", 13.0, #{ color: dim, align: "center", spacing: 2.0 });

    if is_action_just_pressed("jump") && has_wall {
        // Hold the frame: the music plays on in its own thread.
        let t = timestamp();
        while t.elapsed < 0.5 {}
        stalls += 1;
    }
    if is_action_just_pressed("interact") {
        if paused { pop_state(); } else { push_state("paused"); }
        paused = !paused;
    }

    let pos = audio_position(track);
    if pos < 0.0 {
        draw_text(sw * 0.5, 80.0, "waiting for the music to start", 15.0, #{ color: dim, align: "center" });
        return;
    }
    if last_pos >= 0.0 && pos < last_pos - 1.0 {
        laps += 2.0;
    }
    last_pos = pos;
    music = laps + pos;
    if summed < 0.0 {
        summed = music;
        music0 = music;
        try {
            wall0 = timestamp();
        } catch {
            has_wall = false;
        }
        log(`[audio_clock] the music started: ${music} s in`);
    }

    // Light what is heard: what is mixed now reaches the ear a little later.
    let latency = audio_output_latency();
    let heard = if latency > 0.0 { pos - latency } else { pos };
    if heard < 0.0 {
        heard += 2.0;
    }
    light_the_pads(pads, heard);

    let wall = if has_wall { wall0.elapsed } else { -1.0 };
    let y = 80.0;
    draw_text(sw * 0.5, y, `music  ${secs(music)}`, 16.0, #{ color: ink, align: "center" });
    draw_text(sw * 0.5, y + 24.0, `summed delta_time  ${secs(summed)}   (${ms(summed - music)} from the music)`, 15.0, #{ color: dim, align: "center" });
    let lat = if latency < 0.0 { "unknown here" } else { ms(latency) };
    let state = if paused { "    PAUSED" } else { "" };
    draw_text(sw * 0.5, y + 48.0, `output latency  ${lat}    stalls  ${stalls}${state}`, 13.0, #{ color: dim, align: "center" });

    // A line every five seconds: how far the music is from the wall clock,
    // within the two-second loop (so it doesn't depend on seeing every
    // wrap: a throttled page may draw a frame a second), and how far the
    // summed clock is from the music.
    if has_wall && wall >= next_log {
        next_log += 5.0;
        let off = (pos - music0 % 2.0 - wall) % 2.0;
        if off > 1.0 { off -= 2.0; }
        if off < -1.0 { off += 2.0; }
        log(`[audio_clock] wall ${wall} pos ${pos} music-wall ${ms(off)} summed-music ${ms(summed - music)} stalls ${stalls}`);
    }
}
