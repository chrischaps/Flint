# Scripting

Flint's scripting system provides runtime game logic through [Rhai](https://rhai.rs/), a lightweight embedded scripting language. Scripts can read and write entity data, respond to game events, control animation and audio, and hot-reload while the game is running.

## Overview

The `flint-script` crate integrates Rhai into the game loop:

- **ScriptEngine** --- compiles and runs `.rhai` scripts, manages per-entity state (scope, AST, callbacks)
- **ScriptSync** --- discovers entities with `script` components, handles hot-reload by watching file timestamps
- **ScriptSystem** --- implements `RuntimeSystem` for game loop integration, running in `update()` (variable-rate)

Scripts run each frame during the `update()` phase, after physics and before rendering. This gives them access to the latest physics state while allowing their output to affect the current frame's visuals.

## Script Component

Attach a script to any entity with the `script` component:

```toml
[entities.my_door]
archetype = "door"

[entities.my_door.script]
source = "door_interact.rhai"
enabled = true
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `source` | string | `""` | Path to `.rhai` file (relative to the `scripts/` directory) |
| `enabled` | bool | `true` | Whether the script is active |

Script files live in the `scripts/` directory next to your scene file.

## Event Callbacks

Scripts define behavior through callback functions. The engine detects which callbacks are defined in each script's AST and only calls those that exist:

| Callback | Signature | When It Fires |
|----------|-----------|---------------|
| `on_init` | `fn on_init()` | Once when the script is first loaded |
| `on_update` | `fn on_update()` | Every frame. Use `delta_time()` for frame delta |
| `on_collision` | `fn on_collision(other_id)` | When this entity collides with another |
| `on_trigger_enter` | `fn on_trigger_enter(other_id)` | When another entity enters a trigger volume |
| `on_trigger_exit` | `fn on_trigger_exit(other_id)` | When another entity exits a trigger volume |
| `on_action` | `fn on_action(action_name)` | When an input action fires (e.g., `"jump"`, `"interact"`) |
| `on_interact` | `fn on_interact()` | When the player presses Interact near this entity |
| `on_draw_ui` | `fn on_draw_ui()` | Every frame after `on_update`, for 2D HUD draw commands |
| `on_collision_exit` | `fn on_collision_exit(other_id)` | When a contact with another entity ends |
| `on_scene_enter` / `on_scene_exit` | `fn on_scene_enter()` | Around scene transitions (see [Scene Transition API](#scene-transition-api)) |
| `on_sequence_cue` | `fn on_sequence_cue(sequence, cue)` | An animation sequence passed a `cue` event (see [Animation](animation.md#sequences)) |
| `on_animation_end` | `fn on_animation_end(clip)` | A `once` sprite clip finished (see [2D Sprites](sprites-2d.md)) |
| `on_event` | `fn on_event(name, data)` | A custom game event fired by a script, the engine or a player extension (see [Event API](#event-api)) |

The `on_interact` callback is sugar for the common pattern of proximity-based interaction. It automatically checks the entity's `interactable` component for `range` (default 3.0) and `enabled` (default true) before firing.

## API Reference

All functions are available globally in every script. Entity IDs are passed as `i64` (Rhai's native integer type).

### Entity API

| Function | Returns | Description |
|----------|---------|-------------|
| `self_entity()` | `i64` | The entity ID of the entity this script is attached to |
| `this_entity()` | `i64` | Alias for `self_entity()` |
| `get_entity(name)` | `i64` | Look up an entity by name. Returns `-1` if not found |
| `entity_exists(id)` | `bool` | Check whether an entity ID is valid |
| `entity_name(id)` | `String` | Get the name of an entity |
| `has_component(id, component)` | `bool` | Check if an entity has a specific component |
| `get_component(id, component)` | `Map` | Get an entire component as a map (or `()` if missing) |
| `get_field(id, component, field)` | `Dynamic` | Read a component field value |
| `set_field(id, component, field, value)` | --- | Write a component field value |
| `get_position(id)` | `Map` | Get entity position as `#{x, y, z}` |
| `set_position(id, x, y, z)` | --- | Set entity position |
| `get_rotation(id)` | `Map` | Get entity rotation (euler degrees) as `#{x, y, z}` |
| `set_rotation(id, x, y, z)` | --- | Set entity rotation (euler degrees); clears any `rotation_quat` |
| `get_rotation_quat(id)` | `Map` | The rotation the entity renders with, as `#{x, y, z, w}` (`rotation_quat` if present, else the euler angles converted) |
| `set_rotation_quat(id, x, y, z, w)` | --- | Set the rotation as a quaternion (normalised); euler `rotation` is zeroed |
| `rotate_local(id, x, y, z)` | --- | Compose a rotation (euler degrees) about the entity's own axes onto its current orientation, rest quaternion included (ADR 0069) |
| `distance(a, b)` | `f64` | Euclidean distance between two entities |
| `set_parent(child_id, parent_id)` | --- | Set an entity's parent in the hierarchy |
| `get_parent(id)` | `i64` | Get the parent entity ID (`-1` if none) |
| `get_children(id)` | `Array` | Get child entity IDs as an array |
| `get_world_position(id)` | `Map` | World-space position as `#{x, y, z}` (accounts for parent transforms) |
| `set_material_color(id, r, g, b, a)` | --- | Set the material base color (RGBA, 0.0--1.0) |
| `set_material_override(id, material, r, g, b, a)` | --- | Tint one named glTF material on this entity and all its expanded child nodes (writes `material_overrides.<material>`) |
| `find_entities_with(component)` | `Array` | All entity IDs that have the given component |
| `entity_count_with(component)` | `i64` | Count of entities with the given component |
| `spawn_entity(name)` | `i64` | Create a new entity. Returns its ID or `-1` on failure |
| `despawn_entity(id)` | --- | Remove an entity from the world |

### Input API

| Function | Returns | Description |
|----------|---------|-------------|
| `is_action_pressed(action)` | `bool` | Whether an action is currently held |
| `is_action_just_pressed(action)` | `bool` | Whether an action was pressed this frame |
| `is_action_just_released(action)` | `bool` | Whether an action was released this frame |
| `action_value(action)` | `f64` | Analog value for Axis1d actions (0.0 if not bound) |
| `mouse_delta_x()` | `f64` | Horizontal mouse movement this frame |
| `mouse_delta_y()` | `f64` | Vertical mouse movement this frame |
| `mouse_wheel_x()` / `mouse_wheel_y()` | `f64` | Wheel lines (notches) this frame; up and right are positive. A touchpad's scroll arrives as fractions, 50 px to a line. Zero while the cursor is over egui |
| `mouse_x()` / `mouse_y()` | `f64` | Cursor position in logical points (the `screen_width()` / `draw_*` space) |
| `is_mouse_pressed(button)` | `bool` | Mouse button held: `0` left, `1` right, `2` middle |
| `is_mouse_just_pressed(button)` | `bool` | Mouse button went down this frame |

`mouse_x()` / `mouse_y()` and the button queries are what menus and other screen-space UI hit-test against. The buttons are always fed, whether or not the cursor is captured. While it's captured, `mouse_x()` / `mouse_y()` read the window centre, and clicks skip egui ([Cursor Capture and Input Gates](input-gates.md)). In scenes with a player character the first left click also captures the cursor for mouse-look (it still registers as a press).

Action names are defined by input configuration files and are fully customizable per game. The built-in defaults include: `move_forward`, `move_backward`, `move_left`, `move_right`, `jump`, `interact`, `sprint`, `weapon_1`, `weapon_2`, `reload`, `fire`. Games can define arbitrary custom actions in their input config TOML files and query them from scripts with `is_action_pressed("custom_action")`.

