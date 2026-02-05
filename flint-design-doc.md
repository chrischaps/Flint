# Flint Engine Design Document

**Version:** 0.1 (Draft)  
**Date:** February 2026  
**Status:** Pre-development

---

## Executive Summary

Flint is a general-purpose 3D game engine designed from the ground up to provide an excellent interface for AI coding agents, while maintaining effective workflows for human developers. Unlike existing engines (Unity, Unreal, Godot) that optimize for GUI-driven workflows, Flint prioritizes programmatic interaction, introspection, and validation.

### Core Thesis

Current game engines are built around visual editors, drag-and-drop workflows, and GUI-heavy tooling. These become friction points when AI agents attempt to make changes programmatically—the agent ends up fighting against abstractions designed for human spatial reasoning and visual feedback loops.

Flint inverts this: the primary interface is CLI and code, with visual tools focused on *validating* results rather than *creating* them.

### Design Principles

1. **CLI-first** — Every operation expressible as a composable command
2. **Introspectable** — Query any aspect of engine state as structured data
3. **Deterministic** — Same inputs always produce identical outputs
4. **Text-based** — Scene and asset formats are human/machine readable, diffable
5. **Constraint-driven** — Declarative rules the engine validates and optionally enforces
6. **Hybrid workflows** — Humans and AI agents collaborate effectively

---

## Architecture Overview

### Crate Structure

```
flint/
├── crates/
│   ├── flint-core/           # Fundamental types, IDs, content hashing
│   ├── flint-ecs/            # ECS layer (wraps hecs or bevy_ecs standalone)
│   ├── flint-schema/         # Component schemas, introspection registry
│   ├── flint-query/          # Query language parser and executor
│   ├── flint-asset/          # Content-addressed asset system
│   ├── flint-scene/          # Scene format, serialization (TOML/RON)
│   ├── flint-constraint/     # Constraint definitions and solver
│   ├── flint-render/         # wgpu-based renderer
│   ├── flint-animation/      # Animation playback (property tweens + skeletal)
│   ├── flint-physics/        # Rapier integration
│   ├── flint-audio/          # Kira integration
│   ├── flint-script/         # Rhai scripting integration
│   ├── flint-cli/            # CLI application
│   └── flint-viewer/         # Minimal GUI for human validation
│
├── tools/
│   ├── flint-asset-gen/      # AI generation provider integrations
│   └── flint-import/         # Converters from common formats (glTF, FBX, etc.)
│
└── runtime/
    └── flint-player/         # Standalone game player/executable
```

### Technology Choices

| Component | Technology | Rationale |
|-----------|------------|-----------|
| Language | Rust | Performance, safety, excellent ecosystem for games |
| Scripting | Rhai | Rust-native, statically typed, sandboxed, good error messages |
| Rendering | wgpu | Cross-platform, modern API, Rust-native |
| Physics | Rapier | Rust-native, deterministic, well-maintained |
| Audio | Kira | Rust-native, game-focused, good API |
| ECS | hecs or bevy_ecs | Standalone, well-tested, introspectable |
| Scene format | TOML | Human-readable, diffable, good Rust support |
| Asset metadata | TOML/RON | Consistent with scene format |

### Data Flow

```
                    ┌─────────────────────────────────────────┐
                    │              flint-cli                  │
                    │   (human or agent issues commands)      │
                    └─────────────────┬───────────────────────┘
                                      │
                    ┌─────────────────▼───────────────────────┐
                    │             flint-query                 │
                    │   (parse commands, route to subsystems) │
                    └─────────────────┬───────────────────────┘
                                      │
        ┌─────────────┬───────────────┼───────────────┬───────────────┐
        ▼             ▼               ▼               ▼               ▼
   ┌──────────┐ ┌───────────┐  ┌───────────┐  ┌────────────┐  ┌─────────────┐
   │flint-    │ │flint-     │  │ flint-ecs │  │flint-      │  │flint-       │
   │scene     │◄┤asset      │◄─┤           │◄─┤schema      │─►│constraint   │
   └────┬─────┘ └─────┬─────┘  └─────┬─────┘  └────────────┘  └──────┬──────┘
        │             │              │                               │
        │             │              │         ┌─────────────────────┘
        │             │              │         │ (validates/fixes)
        ▼             ▼              ▼         ▼
   ┌─────────────────────────────────────────────────────────┐
   │                 flint-core (World State)                │
   └─────────────────────────────────────────────────────────┘
                                      │
        ┌─────────────┬───────────────┼───────────────┬───────────────┐
        ▼             ▼               ▼               ▼               ▼
   ┌──────────┐ ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌────────────┐
   │flint-    │ │flint-     │  │flint-     │  │flint-     │  │flint-      │  │flint-      │
   │render    │ │physics    │  │animation  │  │audio      │  │script      │  │viewer      │
   │(wgpu)    │ │(rapier)   │  │(keyframe) │  │(kira)     │  │(rhai)      │  │(validation)│
   └──────────┘ └───────────┘  └───────────┘  └───────────┘  └────────────┘  └────────────┘
```

