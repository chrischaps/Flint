# Roadmap

Flint's development is organized into five phases. Each phase delivers a usable milestone that builds on the previous one.

## Phase 1: Foundation --- CLI + Query + Schema

**Status: Complete**

The foundation phase established the core data model and CLI interface. An agent (or human) can create, query, and modify scenes entirely through commands.

**Delivered:**
- `flint-core` --- Entity IDs, content hashing, fundamental types
- `flint-schema` --- Component registry, archetype definitions, TOML-based introspection
- `flint-ecs` --- hecs integration with stable IDs, named entities, parent-child hierarchy
- `flint-scene` --- TOML scene serialization and deserialization
- `flint-query` --- PEG query language with pest parser
- `flint-cli` --- CRUD operations for entities and scenes

**Milestone:** `flint entity create --archetype door` works. `flint query "entities"` returns results.

## Phase 2: Constraints + Assets

**Status: Complete**

The validation and asset management phase. Scenes can now be checked against declarative rules, and external files can be imported into a content-addressed store.

**Delivered:**
- `flint-constraint` --- Constraint definitions, validation engine, auto-fix with cascade detection
- `flint-asset` --- Content-addressed storage (SHA-256), asset catalog with name/hash/type/tag indexing
- `flint-import` --- glTF/GLB importer with mesh, material, and texture extraction

**Milestone:** `flint validate --fix` automatically fixes constraint violations. `flint asset import model.glb` stores and catalogs assets.

## Phase 3: Rendering + Visual Validation

**Status: Complete**

The visual validation phase. Physically-based rendering with a full-featured scene viewer.

**Delivered:**
- `flint-render` --- wgpu 23 PBR renderer with Cook-Torrance shading, cascaded shadow mapping, and glTF mesh rendering
- `flint-viewer` --- egui-based GUI inspector with entity tree, component editing, and constraint overlay
- Scene viewer with orbit camera, hot-reload via `serve --watch`
- Headless rendering for CI (`flint render --headless`)
- Material system with roughness, metallic, emissive, and texture support

**Milestone:** `flint serve --watch` shows a live PBR scene with shadows that updates when files change.

## Phase 4: Interactive Runtime

**Status: In Progress (Stage 1 of 4 complete)**

The game runtime phase. A playable game loop with physics, and eventually audio and scripting.

**Stage 1 --- Game Loop + Physics: Complete**
- `flint-runtime` --- GameClock (fixed-timestep accumulator), InputState (keyboard/mouse with action bindings), EventBus, RuntimeSystem trait
- `flint-physics` --- Rapier 3D integration: PhysicsWorld, PhysicsSync (TOML-to-Rapier bridge), CharacterController (kinematic first-person movement with gravity and jumping)
- `flint-player` --- Standalone player binary with full game loop
- First-person camera mode (backward-compatible with orbit)
- CLI `play` command --- `flint play <scene> [--schemas] [--fullscreen]`
- Physics schemas --- `rigidbody.toml`, `collider.toml`, `character_controller.toml` components + `player.toml` archetype
- Demo scene --- walkable tavern with physics colliders on walls, floor, and furniture

**Stage 2 --- Audio: Planned**
- `flint-audio` --- Kira integration for spatial audio and sound effects
- 3D positioned sounds with distance attenuation
- Ambient loops and sound trigger events

**Stage 3 --- Scripting: Planned**
- `flint-script` --- Rhai scripting for game logic (sandboxed)
- Entity API, event callbacks (`on_collision`, `on_trigger`, `on_action`)
- Hot-reload for script files

**Stage 4 --- Integration: Planned**
- Interactable component with scripted behaviors
- Full demo: walk around, open doors, hear sounds

**Milestone:** `flint play tavern.scene.toml` launches a first-person walkable scene with physics.

## Phase 5: AI Asset Pipeline

**Status: Planned**

Integrated AI generation workflows for textures, meshes, and audio.

**Planned:**
- `flint-asset-gen` --- Provider integrations (texture generation, mesh generation, audio generation)
- Style consistency validation against style guide TOML files
- Human task generation for assets that need manual creation
- Resolution strategy: `ai_generate` alongside existing `strict` and `placeholder`

**Milestone:** `flint asset generate model --provider meshy` produces usable game assets.

## Beyond Phase 5

These are ideas under consideration, not committed plans:

- **Networking** --- multiplayer support
- **Post-processing** --- bloom, ambient occlusion, tone mapping, LOD
- **Plugin system** --- third-party extensions
- **Package manager** --- share schemas, constraints, and assets between projects
- **WebAssembly** --- browser-based viewer and potentially runtime
