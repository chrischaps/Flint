# Roadmap

Flint has a solid foundation — PBR rendering, physics, audio, animation, scripting, particles, post-processing, AI asset generation, and a shipped Doom-style FPS demo. The roadmap now focuses on the features needed to ship production games.

## Visual Scene Tweaking

**Priority: High**

Flint's core thesis is that scenes are *authored* by AI agents and code — not by dragging objects around a viewport. But AI-generated layouts often need human nudges: a light that's slightly too far left, a prop that clips through a wall, a rotation that's five degrees off. The goal isn't a full scene editor — it's a lightweight adjustment layer on top of the CLI-first workflow.

- Translate / rotate / scale gizmos for fine-tuning positions
- Property inspector for tweaking component values in-place
- Changes write back to the scene TOML (preserving AI-authored structure)
- Undo / redo for safe experimentation

## Frustum Culling & Level of Detail

**Priority: High**

Without visibility culling, every object renders every frame regardless of whether it's on screen. This is the performance ceiling that blocks larger scenes.

- BVH spatial acceleration structure
- Frustum culling (skip off-screen objects entirely)
- Mesh LOD switching by camera distance
- Optional texture streaming for large worlds

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

## Terrain System

**Priority: Medium-High**

The engine excels at interior scenes — taverns, dungeons, arenas — but has no solution for outdoor environments. Height-field terrain is the single biggest genre-unlocking feature missing: open-world, exploration, RTS, and large-scale games all depend on it.

- Height-field terrain with chunk-based rendering
- Material splatting (blend grass, dirt, rock, snow by painted weight maps)
- Chunk LOD for draw-distance scaling
- Collision mesh generation for physics and character controller
- Script API: `get_terrain_height(x, z)` for grounding NPCs and objects

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
- Toggle with a debug key (e.g. F10) — zero overhead when disabled
- Optional built-in modes: visualize physics colliders, trigger volumes, nav meshes

## Performance Profiler Overlay

**Priority: Medium**

Targeted optimization requires knowing where time is spent. Currently there's no visibility into the frame budget breakdown.

- In-engine overlay: frame time, draw calls, triangle count, memory
- Per-system breakdown (render vs physics vs scripts vs audio)
- Frame time graph with spike detection
- Toggle with a debug key (e.g. F9)

## Further Horizon

These are ideas under consideration, not committed plans:

- **Networking** — multiplayer support with entity replication
- **Plugin system** — third-party engine extensions
- **Package manager** — share schemas, constraints, and assets between projects
- **WebAssembly** — browser-based viewer and potentially runtime
- **Volumetric lighting** — light shafts and fog volumes
- **Shader graph** — visual shader editing for non-programmers
