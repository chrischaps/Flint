# E1M1: Hangar — Task Breakdown

> E1M1-inspired level for the Flint engine. Same flow as Doom's Hangar (start room → hallway → nukage/zigzag room → computer room → exit) with our own proportions. All game logic in Rhai scripts. ~150 entities.

---

## Task 1: Make Combat Game-Specific
**Why**: `deal_damage`/`get_health`/`heal`/`is_dead` are hardcoded in the engine. They should be game-layer functions so the doom_fps game can build armor-aware damage.

**Work**:
- Add `pub fn register_fn(...)` to `ScriptEngine` (engine.rs) so games can register custom Rhai functions
- Remove combat function registrations from `api.rs`
- Register equivalent combat functions in `player_app.rs` using the same `ScriptCallContext` pattern
- Verify all existing scripts still work (enemy_ai, pickup, player_weapon, hud)

**Files**: `crates/flint-script/src/engine.rs`, `crates/flint-script/src/api.rs`, `crates/flint-player/src/player_app.rs`

---

## Task 2: Armor System
**Why**: E1M1 has green armor. Need an armor component and damage absorption logic.

**Work**:
- Create `games/doom_fps/schemas/components/armor.toml` (current, max, absorption fields)
- Update game-registered `deal_damage` (from Task 1) to check target's armor, absorb percentage, reduce remaining from health
- Create `pickup_armor.toml` archetype
- Extend `pickup.rhai` to handle `pickup_type = "armor"`
- Update `hud.rhai` to display armor value

**Depends on**: Task 1

---

## Task 3: Weapon Switching (Pistol + Shotgun)
**Why**: E1M1's core loop involves finding the shotgun. Player needs 2 weapons.

**Work**:
- Modify `player_weapon.rhai` to handle `on_action("weapon_1")` (pistol) and `on_action("weapon_2")` (shotgun)
- Track per-weapon ammo as module-level variables; swap `weapon` component fields on switch
- Shotgun stats: 7 pellets × 5 dmg, 0.08 rad spread, 0.8s fire rate
- Shotgun fire = loop 7 raycasts with random angular offset
- Create `pickup_weapon_shotgun.toml` archetype and `pickup_shells.toml` archetype
- Extend `pickup.rhai` for `pickup_type = "weapon_shotgun"` and `"shells"`
- Update `hud.rhai`: weapon name, ammo type label, shotgun ammo display
- Generate/create `shotgun_fire.ogg` audio

**Files**: `player_weapon.rhai`, `pickup.rhai`, `hud.rhai`, new archetypes, new audio

---

## Task 4: Hitscan Enemy AI
**Why**: E1M1 has Zombiemen and Shotgun Guys — ranged hitscan enemies unlike the melee Imp.

**Work**:
- Create `scripts/enemy_ai_hitscan.rhai` — fork of `enemy_ai.rhai` with these changes:
  - `update_attack()` fires raycast from enemy position toward player, deals damage on hit
  - Reads `enemy.attack_spread` for accuracy randomization
  - Reads `enemy.pellet_count` for shotgun guy multi-pellet
  - Different alert/death sound fields
- Add fields to `schemas/components/enemy.toml`: `attack_spread`, `pellet_count`, `alert_sound`, `death_sound`
- Create `enemy_zombieman.toml` archetype (20 HP, single hitscan, 1.0s rate, speed 2.5)
- Create `enemy_shotgunguy.toml` archetype (30 HP, 3-pellet hitscan, 1.5s rate)
- Create/generate `sprites/zombieman.png` and `sprites/shotgunguy.png` (4×2 sprite sheets)

**Files**: new `enemy_ai_hitscan.rhai`, modified `enemy.toml`, 2 new archetypes, 2 new sprites

---

## Task 5: Door System
**Why**: E1M1 has doors between rooms — vertical sliding doors that open on interact.

**Work**:
- Create `schemas/components/door_state.toml`: state, open_height, speed, stay_open_time, timer, closed_y, auto_open, trigger_range
- Create `scripts/door.rhai` with state machine:
  - `closed` → interact or proximity → `opening`
  - `opening` → lerp Y upward → `open` (play sound)
  - `open` → timer countdown → `closing`
  - `closing` → lerp Y down → `closed` (re-open if player blocking)
- Create `archetypes/fps_door.toml` (kinematic rigidbody, box collider, door_state, interactable, script)
- Door uses `set_position()` — `PhysicsSync::update_kinematic_bodies()` moves collider automatically
- Generate/source `door_open.ogg` and `door_close.ogg`

**Files**: `door_state.toml`, `fps_door.toml`, `door.rhai`, 2 audio files

---

## Task 6: Nukage Damage Floors
**Why**: E1M1's zigzag room has toxic green nukage that damages the player.

**Work**:
- Create `schemas/components/hazard.toml`: min_x, max_x, min_z, max_z, surface_y, dps, tick_interval
- Create `scripts/hazard_zone.rhai`: find player each frame, check if within bounds and below surface_y, apply periodic damage via `deal_damage`
- Nukage floor entities use bright green material (color ~[0.1, 0.8, 0.1], low roughness for slight glow)
- Controller entity is invisible (no geometry, just the script)

