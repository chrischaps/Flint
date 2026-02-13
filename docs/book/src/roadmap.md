# Roadmap

Flint's development is organized into phases. Each phase delivers a usable milestone that builds on the previous one.

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

**Status: Complete**

The game runtime phase. A playable game loop with physics, audio, animation, scripting, and interactive entities.

**Stage 1 --- Game Loop + Physics: Complete**
- `flint-runtime` --- GameClock (fixed-timestep accumulator), InputState (keyboard/mouse with action bindings), EventBus, RuntimeSystem trait
- `flint-physics` --- Rapier 3D integration: PhysicsWorld, PhysicsSync (TOML-to-Rapier bridge), CharacterController (kinematic first-person movement with gravity and jumping)
- `flint-player` --- Standalone player binary with full game loop
- First-person camera mode (backward-compatible with orbit)
- CLI `play` command --- `flint play <scene> [--schemas] [--fullscreen]`
- Physics schemas --- `rigidbody.toml`, `collider.toml`, `character_controller.toml` components + `player.toml` archetype
- Demo scene --- walkable tavern with physics colliders on walls, floor, and furniture

**Stage 2 --- Audio: Complete**
- `flint-audio` --- Kira 0.11 integration: AudioEngine, AudioSync, AudioTrigger, AudioSystem
- Spatial 3D audio with distance attenuation via SpatialTrackHandle
- Non-spatial ambient loops on main track
- Event-driven sound triggers (collision, interaction)
- Audio component schemas --- `audio_source.toml`, `audio_listener.toml`, `audio_trigger.toml`
- Graceful degradation when no audio device available (headless/CI)
- Demo audio --- CC0 OGG assets: fire crackle, ambient tavern, door open, glass clinks

**Stage 3 --- Animation: Complete**
- `flint-animation` --- Two-tier animation system:
  - **Tier 1: Property tweens** --- TOML-defined `.anim.toml` keyframe clips with Step/Linear/CubicSpline interpolation, `animator` component schema, event firing at keyframe times
  - **Tier 2: Skeletal animation** --- glTF skin/joint import via `flint-import`, GPU vertex skinning with storage buffer bone matrices, separate `SkinnedVertex` pipeline, crossfade blending between clips
- `skeleton` component schema for glTF skin references
- Skinned shadow mapping with dedicated shader entry point
- Demo animations --- bobbing platform (4s loop), door swing (0.8s), skeletal test scene

**Stage 4 --- Scripting: Complete**
- `flint-script` --- Rhai scripting engine with per-entity scopes and AST management
- Entity API (read/write components, spawn/despawn, position/rotation, distance)
- Input API (action pressed/just-pressed, mouse delta)
- Audio API (play_sound, play_sound_at, stop_sound) via deferred ScriptCommand pattern
- Animation API (play_clip, stop_clip, blend_to, set_anim_speed) via direct ECS writes
- Math API (clamp, lerp, random, trig, atan2)
- Event callbacks: `on_init`, `on_update`, `on_collision`, `on_trigger_enter/exit`, `on_action`, `on_interact`
- Hot-reload via file timestamp checking (keeps old AST on compile error)
- `script` component schema with `source` and `enabled` fields

**Stage 5 --- Integration: Complete**
- `interactable` component schema (prompt_text, range, interaction_type, enabled)
- Proximity-based interaction with `find_nearest_interactable()` scanning
- egui HUD overlay: crosshair + interaction prompt text with fade in/out
- NPC behavior scripts: bartender (wave + glass clink), patron (random fidget), mysterious stranger (ominous reactions)
- Footstep sounds synced to player movement
- Ambient event system (random sounds: glass clinks, chair creaks)
- Full atmospheric tavern integration demo with scripts, audio, animation, and interactables

**Milestone:** `flint play tavern.scene.toml` launches a first-person walkable scene with physics, spatial audio, animation, scripted NPCs, and interactive objects.

## Phase 5: AI Asset Pipeline

**Status: Complete**

Integrated AI generation workflows for textures, meshes, and audio with style consistency and provenance tracking.

