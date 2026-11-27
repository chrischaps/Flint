# ADR 0140: The vehicle package: raycast wheels on a Rapier body with arcade assists, one input for players and AI, and what stays the game's

Status: accepted, 2026-11-27 (#1627). Chris accepted it with every
recommendation under "Decisions" at the end. Epic #1626 (Modularity, Phase 6).
Builds on [ADR 0136](0136-the-package-model.md) (decision 7: `vehicle`
starts beside Phases 2c and 3) and [ADR 0137](0137-the-public-package-api.md)
(section 3a: the fixed-step hook). Beside
[ADR 0134](0134-the-character-controller-stays-in-physics-with-fixed-action-names.md)
(the one controller physics holds today). What it finds the public API can't
do goes to #1612, the ADR for Phase 3's standard packages, while that one is
open.

## Context

`vehicle` is the first genre package, and the first one **written against**
the public Package API instead of moved onto it. Its first client is
FlintKart, and FlintKart drives its karts without Rapier dynamics:

- **One script drives every kart.** `scripts/kart_controller.rhai` (47 KB)
  runs once a frame at the frame's `delta_time`, clamped to 0.05 s. It reads
  the player's actions, or `ai_input` for an AI kart. It casts a ground ray
  and eight wall rays against Rapier's query pipeline, integrates speed,
  sideways slip and yaw by hand, and writes the pose with `set_position` and
  `set_rotation`. The kart has no `rigidbody`, so Rapier never moves it.
- **The same script holds the game.** Around the driving sit the drift
  state machine (hop, drifting, three charge tiers, boost with a kick), item
  boost and spinout, the slipstream, the surface (road, curb or offroad, from
  which `spline_mesh` was hit), the wall response (reflect, scrape, turn
  along), kart-to-kart impulses, off-track and pinned respawn, a
  spring-damper body attitude written to the root's rotation and scale, and
  the events `drift_*`, `wall_hit`, `kart_landed` and `kart_bump`.
- **Nine other scripts read what it writes.** `ai_driver` reads `speed`,
  `yaw`, `grounded`, `wall_contact` and `draft`. `chase_camera`,
  `engine_audio`, `sfx`, `hud`, `kart_fx`, `kart_visuals`, `item_manager` and
  `race_manager` read the rest of `kart_state` (17 fields) and
  `kart_visual_state` (7). `race_manager` and `item_manager` also write
  `kart_state` (a restart, a hit).
- **Handling depends on frame rate.** Measured below, the same AI in the
  same scene runs a lap up to 1.5 s apart at 1/30 s and 1/144 s frames.

The plan names a raycast vehicle on Rapier, with suspension, tyre grip and a
drivetrain. FlintKart's kart grips at once, turns at a rate (degrees a
second, capped by a minimum radius) rather than through a tyre, and drifts
on a button. The issue's risk table rates "a Rapier vehicle doesn't reach an
arcade kart's feel" as likely. So the model was measured before it was
chosen.

ADR 0137 section 3a gives the hook. `Subsystem::fixed_update(&mut FixedCtx)`
runs once a step, after the sync-in and before Rapier's step, and lends the
world, `PhysicsSystem`, `dt`, the step index, `InputState`, the event bus,
the state policy and `Resources`. `PhysicsSystem` has `body_of(entity)`,
`bodies_mut()`, `colliders()`, `queries()` and `rapier_mut()`, which returns
the three sets at once, as `DynamicRayCastVehicleController::update_vehicle`
takes them.

### The three options

The issue names three models for the package. The rest of this record calls
them A, B and C.

- **(A) A dynamic body on raycast wheels, with arcade assists.** The vehicle
  is a dynamic `rigidbody` that Rapier integrates. Rapier's
  `DynamicRayCastVehicleController` casts a ray down from each wheel, pushes
  the body up with a spring and damper, and turns tyre friction into drive,
  braking and cornering forces. On top of that the package adds **assists**:
  forces and torques that pull the body toward arcade behaviour (a fixed
  turn rate, instant sideways grip, staying upright). Each assist is a tuning
  field, and at 0 it is off, leaving a plain physics car.
  - For FlintKart, the kart becomes a body. Walls, other karts and items are
    real contacts, and its handling is matched through the assists' tuning.
  - For another game, a car or a truck is the same model with the assists
    turned down.
