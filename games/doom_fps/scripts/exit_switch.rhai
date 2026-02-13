// ================================================================
//  Exit Switch — triggers level completion on interaction
// ================================================================
//
//  On interact: sets level_state.completed = true on the
//  level_controller entity, plays a switch sound, changes the
//  switch material to green, and disables further interaction.
// ================================================================

fn on_interact() {
    let me = this_entity();

    // Guard against double-activation
    let already = get_field(me, "interactable", "enabled");
    if type_of(already) == "bool" && !already { return; }

    // Find level controller
    let lc = get_entity("level_controller");
    if lc < 0 {
        log_info("exit_switch: no level_controller entity found");
        return;
    }

    // Mark level complete
    set_field(lc, "level_state", "completed", true);

    // Play switch sound
    play_sound("audio/exit_switch.wav", 0.9);

    // Change material to green (activated look)
    set_field(me, "material", "color", [0.15, 0.7, 0.15, 1.0]);

    // Disable further interaction
    set_field(me, "interactable", "enabled", false);

    log_info("EXIT SWITCH ACTIVATED — level complete!");
}
