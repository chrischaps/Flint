// Rolling casks for pencil's local strokes (#544): two casks lying on
// their sides roll back and forth across the clearing on the same path,
// and the crate between them turns in place. The back cask's hatching is
// laid on the world's surfaces (grid = "world"), so it holds still while
// the cask rolls under it; the front cask's and the crate's are laid in
// their own space (grid = "local"), so the strokes ride on them.
//
// On game time, so `flint render --scripts --time 0.5..2.5 --fps 24` draws
// the same strokes every run.

fn on_update() {
    let t = total_time();
    // Metres from the middle of the path, and the turn that rolls that far
    let radius = 0.38;
    let travel = 1.3 * (0.55 * t).sin();
    let roll = -(travel / radius) * 180.0 / 3.14159265;
    for pair in [["cask_world", 2.4], ["cask_local", 4.4]] {
        let e = get_entity(pair[0]);
        set_position(e, 0.8 + travel, radius, pair[1]);
        // Lying along z (x = 90), rolled about the world's z
        set_rotation(e, 90.0, 0.0, roll);
    }
    set_rotation(get_entity("crate"), 0.0, 30.0 + 40.0 * t, 0.0);
}