---

## Core Systems

### 1. CLI Interface (flint-cli)

The CLI is the primary interface for both humans and AI agents.

#### Basic Usage

```bash
# Project management
flint new my-game
flint build --release
flint serve --watch

# Scene operations
flint scene create levels/tavern.scene
flint scene list
flint scene validate levels/tavern.scene

# Entity operations
flint entity create --archetype door --name "front_door"
flint entity link front_door --connects "room.foyer" "exterior.porch"
flint entity list --scene levels/tavern.scene

# Asset operations
flint asset import models/chair.glb
flint asset list --type mesh
flint asset resolve --strategy placeholder
```

#### Structured I/O

All commands output JSON/TOML by default for machine consumption:

```bash
flint scene list --format json | \
  jq '.entities[] | select(.archetype == "door")' | \
  flint batch modify --set "material_style=rusty_metal"
```

#### Transaction Mode

For multi-step atomic operations:

```bash
flint transaction begin
flint entity create --archetype door --name "vault_door"
flint entity link vault_door --connects "room.vault" "room.corridor"
flint transaction commit  # or rollback
```

#### Watch Mode

For hybrid workflows where a human validates in real-time:

```bash
flint serve --watch ./project
# Hot-reloads on any change, human sees results in viewer
```

---

### 2. Query System (flint-query)

The engine's internal object model is queryable as data.

#### Query Examples

```bash
# Find entities by criteria
flint query "entities where archetype == 'door'"
flint query "entities where materials contains 'brick_worn'"

# Dependency analysis
flint deps texture/brick_diffuse.png --reverse
# Returns: 12 materials reference this texture

# Impact analysis
flint impact --dry-run "delete texture/brick_diffuse.png"
# Returns: 12 materials, 47 entities would be affected

# Schema introspection
flint schema entity.door
# Returns: archetype definition, required fields, optional fields, valid relationships
```

#### Query Language

A simple, SQL-inspired query language:

```
entities where <condition>
assets where <condition>
components where <condition>

<condition> := <field> <op> <value>
            | <condition> and <condition>
            | <condition> or <condition>
            | not <condition>

<op> := == | != | < | > | <= | >= | contains | matches
```

---

### 3. Schema System (flint-schema)

All components are introspectable with a schema registry.

#### Component Definition

```toml
# schemas/components/door.toml
[component.door]
description = "A door that can connect spaces"

[component.door.fields]
style = { type = "enum", values = ["hinged", "sliding", "rotating"], default = "hinged" }
locked = { type = "bool", default = false }
required_key = { type = "entity_ref", optional = true }
open_angle = { type = "f32", default = 90.0, min = 0.0, max = 180.0 }

[component.door.relationships]
connects = { type = "entity_ref[]", min_count = 1, max_count = 2 }
```

#### Archetype Definition

Archetypes bundle components with defaults:

```toml
# schemas/archetypes/door.toml
[archetype.door]
description = "A standard door entity"
components = ["transform", "mesh", "collider", "door", "interactable"]

[archetype.door.defaults]
mesh = "meshes/door_standard.glb"
collider = { type = "from_mesh" }
interactable.interaction = "door_toggle"
```

---

### 4. Scene Format (flint-scene)

