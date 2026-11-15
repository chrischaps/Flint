// The scripted-render goldens' director (#371): the barrel and YBot are
// authored in ASCII; this holds them smooth until a beat at 2 s of game
// time, then gives them their glyphs back. Game time only, so a render at
// --script-time T draws the same beat every run.

fn on_init() {
    clear_entity_style(get_entity("props_barrel"));
    clear_entity_style(get_entity("props_ybot"));
}

fn on_update() {
    if total_time() < 2.0 {
        return;
    }
    for name in ["props_barrel", "props_ybot"] {
        let id = get_entity(name);
        if get_entity_style(id) == "" {
            set_entity_style(id, "ascii");
        }
    }
}
