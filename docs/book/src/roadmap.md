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

## Phase 3: Rendering + Validation

**Status: Complete (basic)**

The visual validation phase. Humans can now see what the agent built.

**Delivered:**
- `flint-render` --- wgpu-based renderer with archetype-colored boxes
- Scene viewer with orbit camera, hot-reload via `serve --watch`
- Headless rendering for CI (`flint render --headless`)

**Milestone:** `flint serve --watch` shows a live scene that updates when files change.

## Phase 4: Runtime

**Status: Planned**

The game runtime phase. A playable game loop with physics, audio, and scripting.

**Planned:**
- `flint-physics` --- Rapier integration for collision detection and rigid body simulation
- `flint-audio` --- Kira integration for spatial audio and sound effects
- `flint-script` --- Rhai scripting for game logic (sandboxed, statically typed)
- `flint-player` --- Standalone game executable

**Milestone:** A simple game runs --- walk around, open doors, hear sounds.

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
- **Advanced rendering** --- PBR materials, shadow mapping, post-processing, LOD
- **Viewer GUI** --- entity inspector, constraint violation overlay, visual diff mode
- **Plugin system** --- third-party extensions
- **Package manager** --- share schemas, constraints, and assets between projects
- **WebAssembly** --- browser-based viewer and potentially runtime