Scenes are TOML files describing entity hierarchies and relationships.

#### Basic Scene

```toml
# scenes/levels/tavern.scene.toml
[scene]
name = "Tavern Ground Floor"
version = "1.0"

[entity.tavern_root]
transform = { position = [0, 0, 0] }

[entity.main_room]
parent = "tavern_root"
archetype = "room"
bounds = { min = [0, 0, 0], max = [10, 4, 8] }
material_style = "medieval_tavern"

[entity.front_door]
parent = "main_room"
archetype = "door"
transform = { position = [5, 0, 0], rotation = [0, 90, 0] }
connects = ["main_room", "exterior"]
material_style = "worn_wood"

[entity.bar_counter]
parent = "main_room"
archetype = "furniture"
mesh = "meshes/bar_counter.glb"
transform = { position = [8, 0, 4] }
```

#### Semantic Descriptions

Entities can be defined by intent, resolved later:

```toml
[entity.front_door]
archetype = "door"
connects = ["room.foyer", "exterior.porch"]
material_style = "worn_wood"
interaction = "openable"
# Engine resolves to actual geometry, collision, animation
```

---

### 5. Constraint System (flint-constraint)

Declarative rules the engine validates and optionally enforces.

#### Constraint Definition

```toml
# constraints/doors.toml
[[constraint]]
name = "doors_have_connections"
description = "Every door must connect at least one space"
query = "entities where archetype == 'door'"
rule = "connections.length >= 1"
severity = "error"
message = "Door '{name}' must connect at least one room"

[[constraint]]
name = "doors_have_handles"
description = "Non-sliding doors should have handles"
query = "entities where archetype == 'door' and style != 'sliding'"
rule = "children contains archetype 'handle'"
severity = "warning"
message = "Door '{name}' is missing a handle"

[constraint.auto_fix]
enabled = true
strategy = "add_child"
archetype = "handle"
placement = "door_handle_socket"
defaults = { style = "inherit" }
```

#### Validation CLI

```bash
# Just validate
flint validate ./scenes/level1.scene

# Show what fixes would be applied
flint validate ./scenes/level1.scene --fix --dry-run

# Apply fixes
flint validate ./scenes/level1.scene --fix

# Output diff of changes (for agent verification)
flint validate ./scenes/level1.scene --fix --output-diff
```

#### Constraint Composition

Fixes can trigger cascading validations:

```
Door missing handle → auto-add handle →
handle requires material → auto-assign based on door style →
validate material exists → ...
```

The system includes cycle detection and clear execution ordering.

---

### 6. Asset System (flint-asset)

Content-addressed, introspectable, with multiple resolution strategies.

#### Content Addressing

Assets are referenced by hash of their contents:

```toml
[asset.brick_texture]
hash = "sha256:a1b2c3d4..."
path = "textures/brick_diffuse.png"
type = "texture"
format = "png"
dimensions = [1024, 1024]
```

Benefits:
- Automatic deduplication
- Cache invalidation that works
- Diffable history ("what changed between builds?")
- Reproducible builds by locking hashes

#### Semantic Asset Definitions

Assets can be defined by intent:

```toml
[asset.wooden_chair]
type = "furniture"
style = "medieval_tavern"
material = "oak_wood"
wear_level = 0.6
size_class = "standard"

# Resolved by strategy: library match, AI generation, placeholder, or human artist
```

#### Resolution Strategies

```bash
# Use AI generation for missing assets
flint resolve --strategy ai-generate

# Use placeholders (colored boxes with labels)
flint resolve --strategy placeholder

# Fail if any asset unresolved
flint resolve --strategy strict

# Create tasks for human artists
flint resolve --strategy human-task --output-dir ./tasks
```

#### AI Generation Integration

The engine provides hooks to pluggable providers:

```bash
# Generate texture
flint asset generate texture \
    --type diffuse \
    --description "weathered red brick, mossy in corners" \
    --size 1024 \
    --provider flux \
    --output textures/brick_mossy.png

# Generate 3D model
flint asset generate model \
    --archetype "chair" \
    --style "medieval_tavern" \
    --provider meshy \
    --output models/tavern_chair.glb
```

