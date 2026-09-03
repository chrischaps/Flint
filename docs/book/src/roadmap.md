# Roadmap

Flint has a solid foundation — PBR rendering, physics, audio, animation layers, scripting, particles, terrain and grass, ocean and sky, post-processing, music sessions, AI asset generation, and shipped game projects (a Doom-style FPS, FlintKart, Starchild). The roadmap now focuses on the features needed to ship production games.

## ~~Visual Scene Tweaking~~ Done

Flint's core thesis is that scenes are *authored* by AI agents and code — not by dragging objects around a viewport. But AI-generated layouts often need human nudges. `flint edit` now provides that adjustment layer: translate / rotate / scale gizmos (W/E/R), a property inspector, TOML write-back that preserves the authored structure, undo / redo, and the F4 Rendering & Effects menu for tuning post-processing and lighting levers live before committing them to the scene file.

## Frustum Culling & Level of Detail

**Priority: High**

Without visibility culling, every object renders every frame regardless of whether it's on screen. This is the performance ceiling that blocks larger scenes.

- BVH spatial acceleration structure
- Frustum culling (skip off-screen objects entirely)
- Mesh LOD switching by camera distance
- Optional texture streaming for large worlds

> Partly delivered. The renderer extracts a camera frustum every frame
> (`flint-render/src/frustum.rs`) and culls terrain chunks by AABB against it;
> grass has its own distance fade. Entity meshes are still drawn
> unconditionally, so per-object culling and LOD remain open.

## Navigation Mesh & Pathfinding

**Priority: High**

Every game with NPCs needs this. Currently enemies can only do simple raycast-based movement — entire genres are blocked without proper pathfinding.

- Nav mesh generation from scene geometry
- A\* pathfinding with dynamic obstacle avoidance
- Script API: `find_path(from, to)`, `move_along_path()`
- Optional crowd simulation (RVO) for dense NPC scenes

## Coroutines & Async Scripting

**Priority: High**

Rhai scripts today are strictly synchronous per-frame. There's no clean way to express "wait 2 seconds, then open the door, then play a sound" without manually tracking elapsed time in component state.

- `yield` / `wait(seconds)` mechanism for time-based sequences
- Coroutine scheduling integrated with the game loop
- Cleaner cutscene, tutorial, and event-chain authoring

## Transparent Material Rendering

**Priority: High**

The renderer currently uses binary alpha only — pixels are either fully opaque or discarded. There's no way to render glass, water surfaces, energy shields, smoke, or any translucent material. This is a core rendering capability that gates visual variety across every genre.

- Sorted alpha blending pass (back-to-front) for translucent materials
- `opacity` field on material component (0.0–1.0)
- Blend modes: alpha, additive, multiply
- Refraction for glass and water (screen-space distortion)
- Depth peeling or weighted-blended OIT for overlapping transparencies