- **(B) A kinematic arcade model in Rust.** `kart_controller.rhai`'s driving
  is ported to Rust and run on the fixed step. The kart stays outside
  Rapier's dynamics, as today. The package integrates speed, sideways slip
  and yaw itself, casts its own rays for the ground and the walls, and writes
  the pose.
  - For FlintKart, it handles exactly as now, minus the frame-rate
    dependence.
  - For another game, every vehicle is an arcade kart. Nothing in Rapier can
    push it, so the package also has to carry collision code for walls and
    for vehicles hitting each other.
- **(C) Both, behind one component.** The `vehicle` component chooses (A) or
  (B) with a field such as `model = "dynamic" | "arcade"`. Both models share
  one input, one state and one debug panel.
  - A game picks per vehicle.
  - The package builds, tunes and tests two models.

Option (A) is the plan's model, option (B) is FlintKart's, and option (C)
hedges between them. The prototype below measures (A) and (B), plus (A)
without assists, against FlintKart's own lap.

## Measured: a prototype against FlintKart's lap

The prototype lives on the branch `proto/vehicle-1627` (`974ee527`,
`tools/vehicle-proto/`, never merged). Its README says how to run each part.

- **The track** is FlintKart's Oval Plus, built in Rapier 0.22 with the
  engine's integration parameters (`crates/flint-physics/src/world.rs`: one
  solver iteration of four PGS iterations, gravity -9.81). It uses the same
  Catmull-Rom sampling as `flint_geometry::spline` and the scene's
  `spline_mesh` sizes: a 12 m road (friction 0.8) and 0.5 x 1.5 m walls at
  ±6.25 m (friction 0.3, restitution 0.5).
- **One autopilot** drives every model. It is shaped like `ai_driver.rhai`
  (steer at a point one lookahead ahead, plan the fastest speed that still
  slows in time for every bend in sight), but drives the centreline, with no
  racing line, pack or drift. Each model tells it the corner speed it can
  hold.
- **Four models**, all with Blaze's tuning (`ai1`: `kart_physics` defaults
  with `max_speed` 33 and `acceleration` 16.5):
  - `arcade_frame`: `kart_controller.rhai`'s driving ported to Rust, stepped
    once a frame at the frame's dt, as FlintKart is today.
  - `arcade_fixed`: the same port at the fixed 1/60 s step. This is option
    (B).
  - `rapier_raw`: `DynamicRayCastVehicleController` on a 150 kg dynamic
    body (centre of mass 0.25 m low, gravity scale 2.04 for the kart's
    -20 m/s²), with four 0.275 m wheels, tyre friction 1.8 and the kart's
    drag. The steering angle is set so that the wheel asks for the arcade
    model's path.
  - `rapier_assist`: the same body with three arcade assists. This is
    option (A). A **yaw assist** pulls the yaw rate toward the arcade turn rate
    at 10/s. **Grip** scrubs sideways speed at 9/s, as the arcade `grip`
    does. An **upright** torque turns the body's up toward the ground's.
  The port leaves out what stays FlintKart's (section 3). A simplified drift
  is in both families (a slip-angle target, grip 3.5, turn rate x1.2), to
  show that each can express one.
- **FlintKart's own lap** comes from the game itself.
  `flint playtest scenes/oval_plus.scene.toml --render` ran on a copy of
  FlintKart with this branch's `flint` and schemas, with a probe script
  (`lap_probe.rhai`) logging every kart each frame. It ran the full race (six
  karts, items, the slipstream) and Blaze alone (`make_solo.py`). The
  tightest corner held is the smallest speed / yaw rate over a 0.5 s window
  at more than 10 m/s with no wall contact. The prototype measures it the
  same way.