Providers are pluggable—the engine defines the interface, users configure which services to use.

#### Style Consistency

Style guides as first-class engine objects:

```toml
# styles/medieval_tavern.toml
[palette]
primary_wood = "#8B4513"
secondary_wood = "#A0522D"
metal_accent = "#4A4A4A"
wear_intensity = [0.4, 0.7]  # range

[materials]
wood.roughness = [0.7, 0.9]
metal.roughness = [0.3, 0.5]

[geometry]
edge_wear = true
perfect_symmetry = false
```

Validation against style:

```bash
flint asset validate chair.glb --style medieval_tavern
# Warning: metal roughness 0.2 below style minimum 0.3
```

---

### 7. Scripting System (flint-script)

Rhai-based scripting for game logic.

#### Why Rhai

- **Static typing with inference** — Clear error messages for agents
- **No null** — Explicit `Option` types like Rust
- **Sandboxed** — Scripts can only call explicitly exposed functions
- **Rust-native** — Seamless integration, no FFI overhead
- **Good errors** — Line/column info for debugging

#### Example Script

```javascript
// scripts/door_interaction.rhai

fn on_door_interact(door, player) {
    let is_locked = door.get_component("lock")?.locked;
    
    if is_locked {
        let has_key = player.inventory.contains(door.required_key);
        if has_key {
            door.unlock();
            door.open();
            audio::play("door_unlock");
        } else {
            ui::show_message("This door is locked.");
            audio::play("door_locked");
        }
    } else {
        door.toggle_open();
        audio::play(if door.is_open { "door_open" } else { "door_close" });
    }
}

fn on_door_proximity(door, player, distance) {
    if distance < 2.0 {
        ui::show_prompt("Press E to open");
    }
}
```

#### Exposed Engine APIs

```rust
// Rust side: expose APIs to scripts
engine.register_fn("spawn_entity", |archetype: &str, name: &str| { ... });
engine.register_fn("get_component", |entity: Entity, comp: &str| { ... });
engine.register_fn("play_sound", |sound: &str| { ... });
engine.register_fn("show_message", |msg: &str| { ... });
```

---

### 8. Animation System (flint-animation)

Two-tier animation supporting both code-defined property tweens and imported skeletal animation from glTF.

#### Design Goals

- **Data-driven** — Animations are assets, not code. Simple tweens definable in TOML, complex skeletal clips imported from glTF.
- **Deterministic** — Same clip + time = same pose. Animation advances by GameClock delta, not wall time.
- **GPU-accelerated** — Skeletal skinning runs in the vertex shader. CPU evaluates keyframes; GPU applies bone matrices.
- **Composable** — Clips can be blended, layered, and sequenced. Scripts and events can trigger transitions.

#### Tier 1: Property Animation

Animate any transform property over time using keyframe tracks:

```toml
# Inline animation defined in a scene file
[entities.front_door.animator]
clip = "door_open"
autoplay = false
loop = false
speed = 1.0

# animations/door_open.anim.toml
[animation]
name = "door_open"
duration = 0.8

[[animation.tracks]]
target = "rotation"
interpolation = "cubic_spline"
keyframes = [
    { time = 0.0, value = [0, 0, 0] },
    { time = 0.8, value = [0, 90, 0] },
]

[[animation.events]]
time = 0.05
event = "door_creak"
```

Property animations cover the majority of game animation needs: doors, platforms, elevators, UI elements, color shifts, light flicker. No glTF file required.

#### Tier 2: Skeletal Animation

Full bone-based animation for characters and complex meshes:

```
glTF file
  ├── Skin (joint hierarchy, inverse bind matrices)
  ├── Mesh (positions, normals, UVs, joint_indices, joint_weights)
  └── Animations (per-joint translation/rotation/scale channels)
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

Vertex shader skinning:

```wgsl
// Added to VertexInput
@location(4) joint_indices: vec4<u32>,
@location(5) joint_weights: vec4<f32>,

// Bone matrix buffer
@group(2) @binding(0)
var<storage, read> bone_matrices: array<mat4x4<f32>>;