> Partly delivered. The [ocean](concepts/ocean.md) pipeline already does a grab
> pass with screen-space refraction, per-channel absorption and turbidity, and
> [render mode 5](concepts/post-processing.md#render-modes) handles being
> underwater. What remains here is the *general* case: sorted alpha blending
> and an `opacity` material field for glass, shields and smoke.

## Script Modules & Shared Code

**Priority: High**

As games grow beyond a handful of scripts, there's no way to share utility functions. Every `.rhai` file is isolated — common code (damage formulas, inventory helpers, math utilities) gets copy-pasted across scripts. This is the biggest developer-productivity bottleneck for larger projects.

- `import "utils"` mechanism to load shared `.rhai` modules
- Module search path: `scripts/lib/` for shared code, game-level overrides
- Pre-compiled module caching (avoid re-parsing shared code per entity)
- Hot-reload awareness (recompile dependents when a module changes)

## ~~UI Layout System~~ Done

Data-driven UI with layout/style/logic separation. Structure defined in `.ui.toml`, visuals in `.style.toml`, logic in Rhai scripts. The procedural `draw_*` API continues to work alongside the layout system.

- Anchor-based positioning (9 anchor points: top-left through bottom-right)
- Flow layouts: vertical stacking (default) and horizontal
- Percentage-based sizing, auto-height containers, padding and margin
- Named style classes with runtime overrides from scripts
- Rhai API: `load_ui`, `unload_ui`, `ui_set_text`, `ui_show`/`ui_hide`, `ui_set_style`, `ui_set_class`, `ui_get_rect`
- Element types: Panel, Text, Rect, Circle, Image
- Multi-document support with handle-based load/unload
- Layout caching with automatic invalidation on screen resize

## ~~Terrain System~~ Done

Height-field terrain shipped with chunked rendering, RGBA splat-map blending, collision, a `terrain_height(x, z)` script query, a GPU grass system placed by splat density, and the `flint edit` terrain editor. See [Terrain](concepts/terrain.md). Chunk LOD by distance is the one bullet still open and folds into the culling / LOD item above.

## Audio Environment Zones

**Priority: Medium-High**

Walking from a stone cathedral into an open field should *sound* different. The spatial audio system handles positioning well, but there's no environmental modeling. This is the audio equivalent of reflection probes — a massive immersion jump for minimal complexity.

- Reverb zones defined as trigger volumes in scenes
- Preset environments (cathedral, cave, forest, small room, underwater)
- Smooth crossfade when transitioning between zones
- Occlusion: sounds behind walls are muffled (raycast-based)
- Script API: `set_reverb_zone(entity_id, preset)`, `set_reverb_mix(wet, dry)`

## Decal System

**Priority: Medium**

Bullet holes, blood splatters, scorch marks, footprints — decals are the detail layer that makes game worlds feel responsive. Currently there's no way to project textures onto existing geometry at runtime.

- Projected-texture decal rendering
- Configurable lifetime, fade, and layering
- Script API: `spawn_decal(position, normal, texture)`

## Reflection Probes & Environment Mapping

**Priority: Medium**

The PBR pipeline handles diffuse and specular lighting well, but specular reflections are essentially absent. This is the single biggest visual quality jump available.

- Pre-baked cubemap reflection probes at authored positions
- Probe blending between adjacent volumes
- Correct specular reflections on metals, water, glass, and polished surfaces

## Material Instance System

**Priority: Medium**

Each entity currently specifies its own texture paths and PBR parameters. There's no way to define "worn stone" once and apply it to fifty objects.

- Named material definitions (textures + PBR parameters)
- Material instances that reference and override a base material
- Material library for cross-scene reuse

## Save & Load Game State

**Priority: Medium**

`PersistentStore` survives scene transitions, but there's no way to snapshot and restore full ECS state mid-scene. Any game longer than a single session needs this.

- Full ECS snapshot (all entities, components, script state) to disk
- Restore from snapshot with entity ID remapping
- Checkpoint and quicksave support
- Script API: `save_game(slot)`, `load_game(slot)`

## 3D Debug Drawing

**Priority: Medium**

The 2D overlay draws in screen-space, but there's no way to visualize 3D information — physics colliders, AI sight cones, pathfinding routes, trigger volumes, raycast results. This is the single most impactful developer tool for iterating on gameplay.

- Script API: `debug_line(from, to, color)`, `debug_box(center, size, color)`, `debug_sphere(center, radius, color)`, `debug_ray(origin, dir, length, color)`
- Wireframe overlay rendered after scene, before HUD
- Auto-clear each frame (immediate-mode, like the 2D draw API)
- Toggle from the F4 Rendering & Effects menu — the per-feature F-keys were retired (ADR 0053) and the bare F-key space is spoken for
- Optional built-in modes: visualize physics colliders, trigger volumes, nav meshes

> The skeleton overlay in the model previewer and the wireframe debug modes
> (which now include skinned meshes) are the first pieces of this; a
> script-driven 3D line API is still open.

## Performance Profiler Overlay

**Priority: Medium**

Targeted optimization requires knowing where time is spent. F2 already shows frame time, FPS, draw stats and resolution in the player and viewer; what is missing is the breakdown.

- Per-system breakdown (render vs physics vs scripts vs audio)
- Triangle count and memory alongside the existing draw stats
- Frame time graph with spike detection
- Lives on the existing F2 stats overlay or as an F4 menu section (F9 is taken by the music-session force-fail)

## Further Horizon

These are ideas under consideration, not committed plans:

- **Networking** — multiplayer support with entity replication
- **Plugin system** — third-party engine extensions
- **Package manager** — share schemas, constraints, and assets between projects
- **WebAssembly** — browser-based viewer and potentially runtime
- **Shader graph** — visual shader editing for non-programmers
