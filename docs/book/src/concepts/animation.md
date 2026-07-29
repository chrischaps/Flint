# Animation

Flint's animation system provides two tiers of animation through the `flint-animation` crate: **property tweens** for simple transform animations defined in TOML, and **skeletal animation** for character rigs imported from glTF files with GPU vertex skinning.

## Tier 1: Property Animation

Property animations are the simplest form --- animate any transform property (position, rotation, scale) or custom float field over time using keyframes. No 3D modeling tool required; clips are defined entirely in TOML.

### Animation Clips

Clips are `.anim.toml` files stored in the `demo/animations/` directory:

```toml
# animations/door_swing.anim.toml
name = "door_swing"
duration = 0.8

[[tracks]]
interpolation = "Linear"

[tracks.target]
type = "Rotation"

[[tracks.keyframes]]
time = 0.0
value = [0.0, 0.0, 0.0]

[[tracks.keyframes]]
time = 0.8
value = [0.0, 90.0, 0.0]

[[events]]
time = 0.0
event_name = "door_creak"
```

### Interpolation Modes

| Mode | Behavior |
|------|----------|
| **Step** | Jumps instantly to the next keyframe value |
| **Linear** | Linearly interpolates between keyframes |
| **CubicSpline** | Smooth interpolation with in/out tangents (matches glTF spec) |

### Track Targets

Each track animates a specific property:

| Target | Description |
|--------|-------------|
| `Position` | Entity position `[x, y, z]` |
| `Rotation` | Entity rotation in euler degrees `[x, y, z]` |
| `Scale` | Entity scale `[x, y, z]` |
| `CustomFloat` | Any numeric component field (specify `component` and `field`) |

### Animation Events

Clips can fire game events at specific times --- useful for triggering sounds (footstep at a specific frame), spawning particles, or notifying scripts. Events fire once per loop cycle.

### Attaching an Animation

Add an `animator` component to any entity in your scene:

```toml
[entities.platform]
archetype = "furniture"

[entities.platform.transform]
position = [2.0, 0.5, 3.0]

[entities.platform.animator]
clip = "platform_bob"
autoplay = true
loop = true
speed = 1.0
```

The animation system scans for `.anim.toml` files at startup and matches clip names to `animator` components.

## Tier 2: Skeletal Animation

For characters and complex articulated meshes, skeletal animation imports bone hierarchies from glTF files and drives them with GPU vertex skinning.

### Pipeline

```
glTF file (.glb)
  ├── Skin: joint hierarchy + inverse bind matrices
  ├── Mesh: positions, normals, UVs, joint_indices, joint_weights
  └── Animations: per-joint translation/rotation/scale channels
         │
         ▼
  ┌──────────────────────┐
  │   flint-import        │  Extract skeleton, clips, skinned vertices
  └──────────┬───────────┘
             │
  ┌──────────▼───────────┐
  │   flint-animation     │  Evaluate keyframes → bone matrices each frame
  └──────────┬───────────┘
             │
  ┌──────────▼───────────┐
  │   flint-render        │  Upload bone matrices → vertex shader skinning
  └──────────────────────┘
```

### How It Works

1. **Import** --- `flint-import` extracts the skeleton (joint hierarchy, inverse bind matrices) and animation clips (per-joint keyframe channels) from glTF files
2. **Evaluate** --- each frame, `flint-animation` samples the current clip time to produce local joint poses, walks the bone hierarchy to compute global transforms, and multiplies by inverse bind matrices to get final bone matrices
3. **Render** --- bone matrices are uploaded to a GPU storage buffer. The skinned vertex shader transforms each vertex by its weighted bone influences

### Skinned Vertices

Skeletal meshes use a separate `SkinnedVertex` type with 6 attributes (vs. 4 for static geometry), avoiding 32 bytes of wasted bone data on every static vertex in the scene:

| Attribute | Type | Description |
|-----------|------|-------------|
| `position` | vec3 | Vertex position |
| `normal` | vec3 | Vertex normal |
| `color` | vec4 | Vertex color |
| `uv` | vec2 | Texture coordinates |
| `joint_indices` | uvec4 | Indices of 4 influencing bones |
| `joint_weights` | vec4 | Weights for each bone (sum to 1.0) |

### Crossfade Blending

Smooth transitions between skeletal clips (e.g., idle to walk) use crossfade blending controlled by the `animator` component:

```toml
[entities.character.animator]
clip = "idle"
playing = true
loop = true
blend_target = "walk"      # Crossfade into this clip
blend_duration = 0.3       # Over 0.3 seconds
```

