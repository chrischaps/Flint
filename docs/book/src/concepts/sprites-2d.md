# 2D Sprites

Flint's 2D sprite system provides GPU-instanced flat-quad rendering for side-scrollers, top-down games, and any project that needs sprite-based visuals. It sits alongside the existing 3D billboard sprite pipeline but uses an orthographic camera and XY-plane alignment instead of camera-facing quads.

## How It Works

2D sprites render as axis-aligned quads on the XY plane using a dedicated `Sprite2dPipeline` in `flint-render`. The pipeline follows the same storage-buffer instancing pattern as the particle system --- one draw call per texture batch, quads generated from `vertex_index` in the shader, no vertex buffers needed.

```
Scene TOML              CPU collection              GPU rendering
sprite component  ──►  Collect instances   ──►  Sprite2dPipeline
  mode = "sprite2d"    batch by texture           instanced draw
  texture, layer       sort by layer              storage buffer
  tint, flip           pack Sprite2dInstanceGpu    orthographic proj
```

Key differences from 3D billboard sprites:

| | Billboard | Sprite2D |
|---|---|---|
| **Orientation** | Always faces camera | Fixed on XY plane |
| **Camera** | Perspective (3D scene) | Orthographic |
| **Z-ordering** | Depth buffer | Layer field (integer) |
| **Depth write** | Yes (binary alpha discard) | No (layer-based sorting) |
| **Use case** | Items/NPCs in 3D world | 2D games |

## Setting Up a 2D Scene

A 2D scene needs an orthographic camera and sprites set to `sprite2d` mode:

```toml
[scene]
name = "My 2D Game"

[camera]
projection = "orthographic"
ortho_height = 10.0            # visible height in world units
position = [0.0, 0.0, 50.0]   # Z offset doesn't affect ortho rendering
target = [0.0, 0.0, 0.0]

[entities.player]
archetype = "sprite"
[entities.player.transform]
position = [0.0, 2.0, 0.0]
[entities.player.sprite]
mode = "sprite2d"
texture = "player.png"
width = 1.0
height = 2.0
layer = 10
tint = [1.0, 1.0, 1.0, 1.0]

[entities.background]
archetype = "sprite"
[entities.background.transform]
position = [0.0, 0.0, 0.0]
[entities.background.sprite]
mode = "sprite2d"
texture = "sky.png"
width = 20.0
height = 12.0
layer = 0
```

The `layer` field controls draw order --- higher layers render in front. Unlike depth-buffer sorting, this gives you explicit, deterministic control over what overlaps what.

## Sprite Component

The `sprite` component works for both billboard and sprite2d modes. Set `mode = "sprite2d"` to switch to flat rendering.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `texture` | string | `""` | Sprite texture file (empty = solid color from tint) |
| `width` | f32 | 1.0 | World-space width |
| `height` | f32 | 1.0 | World-space height |
| `mode` | string | `"billboard"` | `"billboard"` (3D camera-facing) or `"sprite2d"` (flat XY quad) |
| `layer` | i32 | 0 | Z-order layer for 2D sorting (higher = in front) |
| `tint` | vec4 | [1,1,1,1] | RGBA color multiplier |
| `flip_x` | bool | false | Mirror horizontally |
| `flip_y` | bool | false | Mirror vertically |
| `source_rect` | vec4 | [0,0,0,0] | Source rectangle in pixels `[x, y, w, h]`; `[0,0,0,0]` = full texture |
| `anchor_y` | f32 | 0.0 | Vertical anchor (0 = bottom, 0.5 = center) |
| `frame` | i32 | 0 | Current sprite sheet frame index |
| `frames_x` | i32 | 1 | Columns in sprite sheet |
| `frames_y` | i32 | 1 | Rows in sprite sheet |
| `fullbright` | bool | true | Bypass PBR lighting (always true for 2D) |
| `visible` | bool | true | Show/hide the sprite |

## Texture Atlases

For sprite sheets and texture packing, Flint supports `.atlas.toml` sidecar files that define named regions within a texture:

```toml
# sprites/player_sheet.atlas.toml
[atlas]
texture = "player_sheet.png"

[atlas.regions]
idle_0  = [0,   0, 64, 64]
idle_1  = [64,  0, 64, 64]
run_0   = [0,  64, 64, 64]
run_1   = [64, 64, 64, 64]
run_2   = [128, 64, 64, 64]
run_3   = [192, 64, 64, 64]
```

Each region is `[x, y, width, height]` in pixel coordinates. The `AtlasRegistry` loads all `.atlas.toml` files from a directory and resolves region names to UV rectangles at runtime.

## Sprite Sheet Animation

The `flint-animation` crate provides frame-based sprite animation through `.sprite.toml` clip files and the `sprite_animator` component.