**Files**: `hazard.toml`, `hazard_zone.rhai`

---

## Task 7: Secret Walls
**Why**: E1M1 has classic Doom push-walls hiding bonus items.

**Work**:
- Create `schemas/components/secret.toml`: discovered, slide_direction, slide_distance
- Create `scripts/secret_wall.rhai`: `on_interact()` slides wall in direction, marks discovered, plays sound
- Update `hud.rhai` to show brief "A secret is revealed!" notification (timed fade)
- Generate/source `secret_found.ogg`

**Files**: `secret.toml`, `secret_wall.rhai`, modified `hud.rhai`, 1 audio file

---

## Task 8: Exit Switch + Level Completion
**Why**: E1M1 ends with an exit switch that shows a completion screen.

**Work**:
- Create `schemas/components/level_state.toml`: completed, kill_count, total_enemies, secret_count, total_secrets, elapsed_time
- Create `scripts/level_controller.rhai`: tracks elapsed time, scans for dead enemies + discovered secrets, updates level_state
- Create `scripts/exit_switch.rhai`: `on_interact()` sets level_state.completed = true
- Update `hud.rhai` with completion overlay:
  - Dark fade, "Hangar — Complete!", kills X/Y, secrets X/Y, time MM:SS

**Files**: `level_state.toml`, `level_controller.rhai`, `exit_switch.rhai`, modified `hud.rhai`

---

## Task 9: Build Level Geometry
**Why**: The actual E1M1-inspired room layout.

**Work**: Create `games/doom_fps/scenes/e1m1_hangar.scene.toml` with all geometry entities.

**Layout** (~60×50 units):
```
  ┌─────────────────────────────────┐
  │        COMPUTER ROOM            │  (exit switch)
  │            [DOOR]               │
  ├──────┬──────────────────────────┤
  │ARMOR │    ZIGZAG ROOM           │
  │ALCOVE│  walkway over nukage     │
  ├──────┴───[DOOR]─────────────────┤
  │        HALLWAY                  │  (zombiemen)
  │            [DOOR]               │
  ├─────────────────────────────────┤
  │        START ROOM               │  (player spawn, secret wall)
  └─────────────────────────────────┘
```

**Build order** (each room = floor + ceiling + walls + colliders):
1. Start room (~15 entities)
2. Hallway (~12 entities)
3. Zigzag room + nukage pit + raised walkways (~25 entities)
4. Computer room (~12 entities)
5. Armor alcove (~8 entities)
6. Secret area (~5 entities)
7. Doors (3 door entities)
8. Lights (12-14 point lights + 1 directional)

**Estimated**: ~100-110 geometry entities, ~1200 lines of TOML

---

## Task 10: Populate Level (Enemies + Items)
**Why**: Place all enemies, pickups, and controllers to make it playable.

**Work**: Add to `e1m1_hangar.scene.toml`:

**Enemies (~16)**:
- Start room: 1-2 zombiemen
- Hallway: 3-4 zombiemen
- Zigzag room: 2 imps + 2-3 zombiemen
- Computer room: 1 shotgun guy + 2 zombiemen
- Armor alcove: 1 imp

**Pickups (~15)**:
- Shotgun weapon: hallway
- Stimpacks: 3-4 along path
- Ammo clips: 5-6 near zombie positions
- Shell boxes: 3-4 in zigzag + computer rooms
- Green armor: armor alcove
- Health bonuses: start room
- Medikit: secret area

**Controllers**:
- HUD controller (hud.rhai)
- Level controller (level_controller.rhai)
- Nukage hazard controllers (1-2 zones)
- Exit switch entity

---

## Task 11: Audio + Visual Polish
**Why**: Atmosphere and immersion. Nice-to-have.

**Work**:
- Material color palette per room:
  - Start: warm brown (0.4, 0.3, 0.2)
  - Hallway: gray concrete (0.5, 0.5, 0.5)
  - Nukage area: dark walls, green-lit
  - Computer room: blue-gray tech (0.3, 0.4, 0.5)
- Source/generate remaining audio (zombie alert/death, nukage sizzle)
- Add ambient low hum for computer room
- Decorative barrel/computer sprites (non-interactive)
- AI-generated wall textures via Flux if available

---

## Dependency Graph

```
Task 1 (combat to game layer)
  └─→ Task 2 (armor)
  └─→ Task 3 (weapon switching)
  └─→ Task 4 (hitscan enemies)

Task 5 (doors)         ─── independent
Task 6 (nukage)        ─── independent (needs Task 1 for deal_damage)
Task 7 (secrets)       ─── independent
Task 8 (exit + completion) ─── independent

Task 9 (level geometry) ─── independent (can start anytime)

Task 10 (populate) ─── needs Tasks 2-9 all done
Task 11 (polish)   ─── needs Task 10
```

**Suggested order**: 1 → 2+3+4 (parallel) → 5+6+7+8 (parallel) → 9 → 10 → 11
