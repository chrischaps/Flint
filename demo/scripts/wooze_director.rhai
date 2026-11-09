// ─── wooze_director.rhai ─────────────────────────────────
// Directs demo/wooze_field.scene.toml: a script moving how woozy the world is.
//
// Nothing here touches a tree. The field is the one thing that changes, and every
// prop reads it: the new step bends each clock from wherever it stands, and each
// part takes the rest on its own next beat, so no one frame carries the change.

fn crossed(at) {
    let now = total_time();
    now >= at && now - delta_time() < at
}

fn on_update() {
    // The whole world, at once, twice as fast.
    if crossed(12.0) {
        set_wooze(#{ step: 0.25 });
        log("wooze: the global step is now 0.25");
    }
    // One zone, over a few seconds: the pocket wakes up.
    if crossed(20.0) {
        set_wooze_zone("pocket", #{ glide: 0.15 }, 4.0);
        log("wooze: the pocket's glide heads for 0.15 over 4 s");
    }
}
