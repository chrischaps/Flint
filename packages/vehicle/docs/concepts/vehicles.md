# Vehicles

> **Package:** `vehicle`, a feature package: a game has `vehicle`, `vehicle_wheel`, `vehicle_input`, `vehicle_state` and the `vehicle` script module only when its `[packages]` lists `vehicle = "*"` ([Packages](packages.md)).

A vehicle is a body on wheels that physics carries. It is a dynamic `rigidbody` with a `collider` for its chassis, and its children with `vehicle_wheel` are its wheels. Once each fixed physics step, the `vehicle` package casts a ray down from every wheel, pushes the body up on a spring, and turns the tyres' friction into drive, braking and cornering, through Rapier's raycast vehicle. On top of that sit **assists**, forces that pull the body toward arcade handling: a turn rate it holds, sideways grip, staying upright. Each assist is a tuning field, and 0 turns it off. With every assist at 0 the vehicle is a plain raycast car.

Because it is a body, a wall, another vehicle or a ramp is a contact, not a script. Because it steps at the fixed rate (1/60 s), it handles the same at 30 frames a second as at 144. The same scripted input played at 1/30 s and 1/144 s frames ends in the same place after 86 m of driving, to the bit (`crates/flint-player/tests/vehicle.rs` allows 0.25 m). A frame still decides when its input lands: a key pressed on a 1/30 s frame drives both of that frame's steps, so an input change can land one step (16.7 ms) apart at two frame rates. Why this model and not a port of an arcade kart, and what the prototype measured, is [ADR 0140](https://github.com/chrischaps/Flint/blob/main/docs/adr/0140-the-vehicle-package.md).

## A Vehicle in a Scene

A kart that faces -Z, with the front wheels steering and the back ones driving:

```toml
[entities.kart.transform]
position = [0.0, 0.7, 0.0]
[entities.kart.rigidbody]
body_type = "dynamic"
gravity_scale = 2.04          # FlintKart's heavier fall
[entities.kart.collider]
shape = "box"
size = [1.2, 0.5, 2.0]
[entities.kart.vehicle]
top_speed = 33.0
accel = 16.5
[entities.kart.vehicle_input]

[entities.wheel_fl]
parent = "kart"
[entities.wheel_fl.transform]
position = [-0.55, 0.0, -0.75]   # where the spring hangs, in the kart's frame
[entities.wheel_fl.vehicle_wheel]
steer = true

[entities.wheel_rl]
parent = "kart"
[entities.wheel_rl.transform]
position = [-0.55, 0.0, 0.7]
[entities.wheel_rl.vehicle_wheel]
drive = true
handbrake = true
```

and the same for the two right-hand wheels at `x = 0.55`. A wheel's ray hangs `rest_length + radius` (0.35 + 0.275 m) down the body from its position. The ray starts inside the chassis box, so the package leaves the vehicle's own body out of the cast, and every sensor too.

The package sets the body's mass (`vehicle.mass`, replacing the rigidbody's), its centre (`center_of_mass`, 0.25 m low by default) and the inertia of the collider's box. `rigidbody` has no fields for the last two.

With `pose = true` (the default) the package writes each wheel child's transform after the steps: down on its spring, turned by the steering, rolled by its spin. A model under the wheel entity then turns with it. With `pose = false` the game poses its own wheels.

## Driving It: `vehicle_input`

`vehicle_input` is read once each fixed step. A player's bindings and an AI write the same fields, so the two share one path.

