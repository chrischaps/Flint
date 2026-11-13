// Drawing props into the live frame (#879): each prop is given its pencil
// look once, at blend 0 with transition = "draw" (the scene's preset), so
// nothing shows; then only the blend moves, and under draw a blend is a
// front: the outline grows along its length up to it, the hatching follows
// a stroke at a time, the paper's wash comes last. Ten seconds, looping,
// on game time, so `flint render --scripts --time T` lands on the same
// strokes every run.
//
//   0 s  the crate is drawn on over 2 s
//   1 s  the barrel, over 2 s
//   2 s  the bench, over 2 s
//   6 s  all three rubbed out over 2 s (the same front, run backwards)
//
// The engine eases each blend (tween_style_param, #370); the script only
// says when. The draw order is the row's seed, without boil, so the same
// blend draws the same strokes on every run.

let period = 10.0;

fn on_init() {
    for name in ["crate", "barrel", "bench"] {
        let e = get_entity(name);
        set_entity_style(e, "pencil");
        set_style_param(e, "blend", 0.0);
    }
}

fn on_update() {
    let t = total_time() % period;
    let was = t - delta_time();
    // The first frame starts the first beat too
    let first = total_time() <= delta_time() + 0.0001;
    let crossed = |at| t >= at && (was < at || first);

    if crossed.call(0.0) {
        tween_style_param(get_entity("crate"), "blend", 1.0, 2.0);
    }
    if crossed.call(1.0) {
        tween_style_param(get_entity("barrel"), "blend", 1.0, 2.0);
    }
    if crossed.call(2.0) {
        tween_style_param(get_entity("bench"), "blend", 1.0, 2.0);
    }
    if crossed.call(6.0) {
        for name in ["crate", "barrel", "bench"] {
            tween_style_param(get_entity(name), "blend", 0.0, 2.0);
        }
    }
}
