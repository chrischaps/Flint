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
3. **Render** --- bone matrices are uploaded to a per-entity GPU storage buffer. The skinned vertex shader transforms each vertex by its weighted bone influences. Because each entity owns its buffer, two entities instancing the same skinned asset animate independently: a crowd of the same model no longer shows whichever skeleton uploaded last.

The importer handles all three glTF interpolation modes. `CUBICSPLINE` samplers store three outputs per timestamp (`in_tangent`, `value`, `out_tangent`) and are consumed as triples; a sampler with fewer than `3 x timestamps` outputs warns and degrades to `LINEAR`. Rotation tracks are also made hemisphere-continuous at clip load: Blender exports adjacent keys as `q` then `-q` on large joint rotations, and the Hermite curve between them would collapse through zero and snap the joint. Such keys (value and both tangents) are negated so the curve stays on one side of the sphere.

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

### Animation Layers

Layers run extra clips on their own clocks and compose them onto the base
pose, **in array order, after any crossfade**. Each layer has a weight, a
mode, and an optional bone mask:

```toml
[entities.starthing.animator]
clip = "WalkCycle"
layers = [
  { clip = "StarCower", weight = 1.0, mode = "additive", mask = "head" },
]
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `clip` | string | "" | Clip to play on this layer (empty = inactive slot; indices stay stable) |
| `weight` | f32 | 1.0 | Live dial, 0 = off |
| `mode` | string | "additive" | `additive` or `override` (see below) |
| `mask` | string | "" | Root joint name; the layer only touches that joint and its descendants. **A name that is not in the skeleton masks out every joint**, so a typo makes the layer contribute nothing rather than everything |
| `speed` | f32 | 1.0 | Multiplier on the entity's base speed |
| `fade_target` | f32 | --- | Weight to ramp toward (see the fade section below) |
| `fade_duration` | f32 | 0.0 | Seconds for the ramp; the engine zeroes it when the ramp lands |

Layer indices run from `0` to `254`; the script API silently ignores anything at `255` or above (layer IDs travel as a byte in runtime bookkeeping).

**Additive** layers contribute each keyed joint's *delta from rest*, scaled
by weight. They suit overlays authored as "rest plus a gesture" — a breathing
chest, the starthing's cowering star-arms. **Override** layers blend each
keyed joint *toward the clip's pose* by weight — "upper body aims while the
legs keep walking", usually paired with a mask like `mask = "spine"`.

Either way a layer only touches joints its clip actually keys (composing
identity onto an un-keyed joint would corrupt one whose rest rotation is not
identity), and because layers are composed after blending, **they survive
base crossfades** — the character keeps breathing through the transition from
idle to walk rather than holding its breath for 0.3 seconds.

Layers are ordered: an additive layer under an override is replaced where the
override keys; an additive layer on top of an override adds to it.

The older single-layer fields still work and are treated as `layers[0]`:

```toml
layer_clip = "breathe"     # legacy alias for layers = [{ clip = "breathe", weight = ... }]
layer_weight = 1.0
```

When `layers` is non-empty it wins and the legacy pair is ignored. The script
API (`set_anim_layer` & co.) migrates the legacy pair into `layers[0]` the
first time it touches an entity.

#### Previewing layers

`flint edit <model.gltf>` has a **Layers** stack under the timeline: add rows,
pick a clip per layer, drag weights (they work while paused), flip
Add/Over, choose a mask joint, and solo/mute rows. The **Skeleton colour**
combo under *View* paints the armature overlay and the node tree:

- **Last writer** — yellow = base clip, one colour per layer, grey = rest pose
- **L*n* weight** — grey → layer colour by the weight that layer applied
- **L*n* mask** / **L*n* keyed joints** — which bones the mask / clip reaches

Hover a joint in the node tree for a per-layer breakdown. The same setup can
be rendered headlessly:

```bash
flint edit models/starthing.gltf --render out.png --clip WalkCycle --layer StarCower:1.0:head
# --layer clip[:weight[:mask[:mode]]], repeatable, in order
```

#### Layer fades

A layer's `weight` is a live dial, so a script that sets it pops the pose. To
ramp instead, set `fade_target` and `fade_duration` on the layer table (or call
`fade_anim_layer(entity, index, weight, seconds)`). The engine owns the weight
while the ramp runs: it writes the ramped value back to `layers[i].weight`
every frame and zeroes `fade_duration` when it arrives, so the next
`sync_from_world` doesn't re-arm it — the same contract as `blend_target`. Any
plain `set_anim_layer_weight` cancels a ramp in flight. The previewer's Layers
row shows `→ target (seconds left)` while a fade runs.

### Sequences

A `*.sequence.toml` is a list of timestamped animator events — everything the
script API can do to an `animator`, written down with times so it can be
played, scrubbed and rendered identically in the previewer and the player:

```toml
name = "starthing_showcase"
# duration = 6.0   # optional; default = last event time + its transition
loop = false