- **Driving**: `throttle` (-1 to 1, below 0 reverses), `brake` (0 to 1; near a standstill it reverses instead), `steer` (-1 to 1, positive right), `handbrake` and `hold` (brakes on and the rest ignored, for a countdown or a finish).
- **Modifiers**, how a game shapes the drive without forking it: `grip_scale`, `turn_scale`, `speed_scale`, `speed_bonus` (m/s on the cap), `accel_bonus` (m/s²; negative bleeds speed), `slip_target` (degrees the nose is held off the travel heading, a drift) and `spin` (a yaw rate that replaces the assist's target, a spinout). The package puts them back to neutral after each frame's steps, so a game writes them on every frame it wants them.
- **One-shot**: `push`, a velocity change in the body's frame (m/s, x right, y up, z forward). The next step applies it once and zeroes it. A hop is `push = [0, 6, 0]`.

`vehicle.driver` says who fills the driving half. With `"player"` (the default) the package writes `throttle`, `brake`, `steer` and `handbrake` before the scripts run, from the actions `vehicle.actions` names (`{ throttle, brake, steer, handbrake }`, by default `accelerate`, `brake`, `steering` and `drift`), shaping a stick by `steer_curve`. A script can still override them in `on_update`. With `"script"` the package never writes them, and the driving is an AI's, a replay's or a network peer's:

```rhai
fn on_update() {
    let me = self_entity();
    let state = get_component(me, "vehicle_state");
    // Steer back toward the heading the AI wants, 30 degrees off is full lock
    let off = get_field(me, "ai", "want_yaw") - state.yaw;
    set_field(me, "vehicle_input", "throttle", 1.0);
    set_field(me, "vehicle_input", "steer", clamp(-off / 30.0, -1.0, 1.0));
    if get_field(me, "ai", "boosting") {
        set_field(me, "vehicle_input", "speed_bonus", 10.0);   // again every frame
    }
}
```

A value a script writes in `on_update` reaches the same frame's steps. A frame of 1/30 s runs two steps on the same input, and one of 1/144 s runs none or one.

## Reading It: `vehicle_state`

The package writes `vehicle_state` before the scripts run: `speed` (signed, along the body), `velocity`, `forward`, `lateral_speed`, `slip_angle` (degrees, positive with the nose right of the travel heading), `yaw` (degrees: 0 faces -Z, positive turns left), `yaw_rate`, `yaw_target` (the rate the yaw assist pulled toward), `accel_long` and `accel_lat`, `grounded`, `wheels_grounded`, `airborne_time`, and the chassis' contacts: `contact` (it touches something other than the ground under its wheels), `contact_normal`, `contact_entity` and `impact` (the closing speed, in m/s, as the contact began). Each wheel's `vehicle_wheel` gets `contact`, `ground` (the entity under it, or -1), `normal`, `compression` (0 to 1) and `slip`.

The state comes from the body as the last fixed step began, so it is one step (16.7 ms) stale. Scripts can't yet be lent physics after the steps ([#1647](https://github.com/chrischaps/Flint/issues/1647)).

## Tuning

`vehicle` holds the tuning, and the step reads it every step, so an inspector edit, a slider or a `set_field` takes effect at once.

| Group | Fields |
|---|---|
| Drive | `top_speed`, `accel`, `brake`, `reverse` (a fraction of top speed and accel), `drag` (with `accel` it sets the top speed: 16.5 / 0.5 = 33 m/s), `overspeed_decay`, `mass`, `center_of_mass` |
| Steering | `turn_rate_low` and `turn_rate_high` (degrees a second, at standstill and at top speed), `min_turn_radius`, `steer_response` (full locks a second), `steer_curve` |
| Assists | `grip` (sideways speed scrubbed a second), `yaw_assist` (how fast the yaw rate is pulled to the turn-rate curve), `upright`, `scrub` (speed lost to slip, 0 by default) |
| Tyres and springs | `tyre_grip`, `suspension_stiffness`, `suspension_damping`, `suspension_travel` |
| Control | `driver`, `actions` |

The steering geometry asks for the path the turn-rate curve would drive, and the yaw assist holds it there. FlintKart's `kart_physics` names carry over: `top_speed` is its `max_speed`, `accel` its `acceleration`, `brake` its `brake_decel`.

## The `vehicle` Module

Most of a vehicle is in its components. The module adds what a component can't say, and is enabled in a scene that has a `vehicle`:

<!-- script-api:begin module:vehicle -->
<!-- Generated by `flint script api --book` from the engine's declarations (crates/flint-script/src/api/, or the ScriptModule of a feature that owns its module): edit the doc there, not here. CI's headless job runs --check-book. -->

| Function | Returns | Description |
|---|---|---|
| `vehicle_place(id: entity, x: num, y: num, z: num, yaw: num)` | — | Move a vehicle there, facing yaw degrees (0 faces -Z, positive left), at rest: a respawn or a restart, carried out by the next fixed step. |
| `vehicle_wheels(id: entity)` | `array`: one map a wheel, in the children's order; empty for an entity that isn't a vehicle | A vehicle's wheels, each a map of its entity, contact, ground (an id or -1), normal, compression (0 to 1), slip (degrees), steer and drive. |
| `vehicle_ids()` | `array`: their ids, in id order | The scene's vehicles. |

<!-- script-api:end -->

`vehicle_place` is carried out by the next fixed step, which is the frame after the call: a package's script commands reach it after the steps ([#1648](https://github.com/chrischaps/Flint/issues/1648)). It sets the body's pose through Rapier itself, since a script's `set_position` on a dynamic body is overwritten by the step ([#1650](https://github.com/chrischaps/Flint/issues/1650)).

## The Vehicles Panel

With the debug-hud package in the build, F3 has a Vehicles panel in a scene with a vehicle. It shows each vehicle's speed, slip and yaw rate against the assist's target, and each wheel's contact, spring and slip. It also has sliders for the drive, assists, tyres and springs, which write `vehicle` and so drive the next step. A build without debug-hud has no panel.

## Not Yet

- The chassis' contacts are found by the package against the last step's colliders, since a fixed step can't read Rapier's contact manifolds ([#1649](https://github.com/chrischaps/Flint/issues/1649)).
- A swept road (`spline_mesh`) has no collider without the renderer, so a playtest of one needs `--render` ([#1651](https://github.com/chrischaps/Flint/issues/1651)).
- `[requires] physics` waits for physics to be a package. Until then physics is the kernel's, and every game has it.
- A scene hot reload re-parses the scene and makes its vehicles again.
