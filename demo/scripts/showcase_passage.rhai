// Hidden Passage — becomes interactable when stranger reveals it, triggers scene transition
// Demonstrates: persist_has(), persist_get/set(), load_scene()

let revealed = false;

fn on_update() {
    if revealed { return; }

    // Check if stranger has revealed the passage
    if persist_has("passage_revealed") {
        let me = self_entity();
        set_field(me, "interactable", "enabled", true);
        revealed = true;
    }
}

fn on_interact() {
    let me = self_entity();

    // Award sparks for finding the passage
    let sparks = persist_get("sparks_collected");
    if type_of(sparks) == "()" {
        sparks = 0;
    }
    sparks += 2;
    persist_set("sparks_collected", sparks);

    log("[The wall grinds open, revealing a passage descending into darkness...]");
    play_sound("audio/door_open.ogg");

    load_scene("demo/showcase_crucible.scene.toml");
}
