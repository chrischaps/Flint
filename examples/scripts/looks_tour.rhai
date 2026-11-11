// A tour of looks (#356): every three seconds of game time the clearing
// changes its whole post-processing with set_look(), each look laid under
// the scene's own [post_process] keys. The last beat takes the look off
// (clear_look) to show the scene's keys alone, then the tour loops.
//
// set_look is applied after on_update, once per call, so it is called
// only on the frame a beat starts.

fn tour() {
    ["graphite", "ascii_noir", "matrix", "ink_wash", "watercolor", ""]
}

fn on_update() {
    let looks = tour();
    let beat = (total_time() / 3.0).floor().to_int() % looks.len();
    let shown = get_field(self_entity(), "tour", "beat");
    if beat != shown {
        let name = looks[beat];
        if name == "" {
            clear_look();
        } else {
            set_look(name);
        }
        set_field(self_entity(), "tour", "beat", beat);
    }
}

fn on_draw_ui() {
    let looks = tour();
    let beat = (total_time() / 3.0).floor().to_int() % looks.len();
    let name = looks[beat];
    let label = if name == "" { "no look: the scene's keys" } else { "look = \"" + name + "\"" };
    draw_rect(18.0, 18.0, 330.0, 40.0, 0.04, 0.035, 0.03, 0.72);
    draw_text(32.0, 28.0, label, 20.0, 0.96, 0.92, 0.84, 1.0);
}
