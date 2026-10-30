# Touch Input

Flint's input system extends seamlessly to touchscreens through **touch zones** --- named screen regions that map to the same action bindings used by keyboard and gamepad. A single input config can drive a game on desktop and mobile without code changes.

## How It Works

Touch input integrates into the existing `InputState` system in `flint-runtime`:

```
Touch event (OS)         Normalized tracking         Action evaluation
TouchStart(id, x, y) ──► TouchPoint { id, pos,  ──► binding_value(TouchZone)
TouchMove(id, x, y)      start_pos, phase,          checks zone containment
TouchEnd(id)              start_time }               produces action values
```

Touch coordinates are normalized to `[0..1]` range (0,0 = top-left, 1,1 = bottom-right), making zone definitions resolution-independent. Tap detection uses physics: a touch qualifies as a tap when elapsed time < 300ms and movement distance < 20 pixels.

## Touch Zones

Touch zones are named rectangular screen regions. Five built-in zones cover the most common layouts:

| Zone | Region | Common Use |
|------|--------|------------|
| `full_screen` | Entire screen | Global taps, swipes |
| `left_half` | Left 50% | Move left, D-pad left |
| `right_half` | Right 50% | Move right, D-pad right |
| `top_half` | Top 50% | Look up, jump |
| `bottom_half` | Bottom 50% | Look down, crouch |

Zones are defined as normalized rectangles `(x, y, width, height)`. For example, `left_half` is `(0.0, 0.0, 0.5, 1.0)`.

## Input Configuration

Touch zones use the same `InputConfig` TOML format as keyboard and gamepad bindings. A single action can have multiple binding types:

```toml
# input.toml
version = 1
game_id = "my_game"

[actions.move_left]
kind = "button"
[[actions.move_left.bindings]]
type = "key"
code = "KeyA"
[[actions.move_left.bindings]]
type = "touch_zone"
zone = "left_half"

[actions.move_right]
kind = "button"
[[actions.move_right.bindings]]
type = "key"
code = "KeyD"
[[actions.move_right.bindings]]
type = "touch_zone"
zone = "right_half"

[actions.jump]
kind = "button"
[[actions.jump.bindings]]
type = "key"
code = "Space"
```

Touch zone bindings support a `scale` field (default 1.0) that multiplies the action value, just like gamepad axis bindings.

### Binding Format

```toml
[[actions.my_action.bindings]]
type = "touch_zone"
zone = "left_half"     # Zone name (one of the 5 built-in zones)
scale = 1.0            # Optional: action value multiplier
```

## Mouse-as-Touch Emulation

By default, Flint emulates touch input from mouse clicks on desktop. Left-click-and-drag produces touch events as finger ID 0, letting you test touch-based games without a touchscreen.

- Enabled by default (`emulate_touch_from_mouse = true`)
- Automatically disabled when a real touch event arrives
- Left mouse button maps to finger 0
- Mouse position maps to touch position

This means touch-zone bindings work immediately on desktop during development.

## Tap Detection

Taps are detected automatically when a touch ends:

- **Duration** < 300ms (from touch start to touch end)
- **Distance** < 20 pixels (from start position to end position)

Taps are available for one frame after detection and are consumed on read.

## Scripting API

Touch state is accessible from [Rhai scripts](scripting.md) via these functions:

### Touch Tracking

| Function | Returns | Description |
|----------|---------|-------------|
| `touch_count()` | `i64` | Number of currently active touches |
| `touch_x(index)` | `f64` | Normalized X position (0--1) of touch at index |
| `touch_y(index)` | `f64` | Normalized Y position (0--1) of touch at index |
| `is_touching(id)` | `bool` | Whether the given touch ID is currently active |
| `touch_just_started(id)` | `bool` | Whether the touch ID just became active this frame |
| `touch_just_ended(id)` | `bool` | Whether the touch ID just ended this frame |

### Tap Detection

| Function | Returns | Description |
|----------|---------|-------------|
| `tap_count()` | `i64` | Number of taps detected this frame |
| `tap_x(index)` | `f64` | Normalized X position of tap at index |
| `tap_y(index)` | `f64` | Normalized Y position of tap at index |

### Swipe Detection

A swipe is a touch that travelled far enough, fast enough, before lifting. The engine classifies it into one of four directions; the position reported is where the swipe started.

| Function | Returns | Description |
|----------|---------|-------------|
| `swipe_count()` | `i64` | Number of swipes detected this frame |
| `swipe_direction(index)` | `String` | `"up"`, `"down"`, `"left"` or `"right"` (empty if out of range) |
| `swipe_x(index)` / `swipe_y(index)` | `f64` | Normalized start position of the swipe (`-1.0` if out of range) |
| `is_swipe(direction)` | `bool` | Whether any swipe in that direction happened this frame |

### Example: Touch-Driven Movement

```rust
// scripts/touch_controller.rhai
fn on_update() {
    let me = self_entity();
    let speed = 5.0;
    let dt = delta_time();

    // Action-based movement works with both keyboard and touch
    if action_held("move_left") {
        let pos = get_position(me);
        set_position(me, pos.x - speed * dt, pos.y, pos.z);
    }
    if action_held("move_right") {
        let pos = get_position(me);
        set_position(me, pos.x + speed * dt, pos.y, pos.z);
    }

    // Direct tap handling for jumping
    if tap_count() > 0 {
        // Jump on any tap
        fire_event("jump");
    }
}

fn on_draw_ui() {
    // Visualize active touches (useful for debugging)
    let w = screen_width();
    let h = screen_height();

    for i in 0..touch_count() {
        let tx = touch_x(i) * w;
        let ty = touch_y(i) * h;
        draw_circle(tx, ty, 20.0, 1.0, 1.0, 1.0, 0.4);
    }
}
```

## Design Philosophy

The touch system is intentionally minimal. Rather than providing virtual joysticks, gesture recognizers, or complex multi-touch state machines, it gives you:

1. **Zone-based action bindings** --- works with the existing input config system
2. **Raw touch state** --- positions, phases, and tap detection exposed to scripts
3. **Mouse emulation** --- desktop testing without hardware

Game-specific touch UI (virtual D-pads, on-screen buttons, swipe gestures) belongs in scripts, not the engine. The engine provides the primitives; scripts compose them into the interaction model that fits each game.

## Further Reading

- [2D Sprites](sprites-2d.md) --- the rendering system for 2D games
- [Scripting](scripting.md) --- full scripting API reference
- [Physics and Runtime](physics-and-runtime.md) --- the input system and game loop
- [Deploying to Android](../guides/android.md) --- building and running on mobile devices