[[events]]
time = 0.0
kind = "blend"            # crossfade the base clip (duration 0 = hard cut)
clip = "BreathingIdle"
duration = 0.0

[[events]]
time = 1.5
kind = "blend"
clip = "WalkCycle"
duration = 0.4

[[events]]
time = 2.5
kind = "layer"            # set a layer slot; omitted keys keep their value
index = 0
clip = "StarCower"
mode = "additive"
mask = "head"
weight = 1.0
fade = 0.3                # ramp the weight over 0.3 s (omit = instant)

[[events]]
time = 4.0
kind = "speed"
value = 1.3

[[events]]
time = 5.5
kind = "layer"
index = 0
weight = 0.0
fade = 0.4

[[events]]
time = 6.0
kind = "cue"              # named marker for scripts
name = "done"
```

Event kinds: `blend { clip, duration }`, `layer { index, clip?, weight?, fade,
mode?, mask? }`, `speed { value }`, `cue { name }`. Events are sorted by time
(stable, so same-time events keep authored order) and each fires exactly once
when the playhead reaches its time. Sequences run before the
skeletal tier each frame, so their writes land the same frame.

A looping sequence fires everything up to `min(time, duration)`, then wraps,
re-arms every event (including those at `t = 0`) and fires from zero again,
repeating while the frame still overruns. A large `dt` therefore neither skips
events nor delays the next pass by a frame: a 1 s loop with a cue at 0.8 s,
advanced by 0.5 then 0.6, fires `start, tail, start`. A looping sequence whose
resolved duration is zero is rejected at load (`looping sequence has zero
duration`); a non-looping one with only `t = 0` events fires them and completes
on its first advance.

Base-clip changes always go through `blend_target` (a tracked skeletal entity
never re-reads `clip`); a `duration` of 0 is clamped to 1 ms because the
crossfade path ignores non-positive durations.

#### Previewing a sequence

```bash
flint edit models/starthing.gltf --sequence animations/starthing_showcase.sequence.toml
flint edit models/starthing.gltf --sequence animations/starthing_showcase.sequence.toml \
    --render t3.png --anim-time 3.0        # headless: pose at t = 3 s
```

The bottom panel gains a **Sequence** section: play/pause (P), **Restart**
(R / Home), a **Loop** toggle (`--sequence-loop` sets it from the CLI; ticking it after the end restarts), a seek slider, and a marker strip with one tick per
event (orange = blend, layer colour = layer, grey = speed, cyan = cue; dim =
not yet fired; hover for details, click to seek). Seeking and `--anim-time`
both **replay** the sequence from `t = 0` in 1/120 s steps after restoring the
animator to its pre-sequence state, so the pose at any time is deterministic —
rendering the same `--anim-time` twice yields identical pixels. Cues fired
during playback print `[sequence] cue '<name>' at <t>s`.

#### Sequences at runtime

The player loads every `*.sequence.toml` from the scene's `animations/`
directory (or `../animations/`) and registers it by `name`. Scripts start one
with `play_sequence(entity, name)` and stop it with `stop_sequence(entity)`;
both just write `animator.sequence`, so a scene can also autoplay one:

```toml
[entities.hero.animator]
clip = "Idle"
playing = true
sequence = "intro_bow"
```

When a non-looping sequence ends the engine clears `animator.sequence`, so
`play_sequence` with the same name is a fresh start. Cues reach the owning
entity's script:

```rust
fn on_sequence_cue(sequence, cue) {
    if cue == "done" { play_sequence(self_entity(), "idle_loop"); }
}
```

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
| `layers` | array of tables | [] | Animation layers `{ clip, weight, mode, mask, speed, fade_target, fade_duration }`, composed in order |
| `sequence` | string | "" | Name of a `*.sequence.toml` from the scene's `animations/` directory driving this animator. Set by `play_sequence()`, cleared by the engine when a non-looping sequence finishes |
| `layer_clip` | string | "" | Legacy alias for `layers[0]` (additive, unmasked) |
| `layer_weight` | f32 | 1.0 | Legacy alias for `layers[0].weight` |

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
| `set_anim_layer(entity_id, index, clip, weight)` | Play `clip` on a layer (additive, unmasked) |
| `set_anim_layer_ex(entity_id, index, clip, weight, mode, mask)` | Same with `"additive"`/`"override"` and a root-joint mask |
| `set_anim_layer_weight(entity_id, index, weight)` | Set a layer's weight instantly (cancels a fade) |
| `fade_anim_layer(entity_id, index, weight, seconds)` | Ramp a layer's weight over `seconds` |
| `clear_anim_layer(entity_id, index)` | Leave an inactive slot |
| `play_sequence(entity_id, name)` / `stop_sequence(entity_id)` | Drive the animator from a `*.sequence.toml` |
| `on_sequence_cue(sequence, cue)` | Callback: a sequence passed a `cue` event |

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
