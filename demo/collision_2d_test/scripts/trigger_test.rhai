// Trigger zone script — logs enter/exit events

fn on_trigger_enter(entity_id) {
    let name = entity_name(entity_id);
    log("Trigger zone ENTERED by: " + name);
}

fn on_trigger_exit(entity_id) {
    let name = entity_name(entity_id);
    log("Trigger zone EXITED by: " + name);
}