// Skinning calculation
fn skin_position(pos: vec3<f32>, joints: vec4<u32>, weights: vec4<f32>) -> vec3<f32> {
    let m = bone_matrices[joints.x] * weights.x
          + bone_matrices[joints.y] * weights.y
          + bone_matrices[joints.z] * weights.z
          + bone_matrices[joints.w] * weights.w;
    return (m * vec4<f32>(pos, 1.0)).xyz;
}
```

#### Animation Blending

Smooth transitions between clips:

- **Crossfade** — Linearly blend from clip A to clip B over a duration. Useful for walk → run, idle → attack.
- **Additive** — Layer a partial clip on top of a base (wave hand while walking). The additive clip stores deltas from a reference pose.
- **Blend tree** — (future) Parameterized blending based on movement speed, direction, etc.

#### Component Schema

```toml
# schemas/components/animator.toml
[component.animator]
description = "Controls animation playback for an entity"

[component.animator.fields]
clip = { type = "string", description = "Current animation clip name", default = "" }
playing = { type = "bool", default = false }
loop = { type = "bool", default = true }
speed = { type = "f32", default = 1.0, min = -10.0, max = 10.0 }
time = { type = "f32", default = 0.0, min = 0.0, description = "Current playback position in seconds" }
blend_target = { type = "string", optional = true, description = "Clip to crossfade into" }
blend_duration = { type = "f32", default = 0.3, min = 0.0, description = "Crossfade duration in seconds" }
```

#### CLI Integration

```bash
# List animations available on an entity
flint query "entities where animator.clip != ''"

# Inspect animation state
flint query "entities where animator.playing == true"

# Preview an animation clip asset
flint asset list --type animation
```

---

### 10. Determinism

Ensuring reproducible builds and testable results.

#### Seeded Randomness

All procedural generation uses explicit seeds:

```toml
[asset.forest_zone]
generator = "proc.forest"
seed = 0xDEADBEEF
density = 0.7
```

Same seed + parameters = identical output, forever.

#### Build Manifests

Every build produces a manifest:

```toml
# build/manifest.toml
[build]
timestamp = "2026-02-01T12:00:00Z"
flint_version = "0.1.0"

[assets]
"textures/brick.png" = "sha256:a1b2c3..."
"meshes/door.glb" = "sha256:d4e5f6..."

