# 2D Sprites

> **Package:** `engine`, the kernel: every game has it ([Packages](packages.md)).

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

Each region is `[x, y, width, height]` in pixel coordinates. The `AtlasRegistry` loads all `.atlas.toml` files from a directory and resolves region names to UV rectangles at runtime. A region that isn't four whole numbers is skipped with a warning naming it, and so is any key the format doesn't know (see [A key belongs to the header above it](scenes.md#a-key-belongs-to-the-header-above-it)).

## Sprite Sheet Animation

> **Package:** `flipbook`, a standard package on the `animation` package: a game has its clips, the `animated_sprite` archetype and the `flipbook` script module unless its `[packages]` writes `flipbook = false` (or `animation = false`) ([Packages](packages.md)).

Sprite-sheet animation is the `flipbook` package's (#1620, ADR 0139): `.sprite.toml` clips played on the `animation` package's clock, through the same `animator` component every clip plays through. Both are standard packages, on unless a game writes `flipbook = false` (or `animation = false`, which takes flipbook too) under `[packages]`; a game that leaves them out still draws its sprites, each at its first frame.

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
| `ping_pong` | Plays forward then backward, repeating (the end frames aren't shown twice) |
| `once` | Plays once and stops on the last frame |

The clip's mode and the animator's `loop` work together: `loop_mode = "once"` plays once, and `loop = false` on the animator makes any clip play once.

### Attaching Animation

Add an `animator` component alongside the `sprite` component:

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
[entities.player.animator]
clip = "idle"
playing = true
speed = 1.0
```

The `animated_sprite` archetype bundles `transform`, `sprite` (mode = `sprite2d`), and `animator` together.

A sprite's animator is the one every clip uses (see [Animation](animation.md)): `clip`, `playing`, `speed` (`-10..10`, negative plays backwards), `loop`, `autoplay` and `sequence` all work on it. A `blend_target` (or a sequence's `blend` step) is a cut on a sprite: frames don't crossfade.

### `sprite_animator` is `animator` now

Before #1620 a sprite played through its own `sprite_animator` component, with rules of its own. Its three fields are retired for one release: a scene that still writes them keeps animating (the animator reads an entity's `sprite_animator` when it has no `animator`), and `flint check` and the load say, for each field, ``sprite_animator` has no field `clip` any more``, with what to write instead. Rename the table: `[entities.player.sprite_animator]` becomes `[entities.player.animator]`, with the same keys.

### How Playback Works

The flipbook is a clip kind on the animation package's clock, at the end of the frame's animation (after property clips, sequences and skeletal clips):

1. **Follow the animator** --- a new clip cuts to its start, `playing` is live (a pause holds the frame and resumes there), a finished one-shot replays from its start
2. **Step the frames** --- each frame's `duration_ms` at `speed`, landing on the same frames as before the package
3. **Write back** --- computes the source rectangle from the frame's col/row and writes `sprite.source_rect` when the frame changes, and tells the animator when a one-shot ends

The source rectangle tells the GPU which portion of the sprite sheet to display. This happens automatically --- scripts only need to set the clip name and playing state.

## Scripting Integration

Control sprite animation from [Rhai scripts](scripting.md) with the `flipbook` module, the 2D names for the `animation` module's clip calls:

| Function | Description |
|----------|-------------|
| `play_sprite(entity_id, clip_name)` | Play a clip: a new one from its start, a paused one resumes, a finished one-shot replays (`play_clip`) |
| `stop_sprite(entity_id)` | Pause: the sprite holds its frame (`stop_clip`) |
| `set_sprite_speed(entity_id, speed)` | Set playback speed; negative plays backwards (`set_anim_speed`) |
| `is_sprite_playing(entity_id)` | Whether the animator is playing |

The `on_animation_end(clip_name)` callback fires when a one-shot finishes (of any kind of clip), and a [sequence](animation.md#sequences) can drive a sprite too, its cues reaching `on_sequence_cue(sequence, cue)` on the frame they fall on:

```rust
// Rhai script: character animation controller
fn on_init() {
    let me = self_entity();
    play_sprite(me, "idle");
}

fn on_update() {
    let me = self_entity();
    let vx = 0.0;

    if is_action_pressed("move_right") { vx = 1.0; }
    if is_action_pressed("move_left") { vx = -1.0; }

    if vx != 0.0 {
        play_sprite(me, "run");
        set_field(me, "sprite", "flip_x", vx < 0.0);
    } else {
        play_sprite(me, "idle");
    }
}

fn on_animation_end(clip) {
    if clip == "jump" {
        play_sprite(self_entity(), "idle");
    }
}
```

## Architecture

- **Sprite2dPipeline** (`flint-render`) --- wgpu render pipeline with orthographic projection, storage buffer instancing, per-texture batching
- **Sprite2dInstanceGpu** --- 80-byte per-instance struct: position, size, UV rect, tint, flip flags
- **TextureAtlas / AtlasRegistry** (`flint-render`) --- parses `.atlas.toml` files, resolves region names to pixel rectangles
- **SpriteAnimClip** (`flint-flipbook`) --- frame sequence with per-frame duration and loop mode
- **FlipbookKind** (`flint-flipbook`) --- the clip kind the animation package plays: follows each sprite's `animator`, writes `source_rect` when the frame changes

## Further Reading

- [Touch Input](touch-input.md) --- mobile-friendly input for 2D games
- [Animation](animation.md) --- property tweens and skeletal animation
- [Scripting](scripting.md) --- full scripting API including sprite animation functions
- [Rendering](rendering.md) --- the GPU pipeline architecture