**Delivered:**
- `flint-asset-gen` --- pluggable `GenerationProvider` trait with four implementations:
  - **Flux** --- AI texture generation (PNG output)
  - **Meshy** --- text-to-3D model generation (GLB output, async job polling)
  - **ElevenLabs** --- AI sound effect and voice generation
  - **Mock** --- generates minimal valid files for testing without network access
- **Style guides** --- TOML-defined visual vocabulary (palette, materials, geometry constraints) that enriches generation prompts for consistent asset aesthetics
- **Semantic asset definitions** --- `asset_def` component schema mapping intent to generation requests (description, material intent, wear level, size class)
- **Batch scene resolution** --- `flint asset resolve` with strategies: `ai_generate`, `human_task`, `ai_then_human`
- **Model validation** --- `validate_model()` checks GLB geometry and materials against style constraints (triangle count, UVs, normals, roughness/metallic ranges)
- **Build manifests** --- provenance tracking for all generated assets (provider, prompt, content hash)
- **Layered configuration** --- `~/.flint/config.toml` < `.flint/config.toml` < environment variables for API keys and provider settings
- **Runtime catalog integration** --- `PlayerApp` resolves assets by name through catalog → hash → content store → file fallback chain
- **CLI commands** --- `flint asset generate`, `flint asset validate`, `flint asset manifest`, `flint asset regenerate`, `flint asset job status/list`

**Milestone:** `flint asset generate texture -d "stone wall" --style medieval_tavern` produces a style-consistent texture, validates it, and stores it in the content-addressed catalog.

## Phase A: Doom-Style FPS

**Status: Complete**

A minimum playable Doom-style first-person shooter, demonstrating the engine's capability for real-time action gameplay with billboard sprites, raycasting combat, and game-level logic entirely in scripts.

**Delivered:**
- **Billboard sprite rendering** --- `BillboardPipeline` with camera-facing quads, sprite sheet animation, binary alpha via `discard`, per-sprite uniform buffers. `sprite` component schema (texture, width, height, frame, frames_x/y, anchor_y, fullbright, visible)
- **Raycasting** --- `PhysicsWorld::raycast()` with `EntityRaycastHit` struct, collider-to-entity resolution, self-exclusion. Exposed to Rhai scripts as `raycast()`, `get_camera_direction()`, `get_camera_position()`
- **Script-driven HUD** --- `DrawCommand` pipeline with text, rect, circle, line, and sprite primitives. `on_draw_ui()` callback, `screen_width()`/`screen_height()`, `measure_text()`, `find_nearest_interactable()`. All game UI lives in `.rhai` scripts, not engine code
- **Mouse button action bindings** --- `mouse_button_map` alongside keyboard `action_map`; `fire` bound to left mouse button by default. `weapon_1`/`weapon_2`/`reload` actions added
- **Game project pattern** --- `games/<name>/` directory structure with own schemas/scripts/scenes/assets. `--schemas` flag accepts multiple paths with later-path-wins priority
- **Multi-directory schema loading** --- `SchemaRegistry::load_from_multiple_dirs()` merges engine and game schemas
- **Enemy AI state machine** --- idle/chase/attack/dead states with line-of-sight checks, patrol patterns, and damage response (all in Rhai scripts)
- **Health/ammo pickups** --- pickup component with collect-on-proximity logic in scripts
- **Combat HUD** --- crosshair, health bar, ammo counter, damage flash overlay, weapon name display, interaction prompts (all script-driven via `hud.rhai`)
- **Additional script APIs** --- `this_entity()` alias, `get_component()`, `play_sound(name, volume)` overload, `log_info()` alias

**Milestone:** From the Doom FPS game repo: `.\play.bat fps_arena` launches a playable FPS with enemies, weapons, pickups, and a full combat HUD. The game repo includes the engine as a git subtree.

## Beyond Phase A

These are ideas under consideration, not committed plans. See the Doom FPS game repository's `DOOM_FPS_GAPS.md` for remaining gaps toward a feature-complete Doom clone.

- **Networking** --- multiplayer support
- **Post-processing** --- bloom, ambient occlusion, tone mapping, LOD
- **Plugin system** --- third-party extensions
- **Package manager** --- share schemas, constraints, and assets between projects
- **WebAssembly** --- browser-based viewer and potentially runtime