Input bindings support keyboard, mouse, and gamepad devices. See [Physics and Runtime: Input System](physics-and-runtime.md#input-system) for the config file format and layered loading model.

### Time API

| Function | Returns | Description |
|----------|---------|-------------|
| `delta_time()` | `f64` | Seconds since last frame (scaled by `set_time_scale`) |
| `set_time_scale(scale)` | --- | Game seconds per real second from the next frame (1.0 normal, 0.25 slow motion, clamped 0.05–4). Physics, particles and animation slow with it; audio does not. Held until changed; a scene load resets it to 1.0 |
| `total_time()` | `f64` | Total elapsed time since scene start |

### Audio API

Audio functions produce deferred commands that the player processes after the script update phase:

| Function | Description |
|----------|-------------|
| `play_sound(name)` | Play a non-spatial sound at default volume |
| `play_sound(name, volume)` | Play a non-spatial sound at the given volume (0.0--1.0) |
| `play_sound(name, volume, pitch)` | As above, with a pitch multiplier |
| `play_sound_at(name, x, y, z, volume)` | Play a spatial sound at a 3D position |
| `play_sound_at(name, x, y, z, volume, pitch)` | As above, with a pitch multiplier |
| `play_sound_at(name, x, y, z, opts)` | A spatial sound with a map of options, each optional: `volume` (1), `pitch` (1), `min_distance` (1), `max_distance` (25), `rolloff` (2) |
| `stop_sound(name)` | Stop a playing sound |

Sound names match the audio files loaded from the `audio/` directory (without extension). A one-shot naming a file that is not there fails silently — it logs a warning and plays nothing.

Varying `pitch` slightly per trigger (say 0.9–1.1) is the cheapest way to stop a
repeated one-shot sounding like a repeated one-shot.

By default a spatial one-shot fades to silence at 25 m. A sound meant to be
heard further away sets `max_distance` in the options map, and usually a
`rolloff` below 1 so it fades like a sound in the open. See
[Audio: Scripting](audio.md#scripting-integration).

### Animation API

Animation functions write directly to the `animator` component on the target entity. The `AnimationSync` system picks up changes on the next frame:

| Function | Description |
|----------|-------------|
| `play_clip(entity_id, clip_name)` | Start playing a named animation clip |
| `stop_clip(entity_id)` | Stop the current animation |
| `blend_to(entity_id, clip, duration)` | Crossfade to another clip over the given duration |
| `set_anim_speed(entity_id, speed)` | Set animation playback speed |
| `set_anim_layer(entity_id, index, clip, weight)` | Play `clip` on layer `index` (additive, unmasked) at `weight` |
| `set_anim_layer_ex(entity_id, index, clip, weight, mode, mask)` | Same, with `"additive"`/`"override"` and a root-joint mask |
| `set_anim_layer_weight(entity_id, index, weight)` | Set a layer's weight instantly (cancels a fade) |
| `fade_anim_layer(entity_id, index, weight, seconds)` | Ramp a layer's weight over `seconds` (engine writes the ramp back each frame) |
| `play_sequence(entity_id, name)` | Play a `*.sequence.toml` (timestamped blend/layer/speed/cue events) on this animator |
| `stop_sequence(entity_id)` | Stop the active sequence |
| `clear_anim_layer(entity_id, index)` | Deactivate a layer (slot kept so indices stay stable) |
| `set_field(id, "ik_two_bone", "target", name)` | Arm two-bone IK on a forearm node (also `pole`, `tip`, `weight`); the engine pins the chain every frame (ADR 0070) |

Weights are floats — write `0.5`, never `0` (Rhai does not coerce ints).

### Coordinate System

Flint uses a **Y-up, right-handed** coordinate system:

- **Forward** = `-Z` (into the screen)
- **Right** = `+X`
- **Up** = `+Y`

Euler angles are stored as `(pitch, yaw, roll)` in **degrees**, applied in ZYX order. Positive yaw rotates counter-clockwise when viewed from above (i.e., turns left).

Use the direction helpers (`forward_from_yaw`, `right_from_yaw`) to convert a yaw angle into a world-space direction vector. These encode the coordinate convention so scripts don't need to compute the trig manually.

### Math API

| Function | Returns | Description |
|----------|---------|-------------|
| `PI()` | `f64` | The constant π (3.14159...) |
| `TAU()` | `f64` | The constant τ = 2π (6.28318...) |
| `deg_to_rad(degrees)` | `f64` | Convert degrees to radians |
| `rad_to_deg(radians)` | `f64` | Convert radians to degrees |
| `forward_from_yaw(yaw_deg)` | `Map` | Forward direction vector `#{x, y, z}` for a given yaw in degrees |
| `right_from_yaw(yaw_deg)` | `Map` | Right direction vector `#{x, y, z}` for a given yaw in degrees |
| `wrap_angle(degrees)` | `f64` | Normalize an angle to `[0, 360)` |
| `clamp(val, min, max)` | `f64` | Clamp a value to a range |
| `lerp(a, b, t)` | `f64` | Linear interpolation between `a` and `b` |
| `random()` | `f64` | Random value in `[0, 1)` |
| `random_range(min, max)` | `f64` | Random value in `[min, max)` |
| `sin(x)` | `f64` | Sine (radians) |
| `cos(x)` | `f64` | Cosine (radians) |
| `abs(x)` | `f64` | Absolute value |
| `sqrt(x)` | `f64` | Square root |
| `floor(x)` | `f64` | Floor |
| `ceil(x)` | `f64` | Ceiling |
| `min(a, b)` | `f64` | Minimum of two values |
| `max(a, b)` | `f64` | Maximum of two values |
| `atan2(y, x)` | `f64` | Two-argument arctangent (radians) |

### Ocean API

Available when the scene has an [`ocean`](ocean.md) component. All coordinates
are world-space; all queries are evaluated on the same clock the renderer used
this frame, so what you sample is what is on screen.

| Function | Returns | Description |
|----------|---------|-------------|
| `ocean_height(x, z)` | `f64` | Eulerian surface height in meters |
| `ocean_velocity_y(x, z)` | `f64` | Vertical surface velocity in m/s (analytic ∂h/∂t) |
| `ocean_normal(x, z)` | `Map` | Surface normal `#{x, y, z}` |

```rhai
// Float a hull on five probe points.
let p = get_field(me, "transform", "position");
let h = ocean_height(p.x, p.z);
```

`ocean_velocity_y` is the impact signal: the *relative* approach speed between
water and hull is what distinguishes a lap from a slam. Without a scene ocean
these return 0 (and `#{0,1,0}`) rather than failing.

A handful of probes per frame is cheap. Thousands are not.

### Input and Cursor API

| Function | Returns | Description |
|----------|---------|-------------|
| `any_input_just_pressed()` | `bool` | True on the frame any key, mouse button, or gamepad button was pressed, or a finger landed |
| `last_input_device()` | `String` | `"keyboard"`, `"mouse"`, `"gamepad"` or `"touch"`: the device of the most recent deliberate input |
| `gamepad_connected()` | `bool` | True while at least one gamepad is attached |
| `set_cursor_captured(captured)` | | Capture (hide + lock) or release the mouse cursor, at the end of this frame |
| `is_cursor_captured()` | `bool` | Whether the cursor is captured now ([Cursor Capture and Input Gates](input-gates.md)) |

`any_input_just_pressed` reads **raw** presses and bypasses action maps
entirely — it is for "press any key to continue", where the whole point is that
you do not care which key.

`last_input_device` is a latch, not a per-frame flag: it changes on a key
press, a mouse *button* press, a gamepad button, a stick pushed past 0.3, or a
touch beginning, and keeps its value until the next such event. Mouse motion
and idle stick drift never flip it, so a HUD that swaps prompt glyphs
(`[E]` vs the `(A)` button) on it does not flicker when the mouse is nudged.
`gamepad_connected` is the companion for "show pad prompts before the first
press"; it starts `false` and follows the host's gamepad backend each frame.

`set_cursor_captured(true)` is how a scene gets mouse-look without a
character-controller player entity. The engine only captures automatically for
scenes that have one, so a fixed-camera or custom-camera scene must ask.

### Event API

| Function | Description |
|----------|-------------|
| `fire_event(name)` | Fire a named game event |
| `fire_event_data(name, data)` | Fire an event with a data map payload |

A fired event is broadcast. Every script that defines `on_event(name, data)`
receives it, including the script that fired it, in entity-id order. `data` is
always a map: `fire_event` sends an empty one. The payload travels as TOML, so
map values must be booleans, integers, floats, strings, arrays or maps; `()`
and other types are dropped. Audio triggers hear the same events.

```rhai
fn on_event(name, data) {
    if name == "door_opened" {
        log(`door ${data.id} opened`);
    }
}
```

**Timing.** Script commands run after `on_update`, so an event a script fires
reaches `on_event` at the start of the next frame, before that frame's
`on_update`. Events from the host arrive the same frame:

- **Player extensions** call `ExtensionCtx::fire_event(name, data)` in
  `pre_update`. `data` is anything that converts into a `toml::Value`, usually
  a `toml::Table`. Scripts see it before this frame's `on_update`.
- **Engine code** pushes a `GameEvent::Custom { name, data }` onto the physics
  event queue, which hands it to scripts with the next batch of collisions
  and input actions.

A payload that isn't a table arrives as `#{ value: data }`. Events are dropped
while the game state pauses scripts, as input actions are.

### Log API

| Function | Description |
|----------|-------------|
| `log(msg)` | Log an info-level message |
| `log_info(msg)` | Alias for `log()` |
| `log_warn(msg)` | Log a warning |
| `log_error(msg)` | Log an error |

### Physics API

Physics functions provide raycasting and camera access for combat, line-of-sight checks, and interaction targeting:

| Function | Returns | Description |
|----------|---------|-------------|
| `raycast(ox, oy, oz, dx, dy, dz, max_dist)` | `Map` or `()` | Cast a ray from origin in direction. Returns hit info or `()` if nothing hit |
| `move_character(id, dx, dy, dz)` | `Map` or `()` | Collision-corrected kinematic movement. Returns `#{x, y, z, grounded}` |
| `get_collider_extents(id)` | `Map` or `()` | Collider shape dimensions (see below) |
| `set_joint_target(id, value)` | --- | Write `joint.motor_target` (degrees for hinge/spherical, metres for prismatic); applied on the next fixed step (ADR 0069) |
| `get_joint_target(id)` | `f64` | Read `joint.motor_target` |
| `get_joint_position(id)` | `f64` or `()` | Simulated joint coordinate from the rest pose: hinge angle in degrees or prismatic displacement in metres |
| `get_camera_position()` | `Map` | Camera world position as `#{x, y, z}` |
| `get_camera_direction()` | `Map` | Camera forward vector as `#{x, y, z}` |
| `set_camera_position(x, y, z)` | --- | Override camera position from script |
| `set_camera_target(x, y, z)` | --- | Override camera look-at target from script |
| `set_camera_fov(fov)` | --- | Override camera field of view (degrees) from script |
| `set_camera_orthographic(enabled)` | --- | Switch the camera between orthographic and perspective projection |
| `set_camera_ortho_height(height)` | --- | Orthographic half-height in world units |
| `set_camera_roll(radians)` | --- | Roll the camera about its view axis (ADR 0022, camera roll override) |

The `raycast()` function automatically excludes the calling entity's collider from results. On a hit, it returns a map with these fields:

| Field | Type | Description |
|-------|------|-------------|
| `entity` | `i64` | Entity ID of the hit object |
| `distance` | `f64` | Distance from origin to hit point |
| `point_x`, `point_y`, `point_z` | `f64` | World-space hit position |
| `normal_x`, `normal_y`, `normal_z` | `f64` | Surface normal at hit point |

`raycast()` meets physics colliders. To meet what's drawn, the runtime meshes a host uploaded as ray targets (see [Rendering: Runtime meshes](rendering.md#runtime-meshes)), use `raycast_meshes` and `screen_to_mesh` (ADR 0105). They need no physics, meet the drawn triangles from either side, and answer with the same fields as `raycast()`, the normal turned back toward the ray's origin:

| Function | Returns | Description |
|----------|---------|-------------|
| `raycast_meshes(ox, oy, oz, dx, dy, dz, max_dist)` | `Map` or `()` | The nearest ray target the ray meets within `max_dist` world units. `(dx, dy, dz)` needn't be unit length; `distance` is in world units |
| `screen_to_mesh(x, y)` | `Map` or `()` | The nearest ray target under a screen point (e.g. the cursor), for picking a wall or a roof |

They meet the targets where the last frame drew them. A camera rig that keeps out of walls casts from the player's head toward where it wants to stand, and comes in to the hit:

```rust
let d = [cx - hx, cy - hy, cz - hz];
let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
let hit = raycast_meshes(hx, hy, hz, d[0], d[1], d[2], len);
let clear = if hit == () { 1.0 } else { hit.distance / len };   // the share of the way that's open
```

**`move_character`** performs collision-corrected kinematic movement using Rapier's shape-sweep. The entity must have `rigidbody` and `collider` components. The returned map contains the corrected position and a `grounded` flag:

```rust
fn on_update() {
    let me = self_entity();
    let dt = delta_time();
    let result = move_character(me, 0.0, -9.81 * dt, 5.0 * dt);
    if result != () {
        set_position(me, result.x, result.y, result.z);
        if result.grounded {
            // Can jump
        }
    }
}
```

**`get_collider_extents`** returns the collider shape dimensions. The returned map varies by shape:

- Box: `#{shape: "box", half_x, half_y, half_z}`
- Capsule: `#{shape: "capsule", radius, half_height}`
- Sphere: `#{shape: "sphere", radius}`

Returns `()` if the entity has no collider.

**Example: Hitscan weapon**

```rust
fn fire_weapon() {
    let cam_pos = get_camera_position();
    let cam_dir = get_camera_direction();
    let hit = raycast(cam_pos.x, cam_pos.y, cam_pos.z,
                      cam_dir.x, cam_dir.y, cam_dir.z, 100.0);
    if hit != () {
        let target = hit.entity;
        if has_component(target, "health") {
            let hp = get_field(target, "health", "current_hp");
            set_field(target, "health", "current_hp", hp - 25);
        }
    }
}
```

#### 2D Physics

For sprite games the physics world is a flat plane. These mirror the 3D calls above:

| Function | Returns | Description |
|----------|---------|-------------|
| `set_velocity_2d(id, vx, vy)` | --- | Set a 2D body's velocity (deferred command, applied after the script batch) |
| `get_velocity_2d(id)` | `Map` or `()` | Current velocity as `#{vx, vy}`, or `()` if the entity has no 2D body |
| `overlap_rect(x, y, w, h)` | `Array` | IDs of every entity whose collider overlaps the rectangle |
| `raycast_2d(ox, oy, dx, dy, max_dist)` | `Map` or `()` | `#{entity, distance, point_x, point_y, normal_x, normal_y}` or `()` |

#### 2D Camera

A follow camera with deadzone and smoothing, plus a stackable shake. All positions are world units on the sprite plane; the camera sits at `z = 10` looking down `-z` so every sprite layer stays in front of it.

| Function | Returns | Description |
|----------|---------|-------------|
| `camera_follow(id, offset_x, offset_y, speed, deadzone_w, deadzone_h)` | --- | Track an entity each frame: the camera only moves when the target leaves the deadzone rectangle, then eases toward it with frame-rate-independent smoothing at `speed`. Applies any active shake |
| `camera_follow_position()` | `Map` | Current smoothed follow position as `#{x, y}` |
| `camera_follow_set(x, y)` | --- | Teleport the follow position (use on scene enter or respawn to avoid a long ease) |
| `camera_shake(amplitude, frequency, decay)` | --- | Start or stack a shake. Amplitude takes the max of current and new; frequency in Hz; amplitude decays exponentially at `decay` per second |
| `camera_shake_stop()` | --- | Cancel the shake immediately |
| `camera_apply_shake()` | --- | Advance and apply the shake to a camera you position yourself with `set_camera_position` (not needed when `camera_follow` is in use) |

#### Chunks

Large 2D worlds stream in `.chunk.toml` files at runtime:

| Function | Returns | Description |
|----------|---------|-------------|
| `load_chunk(path, offset_x, offset_y, chunk_id)` | --- | Load a chunk file, translating its entities by the offset, under a name you choose |
| `unload_chunk(chunk_id)` | --- | Despawn every entity that chunk loaded |
| `is_chunk_loaded(chunk_id)` | `bool` | Whether that chunk is currently resident |

### Spline API

Query spline entities for path-following, track layouts, and procedural placement:

| Function | Returns | Description |
|----------|---------|-------------|
| `spline_closest_point(spline_id, x, y, z)` | `Map` or `()` | Nearest point on spline to query position. Returns `#{t, x, y, z, dist_sq}` |
| `spline_sample_at(spline_id, t)` | `Map` or `()` | Sample spline at parameter `t` (0.0--1.0). Returns `#{x, y, z, fwd_x, fwd_y, fwd_z, right_x, right_y, right_z}` |
| `spline_is_gap(spline_id, t)` | `bool` | Whether `t` falls inside one of the spline's authored gaps (`gap_starts` / `gap_ends` in `spline_data`; ranges may wrap past 1.0) |
| `spline_gap_at(spline_id, t)` | `Map` or `()` | The gap containing `t` as `#{start_t, end_t}`, or `()` if `t` is on solid track |

The `t` parameter wraps for closed splines. The returned forward and right vectors are normalized and can be used for orientation.

### Particle API

| Function | Description |
|----------|-------------|
| `emit_burst(entity_id, count)` | Fire N particles immediately |
| `start_emitter(entity_id)` | Start continuous emission |
| `stop_emitter(entity_id)` | Stop emission (existing particles finish their lifetime) |
| `set_emission_rate(entity_id, rate)` | Change emission rate dynamically |
| `play_effect(name, x, y, z)` | Spawn a detached one-shot instance of a `particles/<name>.particles.toml` effect at a point; returns a handle |
| `stop_effect(handle)` | Stop a detached effect's emission; particles in flight finish |
| `set_effect_param(handle, param, value)` | Tune a detached effect: `emission_scale`, `scale`, `playing` (> 0.5), or `x` / `y` / `z` to move it |

See [Particles](particles.md) for full component schema and recipes.

### Post-Processing API

Control the HDR post-processing pipeline at runtime from scripts:

| Function | Description |
|----------|-------------|
| `set_vignette(intensity)` | Set vignette intensity (0.0 = none, 1.0 = heavy) |
| `set_bloom_intensity(intensity)` | Set bloom strength (0.0 = none) |
| `set_exposure(value)` | Set exposure multiplier (1.0 = default) |
| `set_chromatic_aberration(amount)` | Set chromatic aberration strength |
| `set_radial_blur(amount)` | Set radial blur strength |
| `set_ssao_intensity(value)` | Set SSAO intensity |
| `set_fog_density(value)` | Set fog density |
| `set_fog_color(r, g, b)` | Set fog color (linear 0--1) |
| `set_render_mode(mode, mix)` | Stylized render mode (see below) |
| `set_render_mode_params(x, y, z, w)` | Per-mode tuning parameters |
| `set_desaturation(amount)` | Desaturate toward ash grey (0 = full colour, 1 = grey; ADR 0021) |
| `set_dof(strength)` | Depth-of-field defocus strength (0 = sharp, 1 = full blur) |
| `set_dof_focus(distance, range)` | Focus plane distance and half-width, in view metres |

These overrides are applied each frame and combine with the scene's `[post_process]` baseline settings. Useful for dynamic effects like speed vignetting, boost bloom, or exposure flashes.

**All of these are sticky except `set_render_mode`.** Set an override once and
it persists until you change it. The render mode is the deliberate exception:
it is cleared the frame your script stops calling it, so a crashed or
hot-reloaded script cannot strand the world inside an effect. Call it every
frame the effect is active. See
[Post-Processing: Render Modes](post-processing.md#render-modes).

### Conducted Parameters API

When a scene carries a [`music_session`](music-sessions.md) component, the player fills a per-frame snapshot of the session's state and exposes it to every script through these getters (ADR 0020, conducted-parameters script surface). Without a session every getter returns a neutral value: a clean, settled world with `coherence` and `reassembly` at `1.0`, zero lean, and `1e6` beats until anything upcoming. Bindings written as `1 - conducted_coherence()` therefore show nothing when no music is running, and scripts never need to test for a session.

| Function | Returns | Description |
|----------|---------|-------------|
| `conducted_lean()` | `#{x, y}` | The player's lean, both axes in `[-1, 1]` |
| `conducted_target()` | `#{x, y}` | The chart's current lean target |
| `conducted_next_target()` | `#{x, y, beats}` | The next authored lean key and suite beats until its anchor (ADR 0023). `beats = 1e6` and `x`/`y` = the current target when nothing is upcoming |
| `conducted_next_pulse()` | `#{beats, open}` | Suite beats until the next judgment window's anchor, and whether that window is open right now (a press would land) |
| `conducted_sway()` | `#{x, y}` | Right-stick sway (zeros under the prototype input map) |
| `conducted_pressure_l()` / `conducted_pressure_r()` | `f64` | Trigger depths in `[0, 1]` (zeros under the prototype map) |
| `conducted_coherence()` | `f64` | The coherence integrator, `1.0` = fully in step |
| `conducted_beat()` | `f64` | Suite beats from zero, accumulated across tempo changes |
| `conducted_beat_phase()` / `conducted_bar_phase()` | `f64` | `0..1` within the current beat / bar |
| `conducted_bar()` | `i64` | Current bar number |
| `conducted_section()` | `String` | Current section name, `""` when none |
| `conducted_pulses()` | `Array` | Pulses judged this frame, each `#{age, err_ms, kind}` with `kind` one of `"hit"`, `"miss"`, `"spurious"`. Empty most frames |
| `conducted_cues()` | `Array` | Chart cues fired this frame, each `#{name, age, params}` (ADR 0033). `params` is the cue's flat table as floats, strings and bools; nested tables are dropped |
| `conducted_desaturate()` / `conducted_blur()` / `conducted_chromatic()` | `f64` | Ladder visual ramps, `0` = clean |
| `conducted_reassembly()` | `f64` | `1` in normal play, `0` rising to `1` while re-gathering after a full fail |
| `conducted_rewind()` | `f64` | Rewind-interlude progress, `0` = not rewinding |
| `conducted_no_input()` | `bool` | The session has seen no input since the bar-2 check |
| `conducted_preroll()` | `bool` | Still in the count-in; the world should stay untouched |

All scalars are `f64`, so compare and multiply with float literals. A typical binding reads the snapshot in `on_update` and writes a post-process override:

```rhai
fn on_update() {
    let grey = conducted_desaturate();
    set_desaturation(grey);
    let p = conducted_next_pulse();
    if p.open { set_vignette(0.4); } else { set_vignette(0.2); }
}
```

### Audio Filter API

| Function | Description |
|----------|-------------|
| `set_audio_lowpass(cutoff_hz)` | Set master bus low-pass filter cutoff frequency (Hz) |

The low-pass filter affects all audio output. Pass `20000.0` for no filtering, lower values for a muffled effect. Useful for speed-dependent audio (e.g., wind rush at high speed) or dramatic transitions.

### Scene Transition API

Load new scenes, manage game state, and persist data across transitions:

| Function | Returns | Description |
|----------|---------|-------------|
| `load_scene(path)` | --- | Begin transition to a new scene |
| `reload_scene()` | --- | Reload the current scene |
| `current_scene()` | `String` | Path of the current scene |
| `transition_progress()` | `f64` | Progress of the current transition (0.0--1.0) |
| `transition_phase()` | `String` | Current transition phase (`"idle"`, `"exiting"`, `"loading"`, `"entering"`) |
| `is_transitioning()` | `bool` | Whether a scene transition is in progress |
| `complete_transition()` | --- | Advance to the next transition phase |

Scene transitions follow a lifecycle: Idle -> Exiting -> Loading -> Entering -> Idle. During the Exiting and Entering phases, `on_draw_ui()` still runs so scripts can draw fade effects using `transition_progress()`. Call `complete_transition()` to advance phases --- this gives scripts full control over transition timing and visuals.

Two additional callbacks fire during transitions:

| Callback | Signature | When It Fires |
|----------|-----------|---------------|
| `on_scene_enter` | `fn on_scene_enter()` | After a new scene is loaded and ready |
| `on_scene_exit` | `fn on_scene_exit()` | Before the current scene is unloaded |

### Game State Machine API

A pushdown automaton for managing game states (playing, paused, custom):

| Function | Returns | Description |
|----------|---------|-------------|
| `push_state(name)` | --- | Push a named state onto the stack |
| `pop_state()` | --- | Pop the top state (returns to previous) |
| `replace_state(name)` | --- | Replace the top state |
| `current_state()` | `String` | Name of the current (top) state |
| `state_stack()` | `Array` | All state names from bottom to top |
| `register_state(name, config)` | --- | Register a custom state template |

Built-in state templates:
- **`"playing"`** --- all systems run (default)
- **`"paused"`** --- physics, scripts, animation, particles, and audio are paused; rendering runs; `on_draw_ui()` still fires (for pause menus)
- **`"loading"`** --- all systems paused

### Persistent Data API

Key-value store that survives scene transitions:

| Function | Returns | Description |
|----------|---------|-------------|
| `persist_set(key, value)` | --- | Store a value |
| `persist_get(key)` | `Dynamic` | Retrieve a value (or `()` if not set) |
| `persist_has(key)` | `bool` | Check if a key exists |
| `persist_remove(key)` | --- | Remove a key |
| `persist_clear()` | --- | Clear all persistent data |
| `persist_keys()` | `Array` | List all keys |
| `persist_save()` | --- | Write the engine-managed save file now |
| `persist_save(path)` | --- | Save store to an explicit TOML file |
| `persist_load(path)` | --- | Load store from an explicit TOML file |

#### Persistence

The store is also kept on disk without any script involvement. The player
reads `<project>/save/persist.toml` at startup (the project root is the scene
directory's parent, the same rule as `fonts/` and `sprites/`; on Android it is
the app's internal files directory) and, if the file exists, every
`persist_get` sees last session's values from the first `on_init` onward. A
missing file is a fresh, empty store --- nothing is created until something is
stored.

It is written back 1 s after the last `persist_set` / `persist_remove` /
`persist_clear` (a script writing every frame still produces one write per
second), on every scene transition (after the outgoing scene's
`on_scene_exit`), and on exit. `persist_save()` with no arguments forces the
write immediately --- call it after a settings screen commits, so a crash a
moment later loses nothing. The `save/` directory is created on demand; add it
to the game's `.gitignore`. Loads and saves log at `info`, failures at `warn`.

A game's Rust code shares the same store: a player extension reaches it as
`ExtensionCtx::persist` (see [Player Extensions](../guides/player-extensions.md#saved-data)),
and its writes are saved the same way.

### Data Files API

Read-only game data tables (trick dictionaries, item stats, tuning) kept as
TOML under the project root:

| Function | Returns | Description |
|----------|---------|-------------|
| `load_data(path)` | `Map` or `()` | Parse a TOML file relative to the project root (the scene directory's parent) and return the whole document as a map; parsed once per path and cached for the session |
| `load_data(path, reload)` | `Map` or `()` | Same, but `reload = true` drops the cached parse first (hot iteration) |

Types are preserved exactly: a TOML integer arrives as `INT`, a float as
`FLOAT` (Rhai does not coerce between them, so write `points = 500.0` or call
`to_float()` before multiplying by a float). Tables come back as maps with
keys in alphabetical order; use an array of tables (`[[entries]]`) when file
order matters. A missing or malformed file returns `()` and logs a warning.
Call it once in `on_init` and keep the map --- every call clones the tree.

### Data-Driven UI API

Load and manipulate TOML-defined UI documents at runtime:

| Function | Returns | Description |
|----------|---------|-------------|
| `load_ui(path)` | `i64` | Load a UI document (`.ui.toml`). Returns a handle |
| `unload_ui(handle)` | --- | Unload a UI document |
| `ui_set_text(element_id, text)` | --- | Set the text content of a UI element |
| `ui_show(element_id)` | --- | Show a hidden UI element |
| `ui_hide(element_id)` | --- | Hide a UI element |
| `ui_set_visible(element_id, visible)` | --- | Set element visibility |
| `ui_set_color(element_id, r, g, b, a)` | --- | Set element text/foreground color |
| `ui_set_bg_color(element_id, r, g, b, a)` | --- | Set element background color |
| `ui_set_style(element_id, property, value)` | --- | Override a single style property |
| `ui_set_style_array(id, prop, array)` | --- | Override an array-valued property at runtime: `color`, `bg_color`, `stroke_color`, `padding` (4 numbers) or `shadow` (6 numbers) |
| `ui_reset_style(element_id)` | --- | Remove all style overrides |
| `ui_set_class(element_id, class_name)` | --- | Change an element's style class |
| `ui_exists(element_id)` | `bool` | Check if a UI element exists |
| `ui_get_rect(element_id)` | `Map` | Get resolved position/size as `#{x, y, width, height}` |

Older scenes place HUD elements as entities with `screen_anchor`, `ui_text` and `ui_fill` components instead of a UI document. Those are driven with entity-level setters:

| Function | Description |
|----------|-------------|
| `set_text(entity_id, text)` | Write `ui_text.text` |
| `set_text_color(entity_id, r, g, b, a)` | Write `ui_text.color` |
| `ui_set_value(entity_id, value)` | Write `ui_fill.value` (a 0--1 bar fill) |
| `set_anchor(entity_id, anchor)` | Write `screen_anchor.anchor` (`"top-left"` through `"bottom-right"`) |
| `set_anchor_offset(entity_id, x, y)` | Write `screen_anchor.offset_x` / `offset_y` |

UI documents are defined with paired `.ui.toml` (layout) and `.style.toml` (styling) files, following an HTML/CSS/JS-like separation of concerns. See [File Formats](../formats/overview.md) for the format specification.

### UI Draw API

The draw API lets scripts render 2D overlays each frame via the `on_draw_ui()` callback. Draw commands are issued in screen-space coordinates (logical points, not physical pixels) and rendered by the engine through egui.

#### Draw Primitives

| Function | Description |
|----------|-------------|
| `draw_text(x, y, text, size, r, g, b, a)` | Draw text at position |
| `draw_text_ex(x, y, text, size, r, g, b, a, layer)` | Draw text with explicit layer |
| `draw_text_stroked(x, y, text, size, r, g, b, a, stroke_r, stroke_g, stroke_b, stroke_a, stroke_width)` | Text with an outline stroke behind it |
| `draw_text_opts(x, y, text, size, opts)` | Text with an options map: `font`, `color`, `align`, `stroke`, `spacing`, `layer`, `shadow` (see below) |
| `draw_rect(x, y, w, h, r, g, b, a)` | Draw filled rectangle |
| `draw_rect_ex(x, y, w, h, r, g, b, a, rounding, layer)` | Filled rectangle with corner rounding and layer |
| `draw_rect_outline(x, y, w, h, r, g, b, a, thickness)` | Rectangle outline |
| `draw_rect_outline_ex(x, y, w, h, r, g, b, a, thickness, rounding, layer)` | Rectangle outline with corner rounding and layer |
| `draw_rect_ex4(x, y, w, h, r, g, b, a, tl, tr, br, bl, layer)` | Filled rectangle with per-corner rounding (top-left, top-right, bottom-right, bottom-left) |
| `draw_rect_gradient(x, y, w, h, r1, g1, b1, a1, r2, g2, b2, a2, vertical, layer)` | Two-colour linear gradient fill: colour 1 to colour 2, top to bottom when `vertical` is `true`, left to right otherwise |
| `draw_text_stroked_ex(x, y, text, size, r, g, b, a, sr, sg, sb, sa, stroke_width, layer)` | Stroked text with explicit layer |
| `draw_circle(x, y, radius, r, g, b, a)` | Draw filled circle |
| `draw_circle_ex(x, y, radius, r, g, b, a, layer)` | Filled circle with explicit layer |
| `draw_circle_outline(x, y, radius, r, g, b, a, thickness)` | Circle outline |
| `draw_circle_outline_ex(x, y, radius, r, g, b, a, thickness, layer)` | Circle outline with explicit layer |
| `draw_line(x1, y1, x2, y2, r, g, b, a, thickness)` | Draw a line segment |
| `draw_line_ex(x1, y1, x2, y2, r, g, b, a, thickness, layer)` | Line segment with explicit layer |
| `draw_line_3d(x1, y1, z1, x2, y2, z2, r, g, b, a, thickness)` | World-space line projected with the frame's camera (near-plane clipped) |
| `draw_line_3d_ex(x1, y1, z1, x2, y2, z2, r, g, b, a, thickness, layer)` | Same, with explicit layer |
| `draw_world_line(x1, y1, z1, x2, y2, z2, r, g, b, a, thickness)` | World-space line drawn in the 3D frame with a depth test: geometry in front of it hides it (see below) |
| `draw_world_text(x, y, z, text, size, opts)` | Text at a world point, drawn with a depth test: a billboard, or painted on a plane (see below) |
| `draw_ring(cx, cy, r_inner, r_outer, start_deg, end_deg, r, g, b, a, layer)` | Annular sector (`r_inner` = 0 gives a pie slice); see angle convention below |
| `draw_arc(cx, cy, radius, start_deg, end_deg, r, g, b, a, thickness, layer)` | Stroked arc: a ring centred on `radius`, `thickness` wide |
| `draw_polygon(points, r, g, b, a, layer)` | Convex filled polygon; `points` is `[[x, y], ...]` or a flat `[x0, y0, x1, y1, ...]` list (ints or floats) |
| `draw_polygon_outline(points, r, g, b, a, thickness, closed, layer)` | Polyline through `points`, closed back to the start when `closed` is `true` |
| `draw_sprite(x, y, w, h, name)` | Draw a sprite image |
| `draw_sprite_ex(x, y, w, h, name, u0, v0, u1, v1, r, g, b, a, layer)` | Sprite with custom UV coordinates, tint, and layer |

#### Query Functions

| Function | Returns | Description |
|----------|---------|-------------|
| `screen_width()` | `f64` | Logical screen width in points |
| `screen_height()` | `f64` | Logical screen height in points |
| `measure_text(text, size)` | `Map` | Laid-out size of `text` in the default font as `#{width, height}` |
| `measure_text_ex(text, size, font, spacing)` | `Map` | Same, for a named font family and extra letter spacing (`""` = default font) |
| `find_nearest_interactable()` | `Map` or `()` | Nearest interactable entity info, or `()` if none in range |
| `world_to_screen(x, y, z)` | `Map` | A world point in logical points as `#{x, y, depth, visible}`; `visible` is false behind the camera |
| `screen_to_ground(x, y, plane_y)` | `[x, z]` or `()` | The point on the plane `y = plane_y` under a screen point, or `()` where the view doesn't reach it |
| `screen_to_mesh(x, y)` | `Map` or `()` | The drawn ray target under a screen point, as `raycast_meshes` answers (see [Physics API](#physics-api)) |
| `screen_to_heightfield(x, y)` | `[x, y, z]` or `()` | The game's ground under a screen point, where the ray first comes down onto the ground field the host registered, or `()` with none registered or where the ray misses it |
| `ground_height(x, z)` | `f64` or `()` | World y of the registered ground field at (x, z), or `()` with none registered or off its edge |

#### Lines in the world

`draw_line_3d` projects onto the HUD, so it shows in front of everything, including the model a ring is drawn round. `draw_world_line` draws in the 3D frame instead. It goes over the finished image, before the HUD, and it's hidden wherever the scene's depth is nearer. A ring on the ground round a statue therefore passes behind the statue's far side. The arguments and units are `draw_line_3d`'s: world coordinates, a display-space colour, and a width in logical points. The edges are anti-aliased. World lines don't write depth, so they never hide each other. Fog, bloom and tonemapping don't touch them. They're projected with the camera the frame renders with, from `on_update` or `on_draw_ui` alike. There is no layer, and there is no clip rect.

Each end is pulled toward the camera by 0.2% of its distance, so a line laid exactly on a surface doesn't flicker against it. A line meant to lie on a raised surface, such as a road or a pavement, should still be drawn at or just above that surface. Draw a ring as a polyline:

```rust
fn ring(cx, cz, y, r, steps) {
    for i in 0..steps {
        let a0 = i.to_float() / steps.to_float() * 2.0 * PI();
        let a1 = (i + 1).to_float() / steps.to_float() * 2.0 * PI();
        draw_world_line(cx + r * a0.cos(), y, cz + r * a0.sin(),
                        cx + r * a1.cos(), y, cz + r * a1.sin(),
                        1.0, 0.8, 0.2, 0.7, 2.0);
    }
}
```

Rust hosts can set the same lines on the renderer directly with `SceneRenderer::set_world_lines(lines, pixels_per_point)`.

#### Text in the world

`draw_text_opts` at a `world_to_screen` point floats over everything, so a name on a sign shows through the roof in front of it. `draw_world_text(x, y, z, text, size, opts)` writes text in the world instead. Like a world line, it's drawn over the finished image, before the HUD, and hidden wherever the scene's depth is nearer, glyph by glyph: a name half behind a post is half drawn. It doesn't write depth, and fog, bloom and tonemapping don't touch it. It's laid out in the HUD's fonts and colours, so it looks like the same text drawn with `draw_text_opts`.

`opts` takes `draw_text_opts`'s keys: `font`, `color`, `align`, `stroke`, `spacing` and `shadow`. There's no `layer`, and no clip rect. The text is centred on the point unless `align` (`"left"`, `"center"`, `"right"`) or `valign` (`"top"`, `"middle"`, `"bottom"`) say otherwise.

Without `facing`, the text is a billboard: it faces the screen at `size` logical points wherever the point is, like a name tag, and takes the point's depth. With `facing: [nx, ny, nz]` it lies in the plane through the point with that normal, as if painted there, and reads from that side. It isn't drawn from behind, so paint a two-sided sign twice, once on each face. `up` (default `[0, 1, 0]`) says which way the letters stand; only its part across the plane counts. On a plane every length is in world units: `size` is the em, and `spacing`, the stroke's width and the shadow's offset are world units too. The letters are rasterised at the size the em spans on screen, in whole pixels from 4 to 128, so they stay sharp as the camera comes close.

```rust
// A shop's name on both faces of its sign, fitted to a 0.5 m board.
fn sign(x, y, z, fx, fz, name) {
    let m = measure_text_ex(name, 20.0, "display", 0.0);
    let size = (0.5 * 20.0 / m.width).min(0.13);
    for side in [1.0, -1.0] {
        draw_world_text(x + fx * side * 0.03, y, z + fz * side * 0.03, name, size,
            #{ font: "display", color: [0.98, 0.93, 0.80, 1.0], facing: [fx * side, 0.0, fz * side] });
    }
}
```

The point is pulled toward the camera by the world lines' 0.2% (`DEPTH_SLACK`), so text painted on a surface doesn't fight it. A plane's text that reaches behind the camera isn't drawn at all, and nor is any text whose point is behind it or past the far plane. Scripts decide when text is too small to read: there's no minimum size.

`find_nearest_interactable()` returns a map with `entity` (ID), `prompt_text`, `interaction_type`, and `distance` fields when an interactable entity is within range.

`world_to_screen` and `screen_to_ground` are inverses. In `on_draw_ui` they use the camera the frame renders with; in `on_update`, which runs before script camera overrides apply, they use the previous frame's. `world_to_screen` of a ground hit lands back on the screen point it came from. Native functions a game registers get the same answers from `Projector` (see [Player extensions](../guides/player-extensions.md#projecting-from-native-functions)). `screen_to_ground` takes logical points, the `mouse_x()` / `mouse_y()` space, so it is correct at any display scale. It works for perspective and orthographic cameras, including one looking straight down. It returns `()` over the sky, when an orthographic camera looks level, or when the plane is behind the camera. Picking the ground under the cursor:

```rhai
let g = screen_to_ground(mouse_x(), mouse_y(), 0.0);
if g != () && is_mouse_just_pressed(0) {
    set_position(marker, g[0], 0.0, g[1]);   // exactly under the cursor
}
```

On ground that isn't flat, a plane cuts the view at the wrong height: on a hill, the click lands past or short of the spot under the cursor. A game that registers its ground with `Projector::set_ground` (see [Player extensions](../guides/player-extensions.md#picking-a-ground-that-isnt-flat)) gets `screen_to_heightfield(x, y)`, the first point where the ray through the screen point comes down onto that ground, as `[x, y, z]`, and `ground_height(x, z)`. Both return `()` where there's no answer, so fall back to the plane there:

```rhai
let g = screen_to_heightfield(mouse_x(), mouse_y());
if g == () {
    let p = screen_to_ground(mouse_x(), mouse_y(), 0.0);
    if p != () { g = [p[0], 0.0, p[1]]; }
}
```

Both `measure_text` calls lay the string out with the same font stack the HUD draws with, so a measured width can be used to centre or right-pad text exactly. (Before the first frame has rendered they return a glyph-count estimate.)

#### Text Options and Fonts

`draw_text_opts` takes a map whose keys are all optional:

```rhai
draw_text_opts(40.0, 40.0, "128", 96.0, #{
    font: "BarlowCondensed-BlackItalic",   // family name; unknown → default font (warned once)
    color: [1.0, 0.85, 0.2, 1.0],          // default white
    align: "center",                       // "left" | "center" | "right"
    stroke: [0.0, 0.0, 0.0, 1.0, 2.0],     // r, g, b, a, width
    spacing: 1.5,                          // extra letter spacing in points
    layer: 6,                              // integer
    shadow: [3.0, 3.0, 0.0, 0.0, 0.0, 0.7] // dx, dy, r, g, b, a
});
```

Array entries may mix ints and floats. The shadow is drawn once, offset by `(dx, dy)`, beneath the stroke and the text.

**Project fonts.** Drop `.ttf` / `.otf` files in `<project>/fonts/` (found the same way as `sprites/`: the scene file's directory, then its parent). Each file registers a font family named after its file stem, so `fonts/BarlowCondensed-Bold.ttf` is usable as `font: "BarlowCondensed-Bold"`. The folder is scanned non-recursively once at startup and again on scene change when the project root changes. An optional `fonts/fonts.toml` adds aliases that point at the same files:

```toml
[[font]]
name = "display"                      # use as font = "display"
file = "BarlowCondensed-BlackItalic.ttf"
```

Each named family keeps egui's built-in fonts behind it, for glyphs the file lacks. The player logs the loaded families at startup (`RUST_LOG=info`). Text without a `font` — and the data-driven UI's default — uses egui's built-in proportional font, unless the manifest names another (ADR 0081):

```toml
[egui]
proportional = "body"                 # a file stem or an alias
monospace = "code"
```

That face then leads egui's default family, for the script HUD's unnamed text and for a game's own egui panels alike ([Player Extensions](../guides/player-extensions.md#fonts)).

#### Rings and Arcs

`draw_ring` / `draw_arc` angles are **degrees from 12 o'clock, clockwise on screen**: `0` is up, `90` is right, `180` is down, `270` is left. `end_deg` must exceed `start_deg`; a sweep larger than 360 is clamped to a full circle. A radial progress dial that fills clockwise from the top is therefore `draw_ring(cx, cy, 40.0, 50.0, 0.0, 360.0 * progress, ...)`. Sectors are tessellated at roughly one segment per 3 degrees (6 to 180 segments).

#### Clipping

| Function | Description |
|----------|-------------|
| `push_clip(x, y, w, h)` | Confine every following draw call to this rectangle |
| `pop_clip()` | Restore the previous clip rectangle |

Clips nest: an inner `push_clip` is intersected with the enclosing one. Each draw command remembers the clip that was active when it was issued, so clipping composes with `layer` ordering (a clipped layer-5 command still draws after unclipped layer-0 ones). Unbalanced `pop_clip` calls are ignored, and the stack is reset at the start of every frame, so a script that forgets to pop cannot clip the next frame. Typical use is a scrolling list or a marquee:

```rhai
push_clip(list_x, list_y, list_w, list_h);
for i in 0..items.len() {
    draw_text(list_x, list_y + i * 24.0 - scroll, items[i], 18.0, 1.0, 1.0, 1.0, 1.0);
}
pop_clip();
```

#### Over the UI

| Function | Description |
|----------|-------------|
| `push_over_ui()` | Draw every following call above the game's egui panels and windows |
| `pop_over_ui()` | Go back to drawing under them |

The script HUD normally draws under a game's own egui ([Player Extensions](../guides/player-extensions.md)), so a panel covers any HUD drawing behind it. Calls issued between `push_over_ui()` and `pop_over_ui()` go on a layer of their own, painted after every egui panel and window but before popups, debug panels and tooltips. Among themselves they still sort by `layer`, and each can be clipped. Pushes nest, unbalanced pops are ignored, and the depth is reset every frame. Use it for short-lived effects that have to reach the UI, such as an item flying into an inventory button, and keep everything else under it, so the HUD never covers what a panel says:

```rhai
push_over_ui();
draw_circle(pip_x, pip_y, 5.0, 0.79, 0.64, 0.15, 1.0);  // lands on the button
pop_over_ui();
```

#### Shared Modules (`scripts/lib`)

Scripts can `import` shared Rhai modules from `<project>/scripts/lib/` (the `lib/` folder inside whichever `scripts/` directory the scene resolved). `import "ui_kit" as ui;` loads `scripts/lib/ui_kit.rhai`; nested paths work too (`import "fx/easing" as ease;`). Every top-level `fn` in a module is exported automatically (mark helpers `private` to hide them); top-level variables must be `export`ed:

```rhai
// scripts/lib/ui_kit.rhai
export let PANEL_ALPHA = 0.85;
fn ease_out_cubic(t) { 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t) }
private fn helper() { }
```

```rhai
// scripts/hud.rhai
import "ui_kit" as ui;

fn on_draw_ui() {
    let a = ui::ease_out_cubic(delta_time());
    draw_rect(0.0, 0.0, 100.0, 20.0, 0.0, 0.0, 0.0, ui::PANEL_ALPHA);
}
```

Put the `import` at the top level of the script: the engine evaluates a script's global statements before each callback, so a global import is visible inside `on_update`, `on_draw_ui` and any function you define. Modules are compiled once and cached. Editing, adding or removing any `.rhai` file under `scripts/lib/` clears that cache and hot-reloads every script in the scene (script state is preserved as with any hot-reload), so a change to a shared helper shows up everywhere at once.

#### Layer Ordering

All `_ex` draw variants accept a `layer` parameter which must be an **integer** (`0`, `1`, `-1`), not a float. Using `0.0` instead of `0` will cause a "Function not found" error because Rhai does not implicitly convert between `float` and `int`. Commands are sorted by layer before rendering:

- **Negative layers** render behind (background elements)
- **Layer 0** is the default
- **Positive layers** render in front (foreground elements)

#### Coordinate System

Coordinates are in **egui logical points**, not physical pixels. On high-DPI displays, logical points differ from pixels by the scale factor. Use `screen_width()` and `screen_height()` for layout calculations --- they return the correct logical dimensions.

#### Sprite Loading

Sprite names map to image files in the `sprites/` directory (without extension). Supported formats: PNG, JPG, BMP, TGA. Textures are lazy-loaded on first use and cached for subsequent frames.

### Sprite API

Runtime control of the `sprite` component on 2D entities (see [2D Sprites](sprites-2d.md) for the component and clip formats):

| Function | Description |
|----------|-------------|
| `set_sprite_source_rect(entity_id, x, y, w, h)` | Source rectangle in texture pixels (manual atlas frames) |
| `set_sprite_flip(entity_id, flip_x, flip_y)` | Mirror horizontally / vertically |
| `set_sprite_tint(entity_id, r, g, b, a)` | Multiply colour |
| `set_sprite_visible(entity_id, visible)` | Show or hide without removing the component |
| `set_sprite_layer(entity_id, layer)` / `get_sprite_layer(entity_id)` | Draw-order layer (integer; higher draws in front) |

### Data-Driven UI System

For structured interfaces like menus, HUDs, and dialog boxes, Flint provides a data-driven UI system that separates layout, style, and logic into distinct files. The procedural `draw_*` API above continues to work alongside it for dynamic elements like minimaps or particle trails.

The pattern is:
- **Layout** (`.ui.toml`) --- element tree with types, hierarchy, anchoring, and default text/images
- **Style** (`.style.toml`) --- named style classes with visual properties (colors, sizes, fonts, padding)
- **Logic** (`.rhai`) --- scripts load UI documents and manipulate elements at runtime

#### File Format: `.ui.toml`

```toml
[ui]
name = "Race HUD"
style = "ui/race_hud.style.toml"   # Path to companion style file

[elements.speed_panel]
type = "panel"
anchor = "bottom-center"
class = "hud-panel"

[elements.speed_label]
type = "text"
parent = "speed_panel"
class = "speed-text"
text = "0"

[elements.lap_counter]
type = "text"
anchor = "top-right"
class = "lap-text"
text = "Lap 1/3"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | string | `"panel"` | Element type: `panel`, `text`, `rect`, `circle`, `image` |
| `anchor` | string | `"top-left"` | Screen anchor for root elements (see below) |
| `parent` | string | --- | Parent element ID (child inherits position from parent) |
| `class` | string | `""` | Style class name from the companion `.style.toml` |
| `text` | string | `""` | Default text content (for `text` elements) |
| `src` | string | `""` | Image source path (for `image` elements) |
| `visible` | bool | `true` | Initial visibility |

**Anchor points:** `top-left`, `top-center`, `top-right`, `center-left`, `center`, `center-right`, `bottom-left`, `bottom-center`, `bottom-right`

#### File Format: `.style.toml`

```toml
[styles.hud-panel]
width = 200
height = 60
bg_color = [0.0, 0.0, 0.0, 0.6]
rounding = 8
padding = [12, 8, 12, 8]
layout = "stack"

[styles.speed-text]
font_size = 32
color = [1.0, 1.0, 1.0, 1.0]
text_align = "center"
width_pct = 100

[styles.lap-text]
font_size = 24
color = [1.0, 0.85, 0.2, 1.0]
width = 120
height = 30
x = -10
y = 10
```

**Style properties:**

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| `x`, `y` | float | `0` | Offset from anchor point or parent |
| `width`, `height` | float | `0` | Fixed dimensions in logical points |
| `width_pct`, `height_pct` | float | --- | Percentage of parent width/height (0--100) |
| `height_auto` | bool | `false` | Auto-size height from children extent |
| `color` | [r,g,b,a] | `[1,1,1,1]` | Primary color (text color, shape fill) |
| `bg_color` | [r,g,b,a] | `[0,0,0,0]` | Background color (panels) |
| `font_size` | float | `16` | Text font size |
| `font` | string | --- | Font family from `<project>/fonts/` (file stem or `fonts.toml` alias); unset = default font |
| `letter_spacing` | float | `0` | Extra spacing between glyphs, in points |
| `shadow` | [dx,dy,r,g,b,a] | --- | Drop shadow offset and colour, drawn beneath the text |
| `text_align` | string | `"left"` | Text alignment: `left`, `center`, `right` |
| `rounding` | float | `0` | Corner rounding for panels/rects |
| `opacity` | float | `1.0` | Element opacity multiplier |
| `thickness` | float | `1` | Stroke thickness for outlines |
| `radius` | float | `0` | Circle radius |
| `layer` | int | `0` | Render layer (negative = behind, positive = in front) |
| `padding` | [l,t,r,b] | `[0,0,0,0]` | Interior padding (left, top, right, bottom) |
| `uv` | [u0,v0,u1,v1] | `[0,0,1,1]` | Sub-rectangle of an `image` element's texture in 0--1 space (sprite-sheet cells, atlases) |
| `layout` | string | `"stack"` | Child flow: `stack` (vertical) or `horizontal` |
| `margin_bottom` | float | `0` | Space below element in flow layout |

Colour arrays may be `[r, g, b]` (alpha 1) or `[r, g, b, a]`.

#### Style Tokens

Any property value that is a string starting with `$` is a **token reference**, resolved when the style file is parsed. Tokens come from two places:

- a local `[tokens]` table in the style file itself
- a shared token file (the game's theme), named at the top level with `tokens = "ui/theme.toml"`

```toml
# ui/main_menu.style.toml
tokens = "ui/theme.toml"          # shared theme, resolved from the project root

[styles.select-header]
font_size = 42
color = "$accent"                 # searched: local [tokens], then every section of the theme
font = "$font.label"              # section-qualified: [font] label = "Barlow-SemiBold"
rounding = "$shape.radius"
```

The shared file is plain TOML sections --- `[color]`, `[font]`, `[type]`, `[shape]`, `[space]`, `[motion]`, whatever the game defines:

```toml
# ui/theme.toml
[color]
accent = [1.0, 0.55, 0.15, 1.0]
paper  = [0.96, 0.96, 0.98]

[font]
label = "Barlow-SemiBold"

[shape]
radius = 4
```

Lookup order for `"$name"` is the local `[tokens]` table first, then each section of the shared file in file order; `"$section.name"` addresses one section explicitly. A local token may itself be a `"$ref"` into the shared file. Unknown tokens warn once (naming the property that used them) and leave the property unset, so the default applies.

TOML cannot hold `tokens = "..."` and a `[tokens]` table under one key. To use a local table *and* a shared file, name the file with `tokens_file = "ui/theme.toml"` at the top level, or with `import = "ui/theme.toml"` inside `[tokens]`:

```toml
[tokens]
import = "ui/theme.toml"
card_pad = [12, 8, 12, 8]
brand = "$color.accent"           # local alias for a shared token
```

Scripts read the same values with `ui_token(name)` (see below), so a Rhai HUD and a `.style.toml` never disagree about a colour or a spacing.

#### Hot Reload

Every loaded document remembers its `.ui.toml`, `.style.toml` and shared token file. The player polls them each frame alongside script hot-reload: when any changes, the document is re-parsed **in place** --- the handle stays valid, and runtime state set from scripts (`ui_set_text`, `ui_set_color`, `ui_set_bg_color`, `ui_set_visible`, `ui_set_class`, `ui_set_style`) is re-applied to every element id that still exists. New elements appear, removed ones vanish, edited tokens recolour. A file that fails to parse logs a warning and leaves the last good document in place until the next edit. Reloads are logged at `info` (`RUST_LOG=info`).

#### Rhai API: Data-Driven UI

| Function | Returns | Description |
|----------|---------|-------------|
| `load_ui(layout_path)` | `i64` | Load a `.ui.toml` document. Returns a handle (`-1` on error) |
| `unload_ui(handle)` | --- | Unload a previously loaded UI document |
| `ui_set_text(element_id, text)` | --- | Change an element's text content |
| `ui_show(element_id)` | --- | Show an element |
| `ui_hide(element_id)` | --- | Hide an element |
| `ui_set_visible(element_id, visible)` | --- | Set element visibility |
| `ui_set_color(element_id, r, g, b, a)` | --- | Override primary color |
| `ui_set_bg_color(element_id, r, g, b, a)` | --- | Override background color |
| `ui_set_style(element_id, prop, value)` | --- | Override any style property by name --- every property the `.style.toml` parser accepts (see below) |
| `ui_set_style_array(element_id, prop, array)` | --- | Same, for array values: `color` / `bg_color` / `stroke_color` / `padding` (3--4 numbers) or `shadow` (6 numbers). `ui_set_style` also accepts arrays directly |
| `ui_reset_style(element_id)` | --- | Clear all runtime overrides |
| `ui_set_class(element_id, class)` | --- | Switch an element's style class |
| `ui_exists(element_id)` | `bool` | Check if an element exists in any loaded document |
| `ui_get_rect(element_id)` | `Map` or `()` | Get resolved screen rect as `#{x, y, w, h}` |
| `ui_hit(element_id, x, y)` | `bool` | True when the point (logical points, e.g. `mouse_position()`) lies inside the element's resolved rect; `false` for unknown ids |
| `ui_token(name)` | `Array`, `float`, `string`, `bool` or `()` | Read a style token from the loaded documents' `[tokens]` tables and shared theme: `"accent"`, `"$accent"` and `"color.accent"` all work. Arrays and numbers come back as floats |

Element IDs are the TOML key names from the layout file (e.g., `"speed_label"`, `"lap_counter"`). Functions search all loaded documents when resolving an element ID.

`ui_set_style` takes whatever the property takes in the style file --- a number (`x`, `width_pct`, `font_size`, `thickness`, `stroke_width`, `letter_spacing`, `radius`, `rounding`, `margin_bottom`, `layer`, `opacity`), a string (`text_align`, `layout`, `font`), a bool (`height_auto`), an array (`color`, `bg_color`, `stroke_color`, `padding`, `shadow`) or a `"$token"` string for any of them, resolved through the owning document's tokens:

```rhai
ui_set_style("card_1_border", "color", "$accent");        // token -> colour
ui_set_style("card_1_name", "font", "$font.display");
ui_set_style("speed_panel", "padding", [12, 8, 12, 8]);
ui_set_style("speed_panel", "height_auto", true);
ui_set_style("speed_value", "text_align", "right");

let accent = ui_token("accent");                            // [1.0, 0.55, 0.15, 1.0]
draw_rect(10.0, 10.0, 40.0, 4.0, accent[0], accent[1], accent[2], accent[3]);

if ui_hit("start_button", mouse_x, mouse_y) && is_action_just_pressed("click") { start(); }
```

An unknown property name logs one warning per (element, property) and is ignored; a `load_ui` failure returns `-1` and logs the resolved path together with the parse error.

#### Example: Menu with Data-Driven UI

```toml
# ui/main_menu.ui.toml
[ui]
name = "Main Menu"
style = "ui/main_menu.style.toml"

[elements.title]
type = "text"
anchor = "top-center"
class = "title"
text = "MY GAME"

[elements.menu_panel]
type = "panel"
anchor = "center"
class = "menu-container"

[elements.btn_play]
type = "text"
parent = "menu_panel"
class = "menu-item"
text = "Play"

[elements.btn_quit]
type = "text"
parent = "menu_panel"
class = "menu-item"
text = "Quit"
```

```rust
// scripts/menu.rhai
let menu_handle = 0;
let selected = 0;

fn on_init() {
    menu_handle = load_ui("ui/main_menu.ui.toml");
}

fn on_update() {
    // Highlight selected item
    if selected == 0 {
        ui_set_color("btn_play", 1.0, 0.85, 0.2, 1.0);
        ui_set_color("btn_quit", 0.6, 0.6, 0.6, 1.0);
    } else {
        ui_set_color("btn_play", 0.6, 0.6, 0.6, 1.0);
        ui_set_color("btn_quit", 1.0, 0.85, 0.2, 1.0);
    }

    if is_action_just_pressed("move_forward") { selected = 0; }
    if is_action_just_pressed("move_backward") { selected = 1; }

    if is_action_just_pressed("interact") {
        if selected == 0 { load_scene("scenes/level1.scene.toml"); }
    }
}
```

#### When to Use Each UI Approach

| Approach | Best For |
|----------|----------|
| **Data-driven** (`.ui.toml` + `.style.toml`) | Menus, HUD panels, dialog boxes, score displays --- anything with stable structure |
| **Procedural** (`draw_*` API) | Crosshairs, damage flashes, debug overlays, dynamic effects --- anything computed per-frame |
| **Both together** | Load a HUD layout for structure, use `draw_*` for dynamic overlays on top |

## Hot-Reload

The script system checks file modification timestamps each frame. When a `.rhai` file changes on disk:

1. The file is recompiled to a new AST
2. If compilation succeeds, the old AST is replaced and the new version takes effect immediately
3. If compilation fails, the old AST is kept and an error is logged --- the game never crashes from a script typo

This enables a fast iteration workflow: edit a script in your text editor, save, and see the result in the running game without restarting.

Shared modules under `scripts/lib/` are watched as well: a change there clears the module cache and recompiles every script in the scene, since any of them may import the changed file.

## Interactable System

The `interactable` component marks entities that the player can interact with at close range. It works together with scripting to create interactive objects:

```toml
[entities.tavern_door]
archetype = "door"

[entities.tavern_door.interactable]
prompt_text = "Open Door"
range = 3.0
interaction_type = "use"
enabled = true

[entities.tavern_door.script]
source = "door_interact.rhai"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `prompt_text` | string | `"Interact"` | Text shown on the HUD when in range |
| `range` | f32 | `3.0` | Maximum interaction distance from the player |
| `interaction_type` | string | `"use"` | Type of interaction: `use`, `talk`, `examine` |
| `enabled` | bool | `true` | Whether this interactable is currently active |

When the player is within `range` of an enabled interactable entity, the HUD displays a crosshair and the `prompt_text`. Pressing the Interact key (`E`) fires the `on_interact` callback on the entity's script.

The `find_nearest_interactable()` function scans all interactable entities each frame to determine which (if any) to highlight. The HUD prompt fades in and out based on proximity.

## Example: Interactive Door

```rust
// scripts/door_interact.rhai

let door_open = false;

fn on_interact() {
    let me = self_entity();
    door_open = !door_open;

    if door_open {
        play_clip(me, "door_swing");
        play_sound("door_open");
        log("Door opened");
    } else {
        play_clip(me, "door_close");
        play_sound("door_close");
        log("Door closed");
    }
}
```

## Example: Flickering Torch

```rust
// scripts/torch_flicker.rhai

fn on_update() {
    let me = self_entity();
    let t = total_time();

    // Flicker the emissive intensity with layered sine waves
    let flicker = 0.8 + 0.2 * sin(t * 8.0) * sin(t * 13.0 + 0.7);
    set_field(me, "material", "emissive_strength", clamp(flicker, 0.3, 1.0));
}
```

## Example: NPC Bartender

```rust
// scripts/bartender.rhai

fn on_init() {
    let me = self_entity();
    play_clip(me, "idle");
    log("Bartender ready to serve");
}

fn on_interact() {
    let me = self_entity();
    let player = get_entity("player");
    let dist = distance(me, player);

    // Face the player
    let my_pos = get_position(me);
    let player_pos = get_position(player);
    let angle = atan2(player_pos.x - my_pos.x, player_pos.z - my_pos.z);
    set_rotation(me, 0.0, angle * 57.2958, 0.0);

    // React
    play_sound("glass_clink");
    blend_to(me, "wave", 0.3);
    log("Bartender waves at you");
}
```

## Architecture

```
on_init ──► ScriptEngine.call_inits()
                │
                ▼
            per-entity Scope + AST
                │
                ▼
on_update ──► ScriptEngine.call_updates()
                │
                ▼
events ────► ScriptEngine.process_events()
                │                    │
                ▼                    ▼
        ECS reads/writes      ScriptCommands
        (via ScriptCallContext)  (PlaySound, FireEvent, Log,
                                  LoadScene, LoadChunk, SetVelocity2D, ...)
                                     │
on_draw_ui ► ScriptEngine            ▼
                │              PlayerApp processes
                ▼              deferred commands
          DrawCommands
          (Text, Rect, Circle,
           Line, Sprite)
                │
                ▼
          egui layer_painter()
          renders 2D overlay
```

Each entity gets its own Rhai `Scope`, preserving persistent variables between frames. The `Engine` is shared across all entities. World access happens through a `ScriptCallContext` that holds a raw pointer to the `FlintWorld` --- valid only during the call batch, cleared immediately after.

The context also carries the per-frame inputs the host fills before each batch: the `InputState` snapshot (actions, mouse, touch, swipes), the `ConductedSnapshot` from the music session (neutral when there is none), the set of loaded chunk IDs, and the camera follow / shake state. Setters such as `set_vignette` or `set_camera_roll` write `Option` overrides on the context that the player applies after the batch; `ScriptCommand` is the deferred-effect channel for anything that needs the world mutably (spawning, scene loads, chunk loads, 2D velocity). Chart cue parameters cross the boundary as a small `CueParam` enum (`Number`, `Text`, `Flag`) so `flint-script` never depends on `flint-music`.

## Example: Combat HUD

For game-specific UI, use a dedicated `hud_controller` entity with a `script` component. The entity has no physical presence in the world --- it exists only to run the HUD script:

```toml
[entities.hud_controller]

[entities.hud_controller.script]
source = "hud.rhai"
```

```rust
// scripts/hud.rhai

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();

    // Crosshair
    let cx = sw / 2.0;
    let cy = sh / 2.0;
    draw_line(cx - 10.0, cy, cx + 10.0, cy, 0.0, 1.0, 0.0, 0.8, 2.0);
    draw_line(cx, cy - 10.0, cx, cy + 10.0, 0.0, 1.0, 0.0, 0.8, 2.0);

    // Health bar
    let player = get_entity("player");
    if player != -1 && has_component(player, "health") {
        let hp = get_field(player, "health", "current_hp");
        let max_hp = get_field(player, "health", "max_hp");
        let pct = hp / max_hp;

        draw_rect(20.0, sh - 40.0, 200.0, 20.0, 0.2, 0.2, 0.2, 0.8);
        draw_rect(20.0, sh - 40.0, 200.0 * pct, 20.0, 0.8, 0.1, 0.1, 0.9);
        draw_text(25.0, sh - 38.0, `HP: ${hp}/${max_hp}`, 14.0, 1.0, 1.0, 1.0, 1.0);
    }

    // Interaction prompt
    let interact = find_nearest_interactable();
    if interact != () {
        let prompt = interact.prompt_text;
        let tw = measure_text(prompt, 18.0);
        draw_text(cx - tw.width / 2.0, cy + 40.0, `[E] ${prompt}`, 18.0, 1.0, 1.0, 1.0, 0.9);
    }
}
```

This pattern keeps all game-specific HUD logic in scripts rather than engine code. The engine provides only the generic draw primitives.

## Further Reading

- [Audio](audio.md) --- sound system that scripts can control
- [Animation](animation.md) --- animation system driven by script commands
- [Physics and Runtime](physics-and-runtime.md) --- the game loop that calls scripts
- [Rendering](rendering.md) --- billboard sprites and the PBR pipeline
- [Building a Tavern](../guides/building-a-tavern.md) --- tutorial using scripts for interactive entities
