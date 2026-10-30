// Joint test bench controller (ADR 0069)
//
// Drives the three jointed rigs in demo/joint_test.scene.toml:
//   * yaws the kinematic slider base with rotate_local(), which drags the
//     sprung block along through the prismatic joint
//   * flips the slider's motor target every two seconds
//   * sweeps the pod's spherical motor with a sine
//   * logs world positions and joint coordinates twice a second
//
// State lives in a `bench` component on this entity because Rhai module
// variables cannot be assigned from callbacks.

fn on_init() {
    let me = self_entity();
    set_field(me, "bench", "t", 0.0);
    set_field(me, "bench", "log_t", 0.0);
    log("[joints] bench ready: pendulum (hinge), slider (prismatic spring), pod (spherical motor), wheel (cylinder)");
}

fn on_update() {
    let me = self_entity();
    let dt = delta_time();
    let t = get_field(me, "bench", "t") + dt;
    set_field(me, "bench", "t", t);

    // Kinematic base turns about its own Y; the block follows through the joint.
    let base = get_entity("slider_base");
    if base >= 0 {
        rotate_local(base, 0.0, 20.0 * dt, 0.0);
    }

    // Spring target flips between +0.4 and -0.4 m every two seconds.
    let block = get_entity("slider_block");
    if block >= 0 {
        let target = if (t % 4.0) < 2.0 { 0.4 } else { -0.4 };
        set_joint_target(block, target);
    }

    // Pod motor sweeps +-25 degrees about Z.
    let pod = get_entity("gimbal_pod");
    if pod >= 0 {
        set_joint_target(pod, 25.0 * sin(t * 1.5));
    }

    let log_t = get_field(me, "bench", "log_t") + dt;
    if log_t >= 0.5 {
        let bob = get_entity("pendulum_bob");
        if bob >= 0 {
            let p = get_world_position(bob);
            log("[joints] bob (" + p.x + ", " + p.y + ", " + p.z + ") angle " + get_joint_position(bob));
        }
        if block >= 0 {
            let p = get_world_position(block);
            log("[joints] block (" + p.x + ", " + p.y + ", " + p.z + ") slide " + get_joint_position(block) + " target " + get_joint_target(block));
        }
        if pod >= 0 {
            let p = get_world_position(pod);
            log("[joints] pod (" + p.x + ", " + p.y + ", " + p.z + ")");
        }
        set_field(me, "bench", "log_t", 0.0);
    } else {
        set_field(me, "bench", "log_t", log_t);
    }
}
