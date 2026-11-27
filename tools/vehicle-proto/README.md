# vehicle-proto: the prototype behind ADR 0140 (#1627)

Not engine code, and never merged: the measurement the vehicle ADR reports.
It lives on the `proto/vehicle-1627` branch only.

## The harness (`src/main.rs`)

FlintKart's Oval Plus (`splines/oval_plus.spline.toml`: a 12 m road, walls
0.5 x 1.5 m at +-6.25 m) built in Rapier 0.22 with the engine's integration
parameters (`crates/flint-physics/src/world.rs`). One autopilot, shaped like
FlintKart's `ai_driver.rhai` (steer at a point one lookahead ahead, plan the
fastest speed that still slows for every bend in sight) but on the
centreline, drives four models three laps from a standing start, at frame
steps of 1/30, 1/60 and 1/144 s:

| model | what it is |
|---|---|
| `arcade_frame` | `kart_controller.rhai`'s driving ported to Rust, stepped once a frame at the frame's dt clamped to 0.05 s (FlintKart today) |
| `arcade_fixed` | the same port at the fixed 1/60 s step (option B) |
| `rapier_raw` | Rapier's `DynamicRayCastVehicleController` on a 150 kg dynamic body, tyre friction only |
| `rapier_assist` | the same with arcade assists (option A): yaw rate pulled toward the arcade turn rate, sideways grip, an upright torque |

The tuning is FlintKart's Blaze (`ai1`): `kart_physics` defaults with
max_speed 33, acceleration 16.5. The port leaves out what stays FlintKart's:
drift charge and boost, hop, items, slipstream, kart bumps, respawn and the
body attitude. A simplified drift (a slip-angle target, lower grip, turn rate
x1.2) is in both families to show each can express it.

```
cargo run --release      # prints one JSON line a run, writes runs.json
```

It also runs a skidpad (full lock at 10, 20 and 30 m/s, half lock at 20,
a drift at 20) and a glancing wall hit at 10 and 30 degrees.

## FlintKart's own lap (`flintkart/`)

`lap_probe.rhai` logs every kart's pose, speed and wall contact each frame,
and every `lap_complete`. On a copy of FlintKart (its content without
`engine/`, with this engine's `schemas/` as `engine/schemas`):

```
flint playtest scenes/oval_plus.scene.toml --project . --test-script lap_probe.rhai \
  --exit-after 110 --frame-step 0.0166667 --render --width 320 --height 180 2> race.err
python lap_report.py race.err summary.json
```

`--render` is needed: a `spline_mesh` collider is made by the renderer, so
without one the swept road has no collider and the karts fall.
`make_solo.py` writes `solo_probe.scene.toml`, Oval Plus with Blaze alone.