### A lap of Oval Plus

Laps 1 to 3 from a standing start. The frame step is the game's; the
fixed-step models always step at 1/60 s.

| | frame | laps (s) | top speed (m/s) | tightest corner held |
|---|---|---|---|---|
| **FlintKart, full race** (Blaze, its own AI) | 1/30 | 23.33, 21.60, 21.70 | 33.5 | 19.7 m at 26.0 m/s |
| | 1/60 | 23.38, 22.80, 21.68 | 34.0 | 19.1 m at 25.9 m/s |
| | 1/144 | 23.38, 21.64, 21.44 | 33.2 | 18.7 m at 25.8 m/s |
| **FlintKart, Blaze alone** | 1/30 | 23.03, 24.43, 21.57 | 34.7 | |
| | 1/60 | 23.08, 24.65, 21.42 | 34.7 | |
| | 1/144 | 23.12, 25.90, 21.43 | 34.7 | |
| `arcade_frame` (today's model, autopilot) | 1/30 | 23.93, 21.97, 22.00 | 32.0 | 18.2 m at 25.9 m/s |
| | 1/60 | 23.90, 22.00, 22.00 | 32.0 | 17.8 m at 25.8 m/s |
| | 1/144 | 23.89, 22.01, 22.01 | 32.0 | 17.5 m at 25.7 m/s |
| `arcade_fixed` (option (B)) | 1/30, 1/60, 1/144 | lap 3: 22.03, 22.00, 22.02 | 32.0 | 17.6-17.9 m at 25.8 m/s |
| `rapier_raw` | 1/30, 1/60, 1/144 | lap 3: 22.37, 22.35, 22.36 | 32.1 | 16.7-16.9 m at 24.5 m/s |
| **`rapier_assist` (option (A))** | 1/30 | 23.70, 21.83, 21.83 | 32.1 | 19.0 m at 26.4 m/s |
| | 1/60 | 23.65, 21.83, 21.80 | 32.1 | 18.6 m at 26.4 m/s |
| | 1/144 | 23.66, 21.82, 21.81 | 32.1 | 19.0 m at 26.2 m/s |

No model touched a wall on any lap, and neither did Blaze. Blaze never
drifted on Oval Plus either (its `drift_state` read `none` every frame), so
the reference laps hold no drift. Alone, Blaze rubber-bands against the
parked player, which is why its second lap is slow. FlintKart's laps are
faster than the autopilot's because its AI takes a racing line and its top
speed includes the slipstream (`draft_speed` 3).

### The skidpad, a drift and a wall

On a flat plane, with the throttle holding the speed. "Rise" is the time
from the steering step to 63% of the settled yaw rate.

| | full lock, 10 m/s | full lock, 20 m/s | full lock, 30 m/s | half lock, 20 m/s | drift, 20 m/s, steer 0.6 | rise |
|---|---|---|---|---|---|---|
| arcade | 4.0 m | 9.4 m, slows to 18.6 | 10.9 m, slows to 20.3 | 20.9 m | 8.1 m, slip 45°, slows to 13.7 | 50-150 ms |
| rapier_raw | 4.6 m | 14.2 m | 35.2 m | 22.2 m | 15.4 m, slip 10° (no hold) | 33-67 ms |
| **rapier_assist** | 4.4 m | 12.2 m | 27.5 m, holds 29.7 | 21.5 m | 6.9 m, slip 17°, slows to 12.9 | 33 ms |

A glancing hit at 25 m/s with the wheel straight, read 1.5 s later:

| | 10° wall | 30° wall |
|---|---|---|
| arcade (reflect, scrape, turn along) | 9.5 m/s, turned 10° (along the wall) | 4.3 m/s, turned 30° |
| rapier_raw | 11.1 m/s, turned 10° | 7.9 m/s, **spun 51°** |
| rapier_assist | 10.9 m/s, turned 10° | 7.4 m/s, turned 30° |

The cost was measured in a release build on this machine, per simulated
1/60 s, with the autopilot and the world's whole step. The arcade port takes
17 µs. Either Rapier model takes 88 µs, most of it Rapier stepping a world of
about 650 wall boxes, which a game with karts already pays.

### What it shows

- **Option (A) reaches the arcade lap.** On the same autopilot, the assisted
  Rapier kart runs the flying lap in 21.80 s against the port's 22.00, at
  the same top speed, and holds a corner as tight (18.6 m at 26.4 m/s
  against 17.8 m at 25.8 m/s). FlintKart's own flying lap is 21.68 s, with
  its racing line and slipstream.
- **Raw Rapier is the one that falls short.** It is 0.35 s a lap slower. At
  speed it turns on twice the radius (35 m against 27.5 m at 30 m/s), and a
  30° wall spins it 51°. The assists are what close the gap. Without them
  option (A) would make FlintKart's handling worse, as the risk table feared.
- **The fixed step removes the frame-rate spread.** The fixed-step models'
  laps agree within 0.04 s from 1/30 to 1/144. FlintKart today spreads its
  laps by 0.2 s (the first lap), 1.2 s (the full race's second lap) and
  1.5 s (the solo second lap). The prototype's `arcade_frame` spreads only
  0.03 s, so most of FlintKart's spread is the AI and the items deciding
  differently on different frames rather than the integrator. A fixed-step
  vehicle still makes the physics half deterministic.
- **The two families feel different at the limit.** The arcade kart turned
  to full lock at 30 m/s bleeds to 20 m/s and turns on 10.9 m: its velocity
  lags the turning heading, and grip scrubs away what lags, speed and all.
  The assisted body holds 29.7 m/s on 27.5 m. Its arcade drift slips 45° to
  the assisted body's 17°. Both are tuning, not structure: a `scrub` assist
  (speed lost to slip) and the drift's slip target. FlintKart's A/B (#1630)
  sets them.
- **A drift is expressible from outside.** Lower grip, the turn rate x1.2
  and a slip angle the yaw assist holds put the assisted body on a 6.9 m
  drift circle against the arcade's 8.1 m. All three are inputs a game
  writes each frame (section 2), so the drift stays FlintKart's.

## Decision

### 1. One model: a dynamic body on Rapier's raycast wheels, with arcade assists as tuning

This is option (A) of "The three options" above. Why not (B) or (C) is under
"Alternatives considered".

A vehicle is an entity with a dynamic `rigidbody`, a `collider` and a
`vehicle` component. Its wheels are child entities with `vehicle_wheel`. The
package keeps one `DynamicRayCastVehicleController` for each vehicle. Each
fixed step it sets the wheels' steering, engine force and brake from the
input, calls `update_vehicle` through `PhysicsSystem::rapier_mut()`, and
applies its assists and the drag as impulses that Rapier's step then
integrates. A rule of thumb: the wheels and the body are physics, and every
assist is a tuning field whose value 0 turns it off. With every assist at 0
it is a plain raycast car. The assists are:

- **steering limit**: the wheel angle at full lock comes from the turn-rate
  curve (`turn_rate_low` to `turn_rate_high` degrees a second over the speed
  range, capped by `min_turn_radius`), so the wheel asks for the arcade path;
- **yaw assist**: the yaw rate is pulled toward that curve at
  `yaw_assist` per second;
- **grip**: sideways speed (of the travel heading) is scrubbed at `grip` per
  second;
- **upright**: a torque turns the body's up toward the ground's,
  `upright` per second squared;
- **drag** at `drag` per second, which with `accel` sets the top speed
  (16.5 / 0.5 = 33 m/s);
- **overspeed**: speed above the current cap eases back at
  `overspeed_decay` per second;
- **scrub** (new, from the skidpad): forward speed lost in proportion to
  slip, for a kart that slows when thrown into a corner. It defaults to 0,
  and #1630 tunes it against FlintKart.

Being a body is what makes walls, other karts, items and ramps contacts
rather than scripts. It is also why one model serves a kart and a car.

### 2. One input, `vehicle_input`, that a player's bindings and an AI both write

`vehicle_input` is a component on the vehicle, read once each fixed step.
The game writes it (a script's `set_field` lands in the world at once, so a
value written in `on_update` reaches the same frame's steps).

- **Driving**: `throttle` (-1 to 1, below 0 reverses), `brake` (0 to 1),
  `steer` (-1 to 1, positive right), `handbrake` (bool) and `hold` (bool:
  brakes on and the rest ignored, for a countdown or a finish).
- **Modifiers**, which is how a game shapes the drive without forking it.
  They return to neutral when not written. They are `grip_scale`,
  `turn_scale`, `speed_scale`, `speed_bonus` (m/s on the cap),
  `accel_bonus` (m/s², and negative bleeds), `slip_target` (degrees the yaw
  assist holds the nose off the travel heading) and `spin` (a yaw rate in
  degrees a second that replaces the assist's target while non-zero).
- **One-shot**: `push`, a velocity change in the body's frame (m/s, x right,
  y up, z forward). The step applies it once and zeroes it.

`vehicle.driver` says who fills the driving half. With `"player"`, the
package writes `throttle`, `brake`, `steer` and `handbrake` at
`Stage::PreScripts`, from the actions named in
`vehicle.actions` (default `{ throttle = "accelerate", brake = "brake",
steer = "steering", handbrake = "drift" }`, FlintKart's names), with
`steer_curve` shaping a stick. A script can still override them in
`on_update`. With `"script"` (an AI, a replay, a network peer) the package
never writes them. The modifiers and `push` are always the game's. An AI and
a player then share one path, as FlintKart's `ai_input` meant: `ai_driver`
writes `vehicle_input` instead of `ai_input`'s driving fields, and keeps
`drift` and `use_item` in FlintKart's own component.

### 3. What stays FlintKart's, and what it reads and writes

| FlintKart keeps | It reads | It writes |
|---|---|---|
| drift: hop, drifting, charge tiers, boost and kick | `speed`, `steer`, `grounded`, its surface | hop: `push = [0, hop_speed, 0]`; drifting: `grip_scale = drift_grip / grip`, `turn_scale = 1.2`, `slip_target = drift_angle`, `accel_bonus = -0.6`; boost: `speed_bonus`, `accel_bonus`, the kick `push = [0, 0, bonus x boost_kick]` |
| items: item boost, spinout | | `speed_bonus = 10`, `accel_bonus = 12`; spinout: `spin`, `hold` |
| slipstream | other karts' `vehicle_state.forward` and positions | `speed_bonus = draft_speed x draft` |
| surface: road, curb, offroad | each wheel's `ground` entity | offroad: `speed_scale = offroad_speed_factor`, `grip_scale` |
| wall scrapes and their events | `contact`, `contact_normal`, `impact` | `spin` to turn along a wall, if Rapier's contact alone feels wrong in #1630 |
| kart bumps and their events | `contact` with a kart, `impact` | nothing: two bodies collide |
| respawn and race restart | `vehicle_state`, its spline | `vehicle_place(id, ...)` |
| body attitude (roll, pitch, squash) | `vehicle_state` accelerations, `yaw_rate`, landings | a child body entity's rotation and scale. The root is the physics body now, so the mesh moves to a child |
| chase camera (until #1631) | `vehicle_state` | the camera, as today |

`vehicle_state` (written by the package, read by scripts) has `speed`
(signed, along the body), `velocity`, `forward`, `lateral_speed`,
`slip_angle` (degrees), `yaw` (degrees, FlintKart's convention: 0 faces -Z,
positive turns left), `yaw_rate`, `accel_long` and `accel_lat` (m/s², what
the attitude spring and the camera read), `grounded` (any wheel),
`wheels_grounded`, `airborne_time`, `contact` (the chassis touches something
other than the ground), `contact_normal`, `contact_entity` and `impact` (the
closing speed of the last hit, m/s). Each wheel's `vehicle_wheel` gets
`contact`, `ground` (the entity under it), `normal`, `compression` (0 to 1)
and `slip`.

### 4. Components

- **`vehicle`** holds the tuning, authored and read each step. Where a field
  carries over from FlintKart's `kart_physics`, it keeps that field's
  meaning:
  - Drive: `top_speed` (`max_speed`), `accel` (`acceleration`), `brake`
    (`brake_decel`), `reverse` (fraction of top speed, 0.3), `drag`,
    `overspeed_decay`, `mass`.
  - Steering: `turn_rate_low`, `turn_rate_high`, `min_turn_radius`,
    `steer_response` (locks a second), `steer_curve` (player only).
  - Assists: `grip`, `yaw_assist`, `upright`, `scrub`.
  - Tyres and suspension: `tyre_grip` (Rapier's `friction_slip`),
    `suspension_stiffness`, `suspension_damping`, `suspension_travel`. The
    body sits on them, so `gravity` becomes the `rigidbody`'s
    `gravity_scale`, and `ground_ray_length` becomes each wheel's
    `rest_length + radius`.
  - Control: `driver`, `actions`.

  FlintKart's `grip`, `drift_grip` and `offroad_grip` map onto `grip` and
  the `grip_scale` FlintKart writes. The drift tiers, boosts, slipstream,
  walls, bumps and attitude fields stay in `kart_physics`, which FlintKart
  keeps for its own logic.
- **`vehicle_wheel`**, on a child entity, is placed by its transform:
  `radius`, `rest_length`, `steer`, `drive`, `brake` and `handbrake`
  (bools), and `pose` (true: the package writes the child's transform for
  suspension, steering and spin at `Stage::Physics`; false leaves it to the
  game's visuals, as `kart_visuals.rhai` does today). It also holds the
  per-wheel state of section 3.
- **`vehicle_input`** and **`vehicle_state`**, as in sections 2 and 3.

The four schemas live in `packages/vehicle/schemas/components/`. The
manifest's `[provides]` lists them and `[requires]` names `physics`.

### 5. The `vehicle` script module

Most of what scripts need is in components, read with `get_field`. The
module adds what a component can't say:

- `vehicle_place(id, x, y, z, yaw)` moves a vehicle (respawn, restart) and
  zeroes its velocity. It is taken through `Subsystem::script_command` and
  applied at the next step: the frame after the call (gap 2).
- `vehicle_wheels(id)` returns each wheel's state as an array of maps, so a
  script needn't find the children.
- `vehicle_ids()` returns the vehicles in the scene.

The module is enabled where a scene has a `vehicle`, as the toy package's is
(ADR 0137, #1560).

### 6. The hook and the stages

- **`scene_loaded`**: one entry per entity with `vehicle`, its wheels from
  the children with `vehicle_wheel`. The controller is made at the first
  `fixed_update` that finds the body (`body_of` answers only after a step's
  sync-in has made it). The package sets the body's centre of mass and
  inertia through `bodies_mut()`, since `rigidbody` has no such fields.
- **`Stage::PreScripts`**: write `vehicle_state` and each wheel's state from
  the last step, and a player's driving input from the actions.
- **`fixed_update`**, once a step: read `vehicle` (so tuning is live) and
  `vehicle_input`, set the wheels, run `update_vehicle` against the query
  pipeline (filter: not the vehicle's own body, no sensors), apply the
  assists, modifiers, drag and `push`, then zero `push`. Input is held for
  every step of a frame. A frame of 1/30 s runs two steps on the same
  input, as the character controller does (#728).
- **`Stage::Physics`**: write the wheel children's transforms (`pose =
  true`). The chassis pose is already in the world from physics' write-back,
  so a wheel's local transform is all that's left to write.

A capture runs no physics (ADR 0137 section 3a), so it runs no vehicle
either. A paused state skips the hook with Rapier.

### 7. The debug panel and hot tuning

The panel goes through debug-hud's registration point. At `scene_loaded` the
package asks for `resources.get_mut::<DebugPanels>()` and adds a "Vehicles"
panel when the answer is `Some`. Its `sync` reads `vehicle_state` and the
wheels from the world. It shows each vehicle's speed, slip, yaw rate against
the assist's target, and each wheel's contact and compression. Sliders for
the `vehicle` fields write them back with `set_field`.

Tuning is hot because the step reads the component every step, so an
inspector edit, a slider or a script's `set_field` takes effect on the next
step. A scene file's hot reload re-parses the scene (Flint's reload is a full
re-parse) and makes the vehicles again.

### 8. Frame rate and the gate's tolerance

The vehicle steps at the fixed rate, so its handling doesn't depend on the
frame. The prototype's fixed-step laps agree within 0.04 s at 1/30 and
1/144 s frames. The epic's gate ("the same scripted lap ends within a stated
tolerance at 1/30 s and 1/144 s") states the tolerance as **0.1 s on a lap**
for an input script driving the example track (#1629). FlintKart's own A/B
(#1630) runs its AI, which decides per frame. Its tolerance is set there
from a control run, two runs at the same frame step.

### 9. What the public API can't do

Each gap is an issue. The kernel ones are linked from #1612.

1. **No read of physics after the steps** (#1647, for #1612). `StageCtx` lends no
   `PhysicsSystem`, and `fixed_update` runs only before Rapier's step. A
   package can't see a step's result (the body's velocity after the frame's
   last step) until the next step. `vehicle_state.speed`, `velocity` and
   `yaw_rate` then reach scripts one step (16.7 ms) stale. The vehicle ships
   with that until the gap closes. A read-only lend at `Stage::Physics`, or
   an after-step hook, closes it.
2. **A package's script commands arrive after the steps** (#1648, for #1612). The
   player dispatches script commands after `Stage::Physics`, so a module
   call made in `on_update` reaches the package's `fixed_update` a frame
   late. The vehicle keeps per-frame input in components for this reason,
   and `vehicle_place` is a frame late.
3. **A fixed step can't read a body's contacts** (#1649, for #1612 and #1616).
   `PhysicsSystem` lends the bodies, colliders and queries but not the
   narrow phase. `vehicle_state.contact_normal` and `impact` (FlintKart's
   `wall_impact`, a `kart_bump`'s strength) need a body's contact manifolds.
4. **A script can't move a dynamic body** (#1650, physics). `set_position` on one
   is overwritten by the step's write-back (`PhysicsSync::sync_from_rapier`).
   Every game with dynamic bodies needs a respawn. Until physics has one, the
   vehicle's `vehicle_place` sets the body's pose through Rapier itself.
5. **A swept road has no collider without the renderer** (#1651, kernel). A
   `spline_mesh` collider is made in the renderer's `load_scene_splines`
   (`scene_entry.rs`), so `flint playtest` without `--render`, and
   `flint render --scripts`, have no road. This prototype's FlintKart laps
   needed `--render`, and so will #1629's playtest.

These are the vehicle package's own, in #1628's scope:

- the wheel children's poses at `Stage::Physics`;
- the centre of mass and inertia through `bodies_mut()`;
- `vehicle_place` through Rapier, until gap 4 closes;
- the "Vehicles" panel.

## Alternatives considered

- **(B) A kinematic arcade model in Rust**, a port of `kart_controller.rhai`
  to the fixed step. It measured the same as today's model and holds
  FlintKart's feel exactly. It would put FlintKart's collision (eight wall
  rays, impulses written into another kart's state) into the package, and
  make every racing game an arcade kart that nothing in Rapier can push.
  The prototype's port stays on the proto branch as the reference #1630
  A/Bs against.
- **(C) Both, behind one component** (`vehicle.model = "arcade" |
  "dynamic"`). This doubles the tuning surface, the tests and the debug
  panel, for a gap the measurement didn't show. The door stays open: if
  #1630's A/B fails on feel after the assists are tuned, an `arcade` model
  joins behind the same input and state, and the port already exists.
- **Raw Rapier, no assists.** It is a real car, but measured 0.35 s a lap
  slower, wide at speed and spun by a wall. A game that wants it sets every
  assist to 0.
- **The vehicle inside physics**, as the character controller is (ADR
  0134). That was right for a controller every first-person game needs
  before packages had a fixed-step hook. A vehicle is a genre's, and ADR
  0137 gave packages the hook so it needn't be physics'.
- **State through the script module only**, with no `vehicle_state`
  component. Nine of FlintKart's scripts read kart state with `get_field`
  today, and a component is what the inspector, the debug panel and
  `flint playtest --dump` already show.
- **Input as script-module calls** (`vehicle_drive(id, throttle, ...)`).
  Module calls reach the package a frame late (gap 2). A component written
  in `on_update` reaches the same frame's steps.

## Consequences

- `packages/vehicle/` (#1628): a `package.toml` (feature tier, `[requires]
  physics`), the crate `flint-vehicle`, the four schemas, the module and the
  panel. Its crate depends only on the kernel and `flint-physics`, as #1624's
  rule asks.
- #1629's example and playtest need `--render` until gap 5 closes.
- #1630 is a bigger change for FlintKart than swapping a script. The kart
  becomes a body, its mesh moves to a child for the attitude spring, walls
  and bumps become contacts, and `ai_driver` writes `vehicle_input`. The
  drift, items, slipstream and surface stay its own and drive the modifiers.
  The A/B is against the prototype's lap above and a control run.
- When #1616 and #1617 move `FixedCtx::physics` into physics' resource, the
  vehicle moves with them in the same PR, as those tickets say.
- The chase camera stays in FlintKart's scripts until #1614's player and
  view, and moves in #1631.

## Decisions

Chris accepted every recommendation below on 2026-11-27. All nine are
settled: the package is option (A), and (C) stays open only if #1630's
frame-step A/B fails on feel.

1. **The model.** (A) a dynamic body on `DynamicRayCastVehicleController`
   with arcade assists as tuning; (B) a kinematic arcade model in Rust;
   (C) both behind one component. Recommended: **(A)**, with (C) left open
   behind #1630's A/B.
2. **One input**, `vehicle_input`, with driving fields, modifiers and a
   one-shot `push`; `vehicle.driver = "player"` fills the driving half from
   named actions, `"script"` leaves it to the game. Recommended as written.
3. **What stays FlintKart's**: drift, hop, boost, items, slipstream,
   surface, wall and bump events, respawn policy, body attitude and (until
   #1631) the chase camera, reading `vehicle_state` and writing the
   modifiers. Recommended as written.
4. **Components**: `vehicle`, `vehicle_wheel` (on child entities),
   `vehicle_input` and `vehicle_state`, with the fields of sections 3 and 4.
   Recommended as written.
5. **The script module**: `vehicle_place`, `vehicle_wheels` and
   `vehicle_ids`, with state in components. Recommended as written.
6. **The hook and stages**: state at `Stage::PreScripts`, forces in
   `fixed_update`, wheel poses at `Stage::Physics`, with state one step stale
   until gap 1 closes. Recommended as written.
7. **The panel and hot tuning**: a "Vehicles" panel through `DebugPanels`,
   and tuning read every step. Recommended as written.
8. **The gate's tolerance**: 0.1 s on a lap for #1629's input script at
   1/30 s and 1/144 s; FlintKart's set from a control run in #1630.
   Recommended as written.
9. **The gaps**: #1647, #1648 and #1649 answered in #1612's ADR, #1650
   in physics and #1651 in the kernel, each naming epic #1626. The vehicle
   ships around all five. Recommended as written.
