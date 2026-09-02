// Third-person camera controller
// Uses the first-person character controller for movement/mouse input,
// then repositions the camera behind and above the player each frame.

let cam_distance = 10.0;
let cam_height = 2.5;
let look_height = 1.2;

fn on_update() {
    let me = self_entity();
    let pos = get_position(me);

    // Camera direction comes from the character controller's yaw/pitch
    let dir = get_camera_direction();

    // Place camera behind the player along the look direction
    set_camera_position(
        pos.x - dir.x * cam_distance,
        pos.y - dir.y * cam_distance + cam_height,
        pos.z - dir.z * cam_distance
    );

    // Look at the player's upper body
    set_camera_target(pos.x, pos.y + look_height, pos.z);
}