[scenes]
"levels/tavern.scene" = "sha256:789abc..."
```

An agent can say "reproduce build X" and get bit-identical results.

---

### 9. Rendering (flint-render) (Updated: now includes skinned mesh pipeline — see Animation System)

wgpu-based renderer targeting indie-level fidelity.

#### Initial Scope

- PBR materials (metallic-roughness workflow)
- Point, spot, and directional lights
- Shadow mapping
- Basic post-processing (tone mapping, gamma correction)
- glTF import

#### Future Considerations

- Skinned mesh rendering pipeline (see Animation System, Stage 3)
- Screen-space ambient occlusion
- Bloom
- Simple global illumination (light probes)
- LOD system

#### Headless Mode

Rendering can run headless for CI/validation:

```bash
flint render --headless --output frame.png scenes/level1.scene
flint render --headless --output-diff baseline.png current.png
```

---

### 11. Viewer (flint-viewer)

Minimal GUI for human validation, not content creation.

#### Features

- Real-time scene view
- Entity inspector (read-only, or simple property tweaks)
- Constraint violation overlay
- Visual diff mode (compare two scene states)
- Screenshot/recording for documentation

#### Non-Goals

- Full scene editor
- Asset creation tools
- Complex node-based workflows

The viewer answers: "Did the agent do what I asked?" not "Let me build this myself."

---

## Development Phases

### Phase 1: Foundation (CLI + Query + Schema)

**Goal:** Agent can create, query, and modify scenes via CLI—no rendering.

**Deliverables:**
- flint-core: Entity IDs, content hashing, basic types
- flint-schema: Component registry, introspection
- flint-ecs: ECS integration with schema awareness
- flint-scene: TOML serialization/deserialization
- flint-query: Basic query language
- flint-cli: CRUD operations for entities and scenes

**Milestone:** `flint entity create --archetype door` works, `flint query "entities"` returns results.

### Phase 2: Constraints + Assets

**Goal:** Agent can build valid scenes with real assets.

**Deliverables:**
- flint-constraint: Validation rules, auto-fix system
- flint-asset: Content addressing, resolution strategies
- flint-import: glTF importer

**Milestone:** `flint validate --fix` automatically adds missing door handles.

### Phase 3: Rendering + Validation

**Goal:** Human can see what the agent built.

**Deliverables:**
- flint-render: Basic PBR renderer
- flint-viewer: Minimal validation GUI
- Headless rendering for CI

**Milestone:** `flint serve --watch` shows live scene, hot-reloads on changes.

### Phase 4: Runtime

**Goal:** Playable game loop with physics, animation, audio, and scripting.

**Deliverables:**
- flint-physics: Rapier integration (Stage 1 — complete)
- flint-runtime: Game loop, input, event bus (Stage 1 — complete)
- flint-player: Standalone executable (Stage 1 — complete)
- flint-audio: Kira integration (Stage 2)
- flint-animation: Property tweens + skeletal animation from glTF (Stage 3)
- flint-script: Rhai scripting with animation/audio APIs (Stage 4)
- Integration demo: Animated, interactive tavern scene (Stage 5)

**Milestone:** Walk around a tavern, open animated doors, see NPC idle animations, hear sounds.

### Phase 5: AI Asset Pipeline

**Goal:** Integrated AI generation workflow.

**Deliverables:**
- flint-asset-gen: Provider integrations (texture, mesh, audio)
- Style consistency validation
- Human task generation for fallback

**Milestone:** `flint asset generate model --provider meshy` produces usable assets.

---

## Open Questions

### Technical

1. **ECS choice:** hecs vs bevy_ecs standalone. hecs is simpler; bevy_ecs has more features and ecosystem momentum.

2. **Query language complexity:** Start minimal (equality, contains) or include more operators (regex, nested queries) from the start?

3. **Hot reload granularity:** Scene-level? Entity-level? Component-level?

4. **Networking:** Not yet addressed. Needed for multiplayer games. Defer to Phase 5+?

### Product

1. **Licensing:** Open source? Dual license? This affects ecosystem growth.

2. **Documentation strategy:** API docs + tutorials? Interactive examples?

3. **Community:** Discord? GitHub discussions? How to gather feedback?

---

## Appendix: Comparison with Existing Engines

| Aspect | Unity | Unreal | Godot | Flint |
|--------|-------|--------|-------|-------|
| Primary interface | GUI editor | GUI editor | GUI editor | CLI |
| Scene format | Binary (YAML available) | Binary | Text (tscn) | TOML |
| Programmatic API | Yes (secondary) | Yes (secondary) | Yes (secondary) | Primary |
| Introspection | Limited | Limited | Moderate | Full |
| Deterministic builds | No | No | Partial | Yes |
| AI-agent optimized | No | No | No | Yes |
| Constraint system | None | Blueprints (visual) | None | Declarative |

---

## Appendix: Example Agent Workflow

A coding agent building a tavern scene:

```bash
# 1. Create the scene
flint scene create levels/tavern.scene

# 2. Add the root entity
flint entity create --scene levels/tavern.scene \
    --name "tavern_root" \
    --transform '{"position": [0,0,0]}'

# 3. Add the main room
flint entity create --scene levels/tavern.scene \
    --archetype room \
    --name "main_room" \
    --parent "tavern_root" \
    --props '{"bounds": {"min": [0,0,0], "max": [10,4,8]}}'

# 4. Add a door
flint entity create --scene levels/tavern.scene \
    --archetype door \
    --name "front_door" \
    --parent "main_room" \
    --props '{"connects": ["main_room", "exterior"]}'

# 5. Validate and auto-fix
flint validate levels/tavern.scene --fix --output-diff

# 6. Verify the result
flint query --scene levels/tavern.scene "entities where archetype == 'door'"

# 7. Render a preview
flint render --headless --output preview.png levels/tavern.scene
```

The agent can verify each step succeeded, inspect the diff of auto-fixes, and produce a visual artifact for human review.

---

*This is a living document. Update as design evolves.*