### Defining Clips

Animation clips live in `.sprite.toml` files alongside your scene:

```toml
# animations/character.sprite.toml

[animation.idle]
texture = "character_sheet.png"
frame_size = [64, 64]
frames = [
    { col = 0, row = 0, duration_ms = 300 },
    { col = 1, row = 0, duration_ms = 300 },
]
loop_mode = "loop"

[animation.run]
texture = "character_sheet.png"
frame_size = [64, 64]
frames = [
    { col = 0, row = 1, duration_ms = 100 },
    { col = 1, row = 1, duration_ms = 100 },
    { col = 2, row = 1, duration_ms = 100 },
    { col = 3, row = 1, duration_ms = 100 },
]
loop_mode = "loop"

[animation.jump]
texture = "character_sheet.png"
frame_size = [64, 64]
frames = [
    { col = 0, row = 2, duration_ms = 150 },
    { col = 1, row = 2, duration_ms = 200 },
    { col = 2, row = 2, duration_ms = 300 },
]
loop_mode = "once"
```

Each frame specifies a column and row in the sprite sheet, plus a duration in milliseconds. A single file can contain multiple named clips.

### Loop Modes

| Mode | Behavior |
|------|----------|
| `loop` | Repeats indefinitely from the start |
| `ping_pong` | Plays forward then backward, repeating |
| `once` | Plays once and stops on the last frame |

### Attaching Animation

Add a `sprite_animator` component alongside the `sprite` component:

```toml
[entities.player]
archetype = "animated_sprite"
[entities.player.transform]
position = [0.0, 2.0, 0.0]
[entities.player.sprite]
mode = "sprite2d"
texture = "character_sheet.png"
width = 2.0
height = 2.0
[entities.player.sprite_animator]
clip = "idle"
playing = true
speed = 1.0
```

The `animated_sprite` archetype bundles `transform`, `sprite` (mode = `sprite2d`), and `sprite_animator` together.

### Sprite Animator Component

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `clip` | string | `""` | Active animation clip name |
| `playing` | bool | false | Whether animation is playing |
| `speed` | f32 | 1.0 | Playback speed multiplier (0--10) |

### How Playback Works

`SpriteAnimSync` runs each frame during the animation update:

1. **Sync from world** --- reads `sprite_animator.clip` and `sprite_animator.playing` from the ECS, detects changes
2. **Advance** --- steps playback time by `delta * speed`, finds the current frame in the clip
3. **Write back** --- computes the source rectangle from the frame's col/row and writes `sprite.source_rect` back to the ECS

The source rectangle tells the GPU which portion of the sprite sheet to display. This happens automatically --- scripts only need to set the clip name and playing state.

## Scripting Integration

Control sprite animation from [Rhai scripts](scripting.md):

| Function | Description |
|----------|-------------|
| `sprite_play(entity_id, clip_name)` | Start playing a clip |
| `sprite_stop(entity_id)` | Stop playback |
| `sprite_set_speed(entity_id, speed)` | Set playback speed |
| `sprite_is_playing(entity_id)` | Check if currently playing |

The `on_animation_end(clip_name)` callback fires when a `once` clip finishes:

```rust
// Rhai script: character animation controller
fn on_init() {
    let me = self_entity();
    sprite_play(me, "idle");
}

fn on_update() {
    let me = self_entity();
    let vx = 0.0;

    if action_held("move_right") { vx = 1.0; }
    if action_held("move_left")  { vx = -1.0; }

    if vx != 0.0 {
        sprite_play(me, "run");
        set_field(me, "sprite", "flip_x", vx < 0.0);
    } else {
        sprite_play(me, "idle");
    }
}

fn on_animation_end(clip) {
    if clip == "jump" {
        sprite_play(self_entity(), "idle");
    }
}
```

## Architecture

- **Sprite2dPipeline** (`flint-render`) --- wgpu render pipeline with orthographic projection, storage buffer instancing, per-texture batching
- **Sprite2dInstanceGpu** --- 80-byte per-instance struct: position, size, UV rect, tint, flip flags
- **TextureAtlas / AtlasRegistry** (`flint-render`) --- parses `.atlas.toml` files, resolves region names to pixel rectangles
- **SpriteAnimClip** (`flint-animation`) --- frame sequence with per-frame duration and loop mode
- **SpriteAnimSync** (`flint-animation`) --- bridges ECS `sprite_animator` components to playback state, writes `source_rect` back each frame

## Further Reading

- [Touch Input](touch-input.md) --- mobile-friendly input for 2D games
- [Animation](animation.md) --- property tweens and skeletal animation
- [Scripting](scripting.md) --- full scripting API including sprite animation functions
- [Rendering](rendering.md) --- the GPU pipeline architecture
