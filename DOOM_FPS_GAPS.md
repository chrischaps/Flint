# Doom-Style FPS Gap Analysis

**62 gaps identified across 9 categories.** Phase A addresses 7 of them.

This document catalogs every missing piece between the Flint engine's current capabilities and a feature-complete Doom-style first-person shooter. It serves as a roadmap for future phases.

---

## A. Rendering System Gaps (13)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| A.1 | No particle system | Muzzle flash, blood, explosions, smoke | CPU emitter + billboard quads (reuse sprite pipeline) |
| A.2 | No skybox | Sky/environment backdrop | 6-face cubemap, full-screen quad at max depth |
| A.3 | No post-processing pipeline | Bloom, motion blur, screen effects | Intermediate render targets, off-screen passes |
| A.4 | No decal system | Bullet holes, blood splatters, scorch marks | Deferred projection or surface-aligned quads |
| A.5 | ~~No billboard sprites~~ | ~~Doom-style enemies/items~~ | **DONE — Phase A Stage 1** |
| A.6 | No weapon viewmodel | Gun visible in player's hands | 2D sprite overlay (Doom-authentic) or 3D view-space model |
| A.7 | No frustum culling | Performance with large levels | AABB vs view frustum check per entity |
| A.8 | No instancing/batching | Many identical enemies/items | Instance buffer for shared texture atlas |
| A.9 | No MSAA | Jagged geometry edges | MultisampleState count=4 |
| A.10 | No screen-space effects | SSAO, SSR, edge detection | Deferred passes with depth/normal buffers |
| A.11 | No UV/material animation | Flowing lava/water, teleporter effects | Per-frame UV offset in shader |
| A.12 | No point/spot light shadows | Dynamic torch shadows | Per-light shadow passes or omnidirectional shadow maps |
| A.13 | No stencil operations | Portals, mirrors, advanced masking | Enable stencil state in pipeline |

## B. Physics System Gaps (6)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| B.1 | ~~Raycasting not exposed~~ | ~~Hitscan weapons, LOS checks~~ | **DONE — Phase A Stage 2** |
| B.2 | No projectile physics | Rockets, fireballs, plasma bolts | Kinematic bodies with velocity + collision callbacks |
| B.3 | No trigger volumes | Door triggers, secrets, level exits | Sensor colliders with distinct enter/exit events |
| B.4 | No knockback/forces | Explosion pushback, rocket jump | `apply_impulse(entity, fx, fy, fz)` API |
| B.5 | No shape sweep/overlap | AoE damage, melee arcs, thick traces | Expose Rapier's `intersections_with_shape()` |
| B.6 | No platform velocity inheritance | Elevators carrying player | Detect ground body, add its velocity to movement |

## C. Scripting System Gaps (8)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| C.1 | ~~No raycast API~~ | ~~Weapons, LOS~~ | **DONE — Phase A Stage 2** |
| C.2 | ~~No combat/damage APIs~~ | ~~Health tracking, killing~~ | **DONE — Phase A Stage 3** |
| C.3 | ~~No camera direction API~~ | ~~Weapon aiming~~ | **DONE — Phase A Stage 2** |
| C.4 | No archetype spawning | Runtime enemy/projectile/item creation | `spawn_from_archetype(name, pos)` reads schema + populates |
| C.5 | No inventory/state table | Weapon lists, collected keys, ammo types | Custom component fields or serialized string |
| C.6 | No timer/delayed execution | Delayed effects, timed events | Manual timer variables (workaround exists) |
| C.7 | No targeted entity messaging | Entity-to-entity communication | Custom events with entity ID in data |
| C.8 | No scene loading from script | Level exits, teleporters | `load_scene(path)` signaling PlayerApp |

## D. Audio System Gaps (5)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| D.1 | No sound variation | Varied footsteps, gunshots | Random pitch/volume variation per play |
| D.2 | No sound occlusion | Muffled sounds through walls | Raycast listener→source, apply filter if blocked |
| D.3 | No sound groups/mixing | Independent volume for weapons/music/ambient | Kira track groups with per-group volume |
| D.4 | No dynamic music | Combat vs exploration transitions | Multiple tracks, crossfade on game state events |
| D.5 | No dialogue/voice system | NPC dialog, boss taunts | Audio clip + egui text overlay from script |

