# Particles

Flint's particle system provides GPU-instanced visual effects through the `flint-particles` crate. Fire, smoke, sparks, dust motes, magic effects --- any volumetric visual that needs hundreds or thousands of small, short-lived elements.

## How It Works

Each entity with a `particle_emitter` component owns a **pool** of particles simulated on the CPU and rendered as camera-facing quads via GPU instancing. The pipeline is:

```
TOML component                CPU simulation              GPU rendering
particle_emitter   ──►  ParticleSync reads config  ──►  ParticlePipeline
  emission_rate         spawn/integrate/kill             instanced draw
  gravity               pack into instance buffer        storage buffer
  color_start/end       (swap-remove pool)               alpha or additive
```

Unlike billboard sprites (which are individual ECS entities), particles are **pooled per-emitter** --- a single entity can own thousands of particles without overwhelming the ECS.

## Adding Particles to a Scene

Add a `particle_emitter` component to any entity:

```toml
[entities.campfire]
[entities.campfire.transform]
position = [0, 0.2, 0]

[entities.campfire.particle_emitter]
emission_rate = 40.0
max_particles = 200
lifetime_min = 0.3
lifetime_max = 0.8
speed_min = 1.5
speed_max = 3.0
direction = [0, 1, 0]
spread = 20.0
gravity = [0, 2.0, 0]
size_start = 0.15
size_end = 0.02
color_start = [1.0, 0.7, 0.1, 0.9]
color_end = [1.0, 0.1, 0.0, 0.0]
blend_mode = "additive"
shape = "sphere"
shape_radius = 0.15
autoplay = true
```

## Emission Shapes

The `shape` field controls where new particles spawn relative to the emitter:

| Shape | Fields | Description |
|-------|--------|-------------|
| `point` | (none) | All particles spawn at the emitter origin |
| `sphere` | `shape_radius` | Random position within a sphere |
| `cone` | `shape_angle`, `shape_radius` | Particles emit in a cone around `direction` |
| `box` | `shape_extents` | Random position within an axis-aligned box |

## Blend Modes

| Mode | Use Case | Description |
|------|----------|-------------|
| `alpha` | Smoke, dust, fog | Standard alpha blending --- particles fade naturally |
| `additive` | Fire, sparks, magic | Colors add together --- bright, glowing effects |

Additive blending is order-independent, making it ideal for dense effects. Alpha blending looks best for soft, diffuse effects.

## Value Over Lifetime

Particles interpolate linearly between start and end values over their lifetime:

- **`size_start` / `size_end`** --- particles can grow (smoke expanding) or shrink (sparks dying)
- **`color_start` / `color_end`** --- RGBA transition. Set `color_end` alpha to 0 for fade-out

## Sprite Sheet Animation

For textured particles (flame sprites, explosion frames), use sprite sheets:

```toml
[entities.explosion.particle_emitter]
texture = "explosion_sheet.png"
frames_x = 4
frames_y = 4
animate_frames = true   # Auto-advance frames over particle lifetime
```

With `animate_frames = true`, each particle plays through the sprite sheet from birth to death.

## Bursts and Duration

For one-shot effects (explosions, impacts), combine bursts with limited duration:

```toml
[entities.explosion.particle_emitter]
emission_rate = 0.0      # No continuous emission
burst_count = 50         # 50 particles on each burst
duration = 0.5           # Emitter runs for 0.5 seconds
looping = false          # Don't repeat
autoplay = true          # Fire immediately
```

For periodic bursts (fountain, heartbeat), set `looping = true` with a `duration`.

