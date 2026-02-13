// ================================================================
//  Level Controller — ticks elapsed time, holds completion state
// ================================================================
//
//  All kill/secret counting is event-driven: enemy_ai and
//  secret_wall scripts increment fields on this entity when they
//  fire. This script only needs to tick the clock.
// ================================================================

fn to_float(v) {
    if type_of(v) == "f64" { return v; }
    if type_of(v) == "i64" { return v + 0.0; }
    return 0.0;
}

fn on_update(dt) {
    let me = this_entity();
    let ls = get_component(me, "level_state");
    if ls == () { return; }

    // Stop ticking once level is complete
    let done = ls.completed;
    if type_of(done) == "bool" && done { return; }

    // Tick elapsed time
    let t = to_float(ls.elapsed_time);
    t += dt;
    set_field(me, "level_state", "elapsed_time", t);
}