Blending uses slerp for rotation quaternions and lerp for translation/scale, producing smooth pose interpolation.

**The engine clears `blend_target` when the crossfade completes.** It is a
request, not a state — once the fade lands, the field is retired and the clip
you faded into becomes the plain `clip`. Scripts must not treat a non-empty
`blend_target` as "currently blending to X" beyond the fade, and must not
re-assert it every frame: a target that never retires re-arms its own crossfade
forever, and the clip plays only its first `blend_duration` seconds on loop.

Calling `blend_to` with the clip that is **already playing** is a deliberate
restart, not a no-op. That is what lets a held key chain discrete steps —
each press re-triggers the same clip from the top.

> `flint edit <model.glb>` plays clips directly and never goes through the
> blend path, so a clip that previews correctly can still be broken in play.
> Verify crossfades in `flint play`.

### Additive Layers

An additive layer runs a second clip on its own clock and composes it *onto*
the base pose, as a delta from rest, on keyed joints only:

```toml
[entities.character.animator]
clip = "idle"
layer_clip = "breathe"     # loops independently
layer_weight = 1.0         # live dial: 0 = off
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `layer_clip` | string | "" | Additive layer clip |
| `layer_weight` | f32 | 1.0 | Layer strength (0 = off) |

Because the layer only touches joints it actually keys, a breathing or
weight-shift layer authored on the spine leaves the legs entirely to the base
clip. And because it is composed after blending, **the layer survives base
crossfades** — the character keeps breathing through the transition from idle
to walk rather than holding its breath for 0.3 seconds.

`layer_weight` is a live dial, so fading a layer in and out is a single field
write.

### Rest Poses

Pose buffers are seeded from the glTF **bind-local TRS**, not from identity.
This matters for sparse clips: a clip that keys only an arm would otherwise
collapse every un-keyed limb onto its parent, and your character would fold up
the moment it played.

### Skeleton Schema

The `skeleton` component references a glTF skin:

```toml
[entities.character.skeleton]
skin = "Armature"           # Name of the glTF skin
```

Entities with both `animator` and `skeleton` components use the skeletal animation path. Entities with only `animator` use property tweens.

## Animator Schema

The `animator` component controls playback for both tiers:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `clip` | string | "" | Current animation clip name |
| `playing` | bool | false | Whether the animation is playing |
| `autoplay` | bool | false | Start playing on scene load |
| `loop` | bool | true | Loop when the clip ends |
| `speed` | f32 | 1.0 | Playback speed (-10.0 to 10.0) |
| `blend_target` | string | "" | Clip to crossfade into (cleared by the engine when the fade completes) |
| `blend_duration` | f32 | 0.3 | Crossfade duration in seconds |
| `layer_clip` | string | "" | Additive layer clip |
| `layer_weight` | f32 | 1.0 | Additive layer strength (0 = off) |

## Architecture

- **AnimationPlayer** --- clip registry and per-entity playback state for property tweens
- **AnimationSync** --- bridges ECS `animator` components to property animation playback, auto-discovers new entities each frame
- **SkeletalSync** --- bridges ECS to skeletal animation, manages per-entity skeleton state and bone matrix computation
- **AnimationSystem** --- top-level `RuntimeSystem` implementation that ticks both tiers

Animation runs in `update()` (variable-rate), not `fixed_update()`, because smooth interpolation benefits from matching the rendering frame rate rather than the physics tick rate.

## Scripting Integration

Animations can be controlled from [Rhai scripts](scripting.md) by writing directly to the `animator` component. The `AnimationSync` system picks up changes on the next frame:

| Function | Description |
|----------|-------------|
| `play_clip(entity_id, clip_name)` | Start playing a named animation clip |
| `stop_clip(entity_id)` | Stop the current animation |
| `blend_to(entity_id, clip, duration)` | Crossfade to another clip over the given duration |
| `set_anim_speed(entity_id, speed)` | Set animation playback speed |

```rust
// In a Rhai script:
fn on_interact() {
    let me = self_entity();
    play_clip(me, "door_swing");
}

fn on_init() {
    let me = self_entity();
    blend_to(me, "idle", 0.3);  // Smooth transition to idle
}
```

## Further Reading

- [Scripting](scripting.md) --- full scripting API including animation functions
- [Audio](audio.md) --- audio system that responds to animation events
- [Rendering](rendering.md) --- the skinned mesh GPU pipeline
- [Physics and Runtime](physics-and-runtime.md) --- the game loop that drives animation
- [File Formats](../formats/overview.md) --- `.anim.toml` format reference