## Component Schema

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `emission_rate` | f32 | 10.0 | Particles per second (0 = burst-only) |
| `burst_count` | i32 | 0 | Particles fired on each burst/loop start |
| `max_particles` | i32 | 256 | Pool capacity (max 10,000) |
| `lifetime_min` | f32 | 1.0 | Minimum particle lifetime in seconds |
| `lifetime_max` | f32 | 2.0 | Maximum particle lifetime in seconds |
| `speed_min` | f32 | 1.0 | Minimum initial speed |
| `speed_max` | f32 | 3.0 | Maximum initial speed |
| `direction` | vec3 | [0,1,0] | Base emission direction (local space) |
| `spread` | f32 | 15.0 | Random deviation angle in degrees |
| `gravity` | vec3 | [0,-9.81,0] | Acceleration applied per frame (world space) |
| `damping` | f32 | 0.0 | Velocity decay per second |
| `size_start` | f32 | 0.1 | Particle size at birth |
| `size_end` | f32 | 0.0 | Particle size at death |
| `color_start` | vec4 | [1,1,1,1] | RGBA color at birth |
| `color_end` | vec4 | [1,1,1,0] | RGBA color at death |
| `texture` | string | "" | Sprite texture (empty = white dot) |
| `frames_x` | i32 | 1 | Sprite sheet columns |
| `frames_y` | i32 | 1 | Sprite sheet rows |
| `animate_frames` | bool | false | Auto-advance frames over lifetime |
| `blend_mode` | string | "alpha" | `"alpha"` or `"additive"` |
| `shape` | string | "point" | `"point"`, `"sphere"`, `"cone"`, `"box"` |
| `shape_radius` | f32 | 0.5 | Radius for sphere/cone shapes |
| `shape_angle` | f32 | 30.0 | Half-angle for cone shape (degrees) |
| `shape_extents` | vec3 | [0.5,0.5,0.5] | Half-extents for box shape |
| `world_space` | bool | true | Particles detach from emitter transform |
| `duration` | f32 | 0.0 | Emitter duration (0 = infinite) |
| `looping` | bool | true | Loop when duration expires |
| `playing` | bool | false | Current playback state |
| `autoplay` | bool | true | Start emitting on scene load |

## Scripting Integration

Particles can be controlled from [Rhai scripts](scripting.md):

| Function | Description |
|----------|-------------|
| `emit_burst(entity_id, count)` | Fire N particles immediately |
| `start_emitter(entity_id)` | Start continuous emission |
| `stop_emitter(entity_id)` | Stop emission (existing particles finish) |
| `set_emission_rate(entity_id, rate)` | Change emission rate dynamically |

```rust
// Rhai script: burst of sparks on impact
fn on_collision() {
    let me = self_entity();
    emit_burst(me, 30);
}

// Rhai script: toggle emitter with interaction
fn on_interact() {
    let me = self_entity();
    let playing = get_field(me, "particle_emitter", "playing");
    if playing {
        stop_emitter(me);
    } else {
        start_emitter(me);
    }
}
```

## Architecture

- **ParticlePool** --- swap-remove array for O(1) particle death, contiguous alive iteration
- **ParticleSync** --- bridges ECS `particle_emitter` components to the simulation, auto-discovers new emitters each frame
- **ParticleSystem** --- top-level `RuntimeSystem` that ticks simulation in `update()` (variable-rate, not fixed-step)
- **ParticlePipeline** --- wgpu render pipeline with alpha and additive variants, storage buffer for instances

The particle system runs after animation (emitter transforms may be animated) and before the renderer refresh. Instance data is packed contiguously and uploaded to a GPU storage buffer for efficient instanced drawing.

## Recipes

### Fire
```toml
emission_rate = 40.0
gravity = [0, 2.0, 0]
color_start = [1.0, 0.7, 0.1, 0.9]
color_end = [1.0, 0.1, 0.0, 0.0]
blend_mode = "additive"
shape = "sphere"
shape_radius = 0.15
```

### Smoke
```toml
emission_rate = 8.0
gravity = [0, 0.5, 0]
damping = 0.3
size_start = 0.1
size_end = 0.6
color_start = [0.4, 0.4, 0.4, 0.3]
color_end = [0.6, 0.6, 0.6, 0.0]
blend_mode = "alpha"
```

### Sparks
```toml
emission_rate = 15.0
speed_min = 3.0
speed_max = 6.0
spread = 45.0
gravity = [0, -9.81, 0]
size_start = 0.03
size_end = 0.01
color_start = [1.0, 0.9, 0.3, 1.0]
color_end = [1.0, 0.3, 0.0, 0.0]
blend_mode = "additive"
```

### Dust Motes
```toml
emission_rate = 5.0
speed_min = 0.05
speed_max = 0.2
spread = 180.0
gravity = [0, 0.02, 0]
damping = 0.5
size_start = 0.02
size_end = 0.02
color_start = [1.0, 1.0, 0.9, 0.5]
color_end = [1.0, 1.0, 0.9, 0.0]
shape = "box"
shape_extents = [2.0, 1.0, 2.0]
```

## Further Reading

- [Scripting](scripting.md) --- full scripting API including particle functions
- [Animation](animation.md) --- animate emitter transforms with property tweens
- [Rendering](rendering.md) --- the GPU pipeline that draws particles
- [Physics and Runtime](physics-and-runtime.md) --- the game loop that drives particle simulation
