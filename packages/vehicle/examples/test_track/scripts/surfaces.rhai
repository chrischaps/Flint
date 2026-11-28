// The sand's change of surface (#1629): how a game shapes the vehicle
// package's drive without forking it. Each frame, for every vehicle, the
// share of its wheels standing on the sand scales its grip and its top
// speed down. The modifiers go back to neutral after each frame's steps,
// so they are written again every frame the kart is on it.

fn on_update() {
    let sand = get_entity("sand");
    for v in vehicle_ids() {
        let wheels = vehicle_wheels(v);
        if wheels.len() == 0 {
            continue;
        }
        let on_sand = 0;
        for w in wheels {
            if w.contact && w.ground == sand {
                on_sand += 1;
            }
        }
        let share = on_sand.to_float() / wheels.len().to_float();
        if share > 0.0 {
            // All four wheels in: 55% of the grip, 65% of the top speed,
            // and a drag that bleeds speed (accel_bonus below 0)
            set_field(v, "vehicle_input", "grip_scale", 1.0 - 0.45 * share);
            set_field(v, "vehicle_input", "speed_scale", 1.0 - 0.35 * share);
            set_field(v, "vehicle_input", "accel_bonus", -6.0 * share);
        }
    }
}
