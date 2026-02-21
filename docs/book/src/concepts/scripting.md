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
| `set_rotation(id, x, y, z)` | --- | Set entity rotation (euler degrees) |
| `distance(a, b)` | `f64` | Euclidean distance between two entities |
| `set_parent(child_id, parent_id)` | --- | Set an entity's parent in the hierarchy |
| `get_parent(id)` | `i64` | Get the parent entity ID (`-1` if none) |
| `get_children(id)` | `Array` | Get child entity IDs as an array |
| `get_world_position(id)` | `Map` | World-space position as `#{x, y, z}` (accounts for parent transforms) |
| `set_material_color(id, r, g, b, a)` | --- | Set the material base color (RGBA, 0.0--1.0) |
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

Action names are defined by input configuration files and are fully customizable per game. The built-in defaults include: `move_forward`, `move_backward`, `move_left`, `move_right`, `jump`, `interact`, `sprint`, `weapon_1`, `weapon_2`, `reload`, `fire`. Games can define arbitrary custom actions in their input config TOML files and query them from scripts with `is_action_pressed("custom_action")`.

Input bindings support keyboard, mouse, and gamepad devices. See [Physics and Runtime: Input System](physics-and-runtime.md#input-system) for the config file format and layered loading model.

### Time API

| Function | Returns | Description |
|----------|---------|-------------|
| `delta_time()` | `f64` | Seconds since last frame |
| `total_time()` | `f64` | Total elapsed time since scene start |

### Audio API

Audio functions produce deferred commands that the player processes after the script update phase:

| Function | Description |
|----------|-------------|
| `play_sound(name)` | Play a non-spatial sound at default volume |
| `play_sound(name, volume)` | Play a non-spatial sound at the given volume (0.0--1.0) |
| `play_sound_at(name, x, y, z, volume)` | Play a spatial sound at a 3D position |
| `stop_sound(name)` | Stop a playing sound |

Sound names match the audio files loaded from the `audio/` directory (without extension).

### Animation API

Animation functions write directly to the `animator` component on the target entity. The `AnimationSync` system picks up changes on the next frame:

| Function | Description |
|----------|-------------|
| `play_clip(entity_id, clip_name)` | Start playing a named animation clip |
| `stop_clip(entity_id)` | Stop the current animation |
| `blend_to(entity_id, clip, duration)` | Crossfade to another clip over the given duration |
| `set_anim_speed(entity_id, speed)` | Set animation playback speed |

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

### Event API

| Function | Description |
|----------|-------------|
| `fire_event(name)` | Fire a named game event |
| `fire_event_data(name, data)` | Fire an event with a data map payload |

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
| `get_camera_position()` | `Map` | Camera world position as `#{x, y, z}` |
| `get_camera_direction()` | `Map` | Camera forward vector as `#{x, y, z}` |
| `set_camera_position(x, y, z)` | --- | Override camera position from script |
| `set_camera_target(x, y, z)` | --- | Override camera look-at target from script |
| `set_camera_fov(fov)` | --- | Override camera field of view (degrees) from script |

The `raycast()` function automatically excludes the calling entity's collider from results. On a hit, it returns a map with these fields:

| Field | Type | Description |
|-------|------|-------------|
| `entity` | `i64` | Entity ID of the hit object |
| `distance` | `f64` | Distance from origin to hit point |
| `point_x`, `point_y`, `point_z` | `f64` | World-space hit position |
| `normal_x`, `normal_y`, `normal_z` | `f64` | Surface normal at hit point |

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

### Spline API

Query spline entities for path-following, track layouts, and procedural placement:

| Function | Returns | Description |
|----------|---------|-------------|
| `spline_closest_point(spline_id, x, y, z)` | `Map` or `()` | Nearest point on spline to query position. Returns `#{t, x, y, z, dist_sq}` |
| `spline_sample_at(spline_id, t)` | `Map` or `()` | Sample spline at parameter `t` (0.0--1.0). Returns `#{x, y, z, fwd_x, fwd_y, fwd_z, right_x, right_y, right_z}` |

The `t` parameter wraps for closed splines. The returned forward and right vectors are normalized and can be used for orientation.

### Particle API

| Function | Description |
|----------|-------------|
| `emit_burst(entity_id, count)` | Fire N particles immediately |
| `start_emitter(entity_id)` | Start continuous emission |
| `stop_emitter(entity_id)` | Stop emission (existing particles finish their lifetime) |
| `set_emission_rate(entity_id, rate)` | Change emission rate dynamically |

See [Particles](particles.md) for full component schema and recipes.

### Post-Processing API

Control the HDR post-processing pipeline at runtime from scripts:

| Function | Description |
|----------|-------------|
| `set_vignette(intensity)` | Set vignette intensity (0.0 = none, 1.0 = heavy) |
| `set_bloom_intensity(intensity)` | Set bloom strength (0.0 = none) |
| `set_exposure(value)` | Set exposure multiplier (1.0 = default) |

These overrides are applied each frame and combine with the scene's `[post_process]` baseline settings. Useful for dynamic effects like speed vignetting, boost bloom, or exposure flashes.

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
| `persist_save(path)` | --- | Save store to a TOML file |
| `persist_load(path)` | --- | Load store from a TOML file |

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
| `ui_reset_style(element_id)` | --- | Remove all style overrides |
| `ui_set_class(element_id, class_name)` | --- | Change an element's style class |
| `ui_exists(element_id)` | `bool` | Check if a UI element exists |
| `ui_get_rect(element_id)` | `Map` | Get resolved position/size as `#{x, y, width, height}` |

UI documents are defined with paired `.ui.toml` (layout) and `.style.toml` (styling) files, following an HTML/CSS/JS-like separation of concerns. See [File Formats](../formats/overview.md) for the format specification.

### UI Draw API

The draw API lets scripts render 2D overlays each frame via the `on_draw_ui()` callback. Draw commands are issued in screen-space coordinates (logical points, not physical pixels) and rendered by the engine through egui.

#### Draw Primitives

| Function | Description |
|----------|-------------|
| `draw_text(x, y, text, size, r, g, b, a)` | Draw text at position |
| `draw_text_ex(x, y, text, size, r, g, b, a, layer)` | Draw text with explicit layer |
| `draw_rect(x, y, w, h, r, g, b, a)` | Draw filled rectangle |
| `draw_rect_ex(x, y, w, h, r, g, b, a, rounding, layer)` | Filled rectangle with corner rounding and layer |
| `draw_rect_outline(x, y, w, h, r, g, b, a, thickness)` | Rectangle outline |
| `draw_circle(x, y, radius, r, g, b, a)` | Draw filled circle |
| `draw_circle_outline(x, y, radius, r, g, b, a, thickness)` | Circle outline |
| `draw_line(x1, y1, x2, y2, r, g, b, a, thickness)` | Draw a line segment |
| `draw_sprite(x, y, w, h, name)` | Draw a sprite image |
| `draw_sprite_ex(x, y, w, h, name, u0, v0, u1, v1, r, g, b, a, layer)` | Sprite with custom UV coordinates, tint, and layer |

#### Query Functions

| Function | Returns | Description |
|----------|---------|-------------|
| `screen_width()` | `f64` | Logical screen width in points |
| `screen_height()` | `f64` | Logical screen height in points |
| `measure_text(text, size)` | `Map` | Approximate text size as `#{width, height}` |
| `find_nearest_interactable()` | `Map` or `()` | Nearest interactable entity info, or `()` if none in range |

`find_nearest_interactable()` returns a map with `entity` (ID), `prompt_text`, `interaction_type`, and `distance` fields when an interactable entity is within range.

#### Layer Ordering

All draw commands accept a `layer` parameter (or default to 0). Commands are sorted by layer before rendering:

- **Negative layers** render behind (background elements)
- **Layer 0** is the default
- **Positive layers** render in front (foreground elements)

#### Coordinate System

Coordinates are in **egui logical points**, not physical pixels. On high-DPI displays, logical points differ from pixels by the scale factor. Use `screen_width()` and `screen_height()` for layout calculations --- they return the correct logical dimensions.

#### Sprite Loading

Sprite names map to image files in the `sprites/` directory (without extension). Supported formats: PNG, JPG, BMP, TGA. Textures are lazy-loaded on first use and cached for subsequent frames.

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
| `text_align` | string | `"left"` | Text alignment: `left`, `center`, `right` |
| `rounding` | float | `0` | Corner rounding for panels/rects |
| `opacity` | float | `1.0` | Element opacity multiplier |
| `thickness` | float | `1` | Stroke thickness for outlines |
| `radius` | float | `0` | Circle radius |
| `layer` | int | `0` | Render layer (negative = behind, positive = in front) |
| `padding` | [l,t,r,b] | `[0,0,0,0]` | Interior padding (left, top, right, bottom) |
| `layout` | string | `"stack"` | Child flow: `stack` (vertical) or `horizontal` |
| `margin_bottom` | float | `0` | Space below element in flow layout |

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
| `ui_set_style(element_id, prop, value)` | --- | Override any style property by name |
| `ui_reset_style(element_id)` | --- | Clear all runtime overrides |
| `ui_set_class(element_id, class)` | --- | Switch an element's style class |
| `ui_exists(element_id)` | `bool` | Check if an element exists in any loaded document |
| `ui_get_rect(element_id)` | `Map` or `()` | Get resolved screen rect as `#{x, y, w, h}` |

Element IDs are the TOML key names from the layout file (e.g., `"speed_label"`, `"lap_counter"`). Functions search all loaded documents when resolving an element ID.

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
        (via ScriptCallContext)  (PlaySound, FireEvent, Log)
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
