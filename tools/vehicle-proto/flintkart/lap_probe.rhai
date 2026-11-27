// The vehicle ADR's reference probe (Flint #1627): every frame, each kart's
// pose, speed and wall contact; every lap_complete. Read by lap_report.py.
fn on_update() {
    let t = total_time();
    let karts = find_entities_with("kart_state");
    for k in karts {
        let p = get_position(k);
        let s = get_field(k, "kart_state", "speed");
        let w = get_field(k, "kart_state", "wall_contact");
        let y = get_field(k, "kart_state", "yaw");
        let d = "none";
        if has_component(k, "drift_state") { d = get_field(k, "drift_state", "state"); }
        log("VP " + t + " " + entity_name(k) + " " + p["x"] + " " + p["y"] + " " + p["z"] + " " + s + " " + y + " " + w + " " + d);
    }
}

fn on_event(name, data) {
    if name == "lap_complete" {
        log("VLAP " + total_time() + " " + entity_name(data["kart"]) + " " + data["lap"] + " " + data["lap_time"]);
    }
    if name == "race_start" {
        log("VSTART " + total_time());
    }
}
