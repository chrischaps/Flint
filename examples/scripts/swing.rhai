// Swings its entity about z like a pendulum (swinging_lamp.scene.toml).
// A spot light under it with no `direction` shines down the arm, so the
// beam sweeps the yard as the arm swings (#682).

const REACH = 38.0;  // degrees either side
const PERIOD = 4.0;  // seconds a full swing

fn on_update() {
    let t = total_time() * 2.0 * PI() / global::PERIOD;
    set_rotation(self_entity(), 0.0, 0.0, global::REACH * sin(t));
}
