// Cross-fading looks (#496): the clearing holds each look a moment, then
// eases into the next with set_look(name, seconds). Numbers lerp, a style
// only one look has fades in or out by its whole-frame blend, and options
// switch halfway. The engine fires `look_faded` when a fade arrives, which
// starts the next hold, so the beats follow the fades however long they are.
//
// All on game time: `flint-player --frame-step 0.0333 --screenshot-every`
// captures the same frames every run.

let looks = ["graphite", "watercolor", "ink_wash", "", "coloured_pencil"];
let shown = 0;          // the scene starts in looks[0], named in its [post_process]
let fading = false;
let held_since = 0.0;   // when the look on screen arrived
let fade_started = 0.0;
let from = looks[0];

fn hold() { 1.5 }
fn fade() { 2.0 }

fn label(name) {
    if name == "" { "no look" } else { name }
}

fn on_update() {
    if fading || total_time() - held_since < hold() {
        return;
    }
    from = looks[shown];
    shown = (shown + 1) % looks.len();
    fading = true;
    fade_started = total_time();
    let next = looks[shown];
    if next == "" {
        clear_look(fade());
    } else {
        set_look(next, fade());
    }
}

fn on_event(name, data) {
    if name == "look_faded" {
        fading = false;
        held_since = total_time();
    }
}

fn on_draw_ui() {
    let x = 18.0;
    let y = 18.0;
    let w = 300.0;
    draw_rect(x, y, w, 58.0, 0.04, 0.035, 0.03, 0.72);
    if fading {
        let t = ((total_time() - fade_started) / fade()).min(1.0);
        draw_text(x + 14.0, y + 10.0, label(from) + "  to  " + label(looks[shown]), 19.0, 0.96, 0.92, 0.84, 1.0);
        // How far along, in time; the picture itself eases (smoothstep)
        draw_rect(x + 14.0, y + 40.0, w - 28.0, 3.0, 0.96, 0.92, 0.84, 0.18);
        draw_rect(x + 14.0, y + 40.0, (w - 28.0) * t, 3.0, 0.96, 0.92, 0.84, 0.9);
    } else {
        draw_text(x + 14.0, y + 10.0, label(looks[shown]), 19.0, 0.96, 0.92, 0.84, 1.0);
        draw_text(x + 14.0, y + 34.0, "set_look(name, " + fade() + ")", 13.0, 0.96, 0.92, 0.84, 0.55);
    }
}
