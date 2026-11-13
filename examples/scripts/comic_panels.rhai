// Comic panels (#880): the director cuts between page layouts on game
// time, ten seconds, looping, so a --frame-step capture (or `flint render
// --scripts --time T`) lands on the same page every run.
//
//   0 s  one panel, the wide shot alone
//   2 s  two: the wide shot and the pencil close-up of the dancer
//   5 s  three: an overhead inset in the corner of the wide shot
//   8 s  one again
//
// Each cut is one call, made on the frame its beat starts; the beat is
// kept on the director's `story` component, since a script's top level
// runs again before every callback. The caption is the HUD's, drawn over
// the page in on_draw_ui.

let period = 10.0;

fn wide_alone() {
    #{ rect: [0.0, 0.0, 1.0, 1.0] }
}

fn wide_beside() {
    #{ rect: [0.0, 0.0, 0.6, 1.0] }
}

// The rotoscoped close-up, framed from the dancer's origin
fn close_up() {
    #{
        rect: [0.6, 0.0, 0.4, 1.0],
        follow: "dancer",
        position: [1.5, 1.05, 2.9],
        target: [0.0, 0.92, 0.0],
        fov: 38.0,
        look: "rotoscope",
    }
}

// Straight down on the clearing, in ink on paper, drawn last so it sits
// over the wide shot's corner
fn overhead() {
    #{
        rect: [0.03, 0.56, 0.26, 0.41],
        order: 1,
        follow: "dancer",
        position: [0.0, 9.0, 0.6],
        target: [0.0, 0.0, 0.0],
        fov: 40.0,
        look: "graphite",
    }
}

fn on_update() {
    let t = total_time() % period;
    let beat = if t < 2.0 { 0 } else if t < 5.0 { 1 } else if t < 8.0 { 2 } else { 3 };
    let me = self_entity();
    if beat == get_field(me, "story", "beat") {
        return;
    }
    set_field(me, "story", "beat", beat);
    if beat == 0 || beat == 3 {
        // One panel: the page with a single shot, still inside its border
        set_panels(#{ gutter: 12, panel: #{ wide: wide_alone() } });
    } else if beat == 1 {
        set_panels(#{ gutter: 12, panel: #{ wide: wide_beside(), close: close_up() } });
    } else {
        // One panel more, the others as they are
        set_panel("overhead", overhead());
    }
}

fn on_draw_ui() {
    // A caption box in the wide shot's top left corner, over the page
    let x = 26.0;
    let y = 26.0;
    let text = "Meanwhile, in the clearing...";
    let size = 17.0;
    let w = measure_text(text, size).width + 22.0;
    draw_rect(x, y, w, 32.0, #{ color: "#fbf3d2", layer: 1 });
    draw_rect_outline(x, y, w, 32.0, #{ color: "#16120e", thickness: 2.0, layer: 2 });
    draw_text(x + 11.0, y + 7.0, text, size, #{ color: "#16120e", layer: 3 });
}