## E. Game State & Flow Gaps (5)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| E.1 | No game state machine | Pause, death, menus | `GameState` enum, gate systems on state |
| E.2 | No level transitions | Level progression, teleporters | `load_scene()` on PlayerApp, teardown + rebuild |
| E.3 | No save/checkpoint | Quicksave, level start saves | Serialize all entity components to TOML |
| E.4 | No score/statistics | Kill count, secrets, time | Script counters + HUD display |
| E.5 | No difficulty system | Easy through Nightmare scaling | Difficulty multiplier component, scripts read it |

## F. Level Design Gaps (6)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| F.1 | ~~No key/lock system~~ | ~~Colored keycards gating progression~~ | **Partial — E1M1 Task 5**: door system in place with `locked` + `key_required` fields; key inventory + pickup not yet implemented |
| F.2 | No switches/buttons | Remote door/elevator activation | `switch` component with target_entity + action |
| F.3 | No elevators/lifts | Gameplay-driven vertical transport | `elevator` component, script-driven movement |
| F.4 | No secret areas | Classic Doom secret counting | `secret` component on trigger volumes |
| F.5 | No teleporters | Instant position transport | Trigger volume script sets player position |
| F.6 | No hazard zones | Lava, nukage, acid floors | `hazard` component with DPS, script checks contact |

## G. Enemy & Combat Gaps (8)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| G.1 | ~~No enemy AI~~ | ~~Combat behavior~~ | **Partially DONE — Phase A Stage 5** (1 type) |
| G.2 | No pathfinding | Obstacle avoidance | Grid A* or direct movement (Doom-authentic) |
| G.3 | No multiple enemy types | Variety (imp, soldier, demon, etc.) | Unique sprite sheets + behavior script variants |
| G.4 | No projectile system | Fireballs, rockets, plasma | `projectile` component with trajectory + splash |
| G.5 | No infighting | Enemy-on-enemy damage + retargeting | Track damage source, allow target switching |
| G.6 | No powerups | Invulnerability, berserk, etc. | `powerup` component with type + duration timer |
| G.7 | No breakable objects | Explosive barrels, destructibles | `destructible` component with health + death effect |
| G.8 | No gibs/overkill | Excessive damage visual feedback | Damage threshold → spawn gib sprites |

## H. HUD & UI Gaps (6)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| H.1 | ~~No combat HUD~~ | ~~Health, ammo, weapon display~~ | **DONE — Phase A Stage 6** |
| H.2 | No full status bar | Armor, keys, face/mugshot | Extend HUD with additional component reads |
| H.3 | No damage direction indicator | Where damage came from | Screen-edge arrow/flash toward source |
| H.4 | No automap | 2D top-down level map | Render floor plan as 2D lines in egui |
| H.5 | No menu system | Main menu, options, settings | egui windows with game state transitions |
| H.6 | No message/notification system | "Found a secret!", "Blue key acquired" | egui text with fade timer, message queue |

## I. Infrastructure Gaps (5)

| # | Gap | Needed For | Approach |
|---|-----|-----------|----------|
| I.1 | No build/package system | Distributing standalone game | Asset bundling, release builds |
| I.2 | ~~No settings persistence~~ | ~~Mouse sensitivity, volume, keybinds~~ | **Partial — Input Extensibility**: keybind persistence via layered InputConfig TOML + user override files; mouse sensitivity and volume settings not yet implemented |
| I.3 | No demo recording/playback | Replays, testing | Serialize InputState per tick |
| I.4 | No multiplayer/networking | Deathmatch, co-op | Major architecture change (deferred) |
| I.5 | No mod support | Community content | Load additional schema/script/asset dirs |

---

## Summary

| Category | Total | Fixed in Phase A | Remaining |
|----------|-------|-----------------|-----------|
| A. Rendering | 13 | 1 (sprites) | 12 |
| B. Physics | 6 | 1 (raycast) | 5 |
| C. Scripting | 8 | 3 (raycast, combat, camera) | 5 |
| D. Audio | 5 | 0 | 5 |
| E. Game State | 5 | 0 | 5 |
| F. Level Design | 6 | 0.5 (doors partial) | 5.5 |
| G. Enemy & Combat | 8 | 1 (basic AI) | 7 |
| H. HUD & UI | 6 | 1 (combat HUD) | 5 |
| I. Infrastructure | 5 | 0.5 (keybinds partial) | 4.5 |
| **Total** | **62** | **8** | **54** |
