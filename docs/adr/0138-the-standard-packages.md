# ADR 0138: The standard packages: the stages they join, their state as resources, the kernel's player and view, and one dependency rule

Status: proposed, 2026-11-27 (#1612). Waits on Chris's word on each item
under "Decisions" at the end. Epic #1611 (Modularity, Phase 3).
Builds on [ADR 0117](0117-player-subsystems-and-one-game-step.md) (stages
and one game step), [ADR 0136](0136-the-package-model.md) (the package
model) and [ADR 0137](0137-the-public-package-api.md) (the public Package
API). Answers [ADR 0140](0140-the-vehicle-package.md)'s gaps 1 to 3 (#1647,
#1648, #1649). Once accepted, it supersedes decision 4 of
[ADR 0134](0134-the-character-controller-stays-in-physics-with-fixed-action-names.md).
The shape of animation is #1613's record; this one says only which stages
the animation clock and `rig` join.

## Context

Phase 2 made a package a folder that the launchers compose from a list. The
five standard packages (physics, audio, animation, particles, terrain) are
still crates the kernel names, so `packages/engine/package.toml` holds them
undivided and no game can leave them out. Each Phase 3 move (#1614 to
#1623) has to answer the same four questions. If each move answered them
itself, the five moves would invent five seams:

1. **Stages.** `GameCore::step` (`crates/flint-player/src/player_app/game_core.rs`)
   calls the standard crates by name: `physics.begin_frame`,
   `character_frame_input`, `update_character`, `fixed_update` /
   `fixed_update_with`, `camera_position` / `camera_target` / `camera_yaw`
   / `camera_pitch`, `animation.update`, `drain_sprite_events`,
   `drain_sequence_cues`, `run_ik`, `particles.update`. The host adds
   `sound` and `bones_posed`. A removable package can't be called by name.
2. **Script state.** `ScriptCallContext` (`crates/flint-script/src/context.rs`)
   holds `physics: *const PhysicsSystem`, `ground: Option<Arc<flint_terrain::GroundField>>`
   and `music: MusicClock` as typed fields. The callbacks take
   `flint_animation` event types. The core `ScriptCommand` enum has the
   audio, particle and 2D-velocity variants.
3. **Who the player is.** Five player files ask physics:
   `PhysicsSystem::has_player_entity` in `game_core.rs`, `console.rs`,
   `events.rs`, `scene_entry.rs` and `debug_panels.rs`, and the camera
   (`camera_position`, `camera_yaw`, `camera_pitch`) in `game_core.rs`.
   flint-script's `is_near_player` looks for the first
   `character_controller` itself.
4. **The renderer's inputs.** flint-render builds terrain from its component
   (`load_scene_terrain` in `scene_renderer/binding.rs`), and reads
   `flint_particles::ParticleSync` in `FrameInputs`.

The vehicle package (ADR 0140, #1628) is the first package written against
ADR 0137, and found three kernel gaps that belong here: no read of physics
after the steps (#1647), module calls that reach a package a frame late
(#1648), and no contacts in a fixed step (#1649).

**What the graph refuses today.** A prototype of decision 6's rule over
`cargo metadata --no-deps` on `main` at `71cb690d` (the engine manifest's
five groups split into their packages, and the `[requires]` lines this
record plans) refuses 25 edges. That is 19 from kernel crates and 6 between
packages:

| Crate (package) | Depends on | Ticket that ends it |
|---|---|---|
| flint-render (kernel) | flint-particles, flint-terrain | #1615 |
| flint-package (kernel) | flint-physics, flint-audio, flint-particles, flint-terrain | #1616 |
| flint-script (kernel) | flint-physics; flint-animation; flint-terrain | #1617; #1620; #1623 |
| flint-player (kernel) | all five | #1617, #1619, #1620, #1621, #1622, #1623 |
| flint-cli (kernel) | flint-physics, flint-animation, flint-particles, flint-terrain | #1617, #1620, #1622, #1623 |
| flint-viewer (kernel) | flint-particles | #1622 |
| flint-debug-ui (debug-hud) | flint-particles; flint-terrain | #1622; #1615 |
| flint-living (living) | flint-terrain (`GrassConfig`) | #1615 |
| flint-rhythm-game (rhythm) | flint-audio | #1619 (`[requires] audio`) |
| flint-procgen (procgen) | flint-terrain (the `terrain_v1` generator) | #1623 |
| flint-procgen (procgen) | flint-wooze (living's) | **#1659**, filed for this record |

The last row is a reverse edge. Living requires procgen, but procgen's
optional `living` part depends on living's crate (ADR 0137 section 9). No
Phase 3 ticket removes it.

**What a 2D game stops compiling.** These counts are context, not a gate
(Chris, 2026-11-27). They are third-party crates in each crate's normal
`cargo tree`, beyond what flint-core and flint-runtime already bring (49):
flint-physics 43 (rapier3d, parry, nalgebra, crossbeam ...), flint-audio 29
(kira, cpal ...), flint-animation 28 (gltf and image's decoders), flint-terrain
23 (image), flint-particles 2. Source lines, tests included: physics 8,783,
audio 9,438 (+2,265 for the worklet), animation 7,773, particles 6,086,
terrain 3,354.

## Decision

### 1. A standard package's state is a run resource of its own type

Each package puts its state into `Resources` (ADR 0137 decision 2) through
`Registry::run_resources`, as procgen's resolver and living's props are
already. A context lends `resources`, and a package that `[requires]` another
names that package's type:

| Package | Resource | Today |
|---|---|---|
| physics | `flint_physics::PhysicsSystem` | `GameCore::physics` |
| audio | `flint_audio::engine::AudioEngine` (the device, silent, or the worklet's, as the host asks) | `PlayerApp::audio`, `StageCtx::audio` |
| particles | `flint_particles::ParticleSystem` | `GameCore::particles`, `PanelCtx::particles` |
| rig | its poses and bone palettes (#1613's record names the type) | inside `GameCore::animation` |
| terrain | none of its own: it publishes the kernel's ground (decision 5) | `LoadedTerrain` from the renderer |
| first-person | none: its controller state lives in its subsystem, and it writes the kernel's `PlayerView` (decision 4) | `PhysicsSystem::character` |

- **`Resources` moves down to flint-core** (`flint_core::resources::Resources`,
  the same `TypeId` map with the pass-through hasher), so flint-script can
  lend it to script calls (decision 3). `flint_package::Resources`
  re-exports it, so no package's code changes.
- **`FixedCtx::physics` goes.** A fixed step reads
  `ctx.resources.get_mut::<PhysicsSystem>()`, wrapped as
  `flint_physics::of_mut(&mut Resources)`. The field can't stay as a
  forwarding accessor, because the accessor would keep flint-package
  depending on flint-physics. Vehicle is the one client, and #1616 moves it
  in the same PR (ADR 0140's Consequences already say so). The changelog
  fragment gives a game's own package the one-line change.
- **#1647 closes with it.** Physics' state is a resource, so every
  `StageCtx` lends it. At `Stage::Physics`, after the frame's last step and
  the write-back, vehicle reads the chassis's velocity fresh, and
  `vehicle_state` stops being a step stale. No after-step hook is needed.
- **The made meshes' triangles go with physics.** `GameCore::collider_sources`
  (the triangles of procgen and runtime meshes that `mesh` and `convex`
  colliders name, #1222) becomes physics' resource too. The asset makers
  keep the kernel trait they write through (`flint_package::assets::MadeMeshes`),
  and `MakeCtx::colliders` becomes an `Option`, `None` in a build without
  physics, where no collider asks for the triangles.
- **#1649: `PhysicsSystem::contacts_of(entity) -> impl Iterator<Item = Contact>`**,
  where `Contact { other: EntityId, normal: [f32; 3], point: [f32; 3], impulse: f32 }`
  is read from Rapier's narrow phase. It is a physics method and plain data,
  not a lend of `NarrowPhase`, so Rapier's types stay out of a client's
  signatures. #1617 adds it.

### 2. The stages: three new frame stages, and four stages inside a step

A package's frame work goes at a named stage, through `update`. Its work
inside a fixed step goes at one of four step stages. The kernel keeps what
every game has: the clock and its step loop, the game state's policy, the
event bus, the scripts, the camera, the animation clock (#1613) and the
renderer. `Stage` is `#[non_exhaustive]`, so adding stages breaks no game.

**Inside a step** (`flint_package::StepStage`), the order today's
`tick_fixed_physics` runs:

| Step stage | Who | Today |
|---|---|---|
| `Control` | first-person: the walk (`update_character`) | `physics.update_character` before the sync-in |
| `SyncIn` | physics: new bodies and joints, kinematic poses, colliders, joint targets | the first half of `fixed_update_with` |
| `Forces` | every `fixed_update`: vehicle, the test toy | ADR 0137 section 3a's hook |
| `Integrate` | physics: Rapier's step, then the step's contacts onto the bus | the second half of `fixed_update_with` |

`Subsystem` gains `fn step(&mut self, stage: StepStage, ctx: &mut FixedCtx)`.
Its default calls `fixed_update(ctx)` at `Forces`, so vehicle and the toy
compile unchanged. The kernel's loop is unchanged: `should_fixed_update`,
the `steps && config.runs(PHYSICS)` guard (where `physics` is now the physics
package's policy name, decision 7), `consume_fixed_step` on every iteration,
and `fixed_steps` counting the steps that ran. Each step walks the four
stages over the subsystems in their order. A capture runs no steps, as now.

**The frame**, with the three new stages in bold and each moved call at the
stage where it now runs:

| Stage | What runs there | Moved from |
|---|---|---|
| `Clock`, `Input`, `PreScripts` | as today; audio puts the sounds' clocks into its module state at `PreScripts` | `window_for_scripts`' audio clocks |
| *scripts' `on_update`* | | |
| `Scripts` | physics: `collider_sources.refresh`, then `begin_frame(pending_fixed_ratio)`; first-person: `character_frame_input` (look, the jump press), under the physics policy | the top of `tick_fixed_physics` |
| *module commands that ask for it* | decision 3 | (new) |
| `FixedStep` | the step loop above | `tick_fixed_physics` |
| **`View`** | a controller places the eye: first-person's `camera_position`, `camera_target`, yaw and pitch into `PlayerView::eye` | after the step loop |
| *the kernel* | the camera follows `PlayerView::eye` (`update_first_person`, the target) and sets `PlayerView::listener` | the same |
| `Physics` | audio moves the listener to `PlayerView::listener`; vehicle writes its wheel poses | `host.listener(FirstPerson)` |
| *dispatch, the script camera* | the kernel sets `listener` to the script's camera when a script set its position and no controller placed an eye | |
| `PostPhysics` | audio moves the listener when the script's camera moved it | `host.listener(ScriptCamera)` |
| `ScriptUi`, *late models* | as today | |
| **`Sound`** | audio: the reload of changed sounds, triggers from the frame's events, the sources' update | `FrameHost::sound` |
| *the animation clock* | the kernel: clips, layers and sequences, flipbooks as tracks; sprite ends and cues to scripts | `animate` |
| *bones to the GPU* | the host uploads the palettes rig published (plain data, until Phase 4) | `bones_posed` |
| **`Pose`** | rig: bone probes, then IK, the last writer of transforms | `after_animation`'s first half |
| `Animation` | particles: its step, under the particles policy | `after_animation`'s second half |
| `Procgen`, `Sync`, `PreRender` | as today | |

- **Checked against `game_core::step` at `71cb690d`.** Every moved call
  keeps its place relative to the calls around it. `refresh` still runs
  before `begin_frame`, which runs before `character_frame_input`, because
  physics comes before first-person in the registry's order (requires
  first). The walk still runs before the sync-in, and the forces still run
  between the sync-in and Rapier's step. The camera still follows before
  `Stage::Physics` and the listener before the first dispatch. Sound still
  runs after the late models and before the clock. Probes and IK still run
  after the bone upload and before particles, and particles before every
  other package's `Animation` (standard before feature in the order, and no
  subsystem hears `Animation` today).
- **What does move:** a game extension's `update` at `Scripts` now runs
  after physics' and first-person's frame work rather than before it, and
  extensions hear three more stages. No game uses `Stage::Scripts` (Float,
  FlintKart and Kandinski have no Rust of their own outside `engine/`).
- **`StageCtx` gains two fields:** `events: &[GameEvent]` (the frame's
  drained events, for audio's triggers at `Sound`) and `capture:
  Option<&Capture>`. A capture keeps its own animation and particles today
  (`HeadlessAnimation` in `bones_posed`, flint render's particle stage), so
  the kernel doesn't run `Sound` or `Pose` there, as it doesn't run `animate`.
  Particles asks `ctx.capture` at `Animation`, and each package's capture
  side stays its own (#1622, #1621).
- **What it costs.** With S subsystems a frame makes 13 S calls today (ADR
  0137, "How many calls a frame"). This adds 3 S for the new stages and
  3 S a step for the step stages beyond `Forces`. Physics, first-person,
  audio and particles, about 12 lookups at 1.5 ns, and a stock walk scene
  with S = 5 then makes about 100 calls at 1 ns a frame. That is about
  0.1 µs of a 16.7 ms frame. Each move proves byte-identical goldens and
  playtests anyway, which is the stronger check.

### 3. Script modules reach their state through the lent resources

- **Reads.** The scripts' state scope (`ScriptSystem::state_scope`) lends
  `&Resources` for the call, where it lends `&PhysicsSystem` today, through
  the same raw-pointer discipline (read only inside a script call, never
  across a `&mut` use). A module function asks `call.resource::<T>()`, so
  `physics`' `raycast` asks for `PhysicsSystem` and `terrain_height` for the
  ground. The `physics` pointer, `try_physics` and `Call::physics` go.
- **Per-frame values a host hands a module** stay module state
  (`ScriptSystem::set_module_state`, #698): `MusicClock` (today a typed
  field) and audio's sound clocks.
- **Writes are module commands** (`ModuleCommand`, ADR 0116 decision 5),
  taken by the owning package's `script_command`. The core `ScriptCommand`
  variants of a moving module become that package's payloads: `PlaySound`,
  `PlaySoundAt`, `StopSound`, `SetSoundVolume`, `SetBusVolume`,
  `SetUserBusVolume` and `SetBusLowpass` for audio;
  `EmitBurst`, `PlayEffect`, `StopEffect` and `SetEffectParam` for
  particles; `SetVelocity2D` for physics. Each is still taken at the drain
  it is taken at today, so no frame changes. `SetAudioLowpass` is a
  one-value-a-frame request (`FrameRequests::audio_lowpass`), which the
  player applies in `finish_frame`; it becomes audio's own request, applied
  by audio at `Sync`, the last stage before `finish_frame`. A capture's
  `PRESENTATION_ONLY` refusals name the functions.
- **#1648: `ModuleCommand::before_steps()`.** A function that wants its
  command in this frame's steps marks it. The kernel hands the marked
  commands to their packages right after `Stage::Scripts`, before the step
  loop. The rest wait for the usual drain. The mark belongs to the
  function, so moving a module changes no frame, and vehicle's
  `vehicle_place` and a hop or boost reach `fixed_update` the same frame.
  #1617 adds the call and a toy test that holds it.
- **Callbacks stay the kernel's.** `on_collision` and `on_trigger_*` read
  `GameEvent`s (flint-runtime), which physics pushes. `on_animation_end`
  and `on_sequence_cue` read the animation clock's events, and the clock is
  the kernel's from #1620.

| Module | Goes to | Its state |
|---|---|---|
| `physics` (6), `physics_2d` (4), `nav` (2) | physics | `PhysicsSystem` (raycasts, layers, extents, paths); `move_character` stays physics': it is a sweep any controller uses (ADR 0134) |
| `audio` (12) | audio | the `AudioEngine` resource for reads, module state for its clocks, commands for the rest |
| `particle` (7) | particles | commands |
| `terrain` (1) | terrain | the kernel's ground (decision 5) |
| `animation` (11), `tween` (1) | the kernel, unless #1613's record places a function in `rig` (a function goes with the code it calls) | the clock's own |
| `music` (3) | music (`crates/flint-script/src/api/music.rs` to `packages/music`), which ends #1591's `music` exception | `MusicClock` as module state |

### 4. The kernel's player and view

One kernel resource says who the game is played as, and where it is seen
and heard from. It lives in flint-runtime, beside the clock and the input,
so that flint-script can read it as well as the player:

```rust
/// Who the game is played as, and where it is seen and heard from
/// (ADR 0138). A run resource; reset at each scene's entry.
#[derive(Debug, Clone, Default)]
pub struct PlayerView {
    /// The entity the game is played as. `None`: no player (a menu, a 2D
    /// scene, a script's own controller that never says).
    pub player: Option<EntityId>,
    /// Whether the player looks with the mouse: the cursor is captured at
    /// the scene's entry, on a click while physics runs, and when the
    /// panels close.
    pub mouse_look: bool,
    /// The actions the controls line lists for the player (first-person's
    /// six, `flint_first_person::ACTIONS`).
    pub controls: &'static [&'static str],
    /// The eye a controller placed this frame, at `Stage::View`; `None`
    /// leaves the camera where the scene and the scripts put it.
    pub eye: Option<Eye>,
    /// Where the listener goes this frame, set by the kernel from the eye
    /// or the script's camera; audio applies it.
    pub listener: Option<Eye>,
}

pub struct Eye { pub position: Vec3, pub target: Vec3, pub yaw: f32, pub pitch: f32 }
```

| Reader today | Reads instead |
|---|---|
| `game_core.rs`: `has_fps_player`, the first-person camera after the steps, the listener's two cases | `player`, `eye`, `listener` (decision 2) |
| `console.rs`: the controls line's `CHARACTER_ACTIONS` | `controls` |
| `events.rs`: `click_captures` | `mouse_look` |
| `scene_entry.rs`: the cursor at entry | `mouse_look` |
| `debug_panels.rs`: `cursor_follows_panels` | `mouse_look` |
| flint-script `is_near_player`: the first `character_controller` it finds | `player` |

- **A `first-person` package** (standard, `[requires] physics`) takes
  `packages/physics/crates/flint-physics/src/character.rs` and its camera. Its builder finds
  the player as today (the first `character_controller` by name, ADR 0134
  decision 1) and sets `player`, `mouse_look = true` and `controls` before
  the scene's first frame, so the cursor and the controls line are right at
  entry (#1614's risk). Its hooks are `Scripts`, the `Control` step stage
  and `View` (decision 2). The six action names move with it, as
  `flint_first_person::ACTIONS`.
- **#1614 comes before the package.** Physics' built-in controller writes
  `PlayerView` from inside physics until #1618 moves it, so the readers
  move once. *As built (#1614):* `PhysicsSystem::enter_player_view` at
  the scene's entry and `place_eye` after the steps; the view is a run
  resource from `GameCore::new`. A capture steps no physics, so it sets
  `player` alone (`flint_physics::character::find_player`), which
  `find_nearest_interactable` reads as well as `is_near_player`. The
  script camera's listener keeps its old pitch sign (`Eye::looking`, #1666),
  and `set_player` is #1667.
- **Room for other controllers and cameras.** A script controller that calls
  `move_character` sets `player` through a kernel script function
  (`set_player(id)`, in the kernel's `camera` module) and gets the cursor,
  the listener and `is_near_player`. A vehicle's chase camera (#1631) places
  the eye at `Stage::View` in the vehicle package, an orbit camera (ADR 0134's
  deferred `camera_mode`) the same way. The camera kinds go in the package
  of the thing they follow: no `camera` package until two packages need
  the same kind.
- **This supersedes ADR 0134 decision 4.** All three of its reasons are gone.
  Packages have a fixed step (`fixed_update`, #1551). A Rust package costs
  nothing over a built-in. And the host reads the player from `PlayerView`,
  not from physics. Decisions 1 to 3 stand: one player, the first by name;
  the retired fields; the fixed action names. #1618 updates 0134's status
  line.

### 5. The renderer keeps the plain data it reads

flint-render stops depending on flint-terrain and flint-particles (Chris's
Q1). Their GPU halves stay in flint-render until Phase 4's render-feature
seam.

| Type | Goes to | Why |
|---|---|---|
| `ParticleInstance`, `ParticleBlendMode` | `flint_render::particles` | the GPU layout and the pipeline key; flint-particles builds them |
| what `ParticleSync` hands the frame (`FrameInputs::particles: Option<&ParticleSync>`) | `FrameInputs::particles: &[ParticleDraw]`, flint-render's plain draw (instances, texture name, blend, sort key, frames, soft distance, fades, lighting, fog, stretch: today's `ParticleDrawData` plus what `ParticlesState::of` reads) | particles fills it in its `frame_inputs` hook; a frame capture's `particles.bin` is written from the same slice |
| `TerrainChunk` | `flint_render::terrain` | mesh data `load_terrain` already takes as a slice |
| `Terrain`, `TerrainConfig`, `Heightmap` | stay in flint-terrain | the renderer stops building terrain: `load_scene_terrain` and `LoadedTerrain` move to the terrain package's asset maker (below) |
| `GrassConfig` | `flint_render::grass` | grass is a render feature until Phase 4; this ends living's and debug-hud's edges to terrain |
| `GroundField` and the height grid it samples | flint-geometry | a placed height field any game can supply (ADR 0095); the projector, `terrain_height`, physics' heightfield and grass all read it |

- **Terrain is an asset maker** (`Registry::asset_maker`, #1557). At the
  scene's `scene` moment, which comes before the model loader in the player
  and in a capture alike, it builds the chunks and the grass on a terrain
  into the lent renderer. It also publishes each terrain's `GroundField`.
  Physics registers a static heightfield for each ground at its scene build,
  which comes later in `enter_scene`, as `scene_loading::register_terrain_collider`
  does today. flint-terrain re-exports `GrassConfig` and `TerrainChunk` from their
  new homes for one release, so a game's `use flint_terrain::GrassConfig`
  still builds.
- **Procgen's `terrain_v1` generator** becomes terrain's `[with.procgen]`
  part, registered on procgen's generator point (ADR 0119). It wraps
  terrain's own bake.

### 6. The dependency rule, as #1624's check reads it

**Input.** `cargo metadata --format-version 1 --no-deps` gives each
workspace member's declared dependencies, with `kind`, `optional` and the
crate's `features`. Every `packages/*/package.toml` gives each crate's
package. A crate belongs to the package whose `[provides] crates` names it.
The `engine` manifest's crates are the kernel's. A member no manifest names
(`tools/arch-analyzer`, a generated `launch/<id>/` launcher) is held to
nothing.

**What is held.** Every dependency of kind normal or build, whether optional
or not, for every target. Dev-dependencies are not held, because a test
links what it tests.

1. **A kernel crate depends on no package's crate.** The one allowance is a
   line inside the `# package-sync:dependencies` block of
   `crates/flint-player/Cargo.toml` or `crates/flint-cli/Cargo.toml` (the
   stock launchers' list, ADR 0137 section 5), and that line must be
   `optional`.
2. **A crate of package P depends only on** kernel crates, on P's own
   crates, on crates of a package that P's `[requires]` names, and, as an
   `optional` dependency turned on only by features that P's `[with.Q]
   features` names, on crates of Q. `[requires]` is read as written and not
   closed over. Rhythm uses audio's crate, so rhythm lists `audio`, even
   though music already requires it.
3. **Exceptions** are one list in the check: crate, dependency, reason and
   the issue that ends it. The check fails on an exception whose issue is
   closed, and on one whose edge the graph no longer has, so the list can't
   outlive its edges.

A failure names the crate, the dependency, both packages and the fix: "add
`Q = "*"` to P's `[requires]`, or make it optional under `[with.Q]`". The
two cases #1624's done-when names fail under rules 1 and 2:
`crates/flint-render` gaining flint-physics, and `packages/music` depending
on rhythm's crate.

**Exceptions this record grants.** There is one, flint-procgen → flint-wooze,
until #1659 moves the part into living. Every other edge in the Context's
table ends with its own ticket, and the check lands last (#1624), so none
of them needs an exception. A move that can't finish its edge adds an
exception with its issue in the same PR.

### 7. Defaults, the 2d template, and `package used`

- **The standard packages stay on unless set to `= false`** (ADR 0136
  section 4). `first-person` and `rig` are new standard packages, so a game
  that writes nothing keeps its walk and its skeletons. Each gains a `[build]
  features` switch in its move, and `false` on it then stops being an error.
- **Each package's policy name** (`physics`, `audio`, `particles`) is its
  own, registered with `Registry::policy`. Its manifest lists it under
  `[without] policies`, so a game state that names it still registers in a
  build without the package (as living's does, #1585). `flint_runtime::state::PHYSICS`
  and the other two stay as constants of the names. `ANIMATION` and
  `SCRIPTS` stay the kernel's.
- **`flint init --template 2d`** writes `physics = false`,
  `first-person = false`, `rig = false`, `particles = false` and
  `terrain = false` under `[packages]`, each with a comment naming what the
  line takes away (`# raycasts, 2D velocity, nav: delete to get physics
  back`). It keeps `audio`, `procgen` and `debug-hud`. The epic's gate job
  (#1624) adds `audio`, `procgen` and `debug-hud = false` on top, so the gate
  still builds and plays on the kernel alone.
- **`flint package used`** already prints a `name = false   # nothing scanned
  uses it` line for each standard package that has a build switch and that
  nothing scanned uses (#1536). Each move turns the switch on, so the line
  appears with no code change, and `character_controller` and `skeleton`
  name their new owners through the manifests. `--write` still writes no
  `false` line.

### 8. Sizes are context

Each move reports in its PR what it takes out of a build that leaves the
package out: the crates in `cargo tree` and the release player's bytes. No
budget, check or gate reads them (Chris, 2026-11-27; the Context's counts
are the starting point).

### The six packages

Checked against `packages/engine/package.toml` at `71cb690d`. Every
component, archetype and script module in its physics, audio, animation,
particles and terrain groups is placed below or kept by the kernel.

| Package | Tier, requires | Crates | Components | Archetypes | Script modules | Stages | Resources |
|---|---|---|---|---|---|---|---|
| physics | standard | flint-physics | `collider`, `rigidbody`, `joint` | | `physics`, `physics_2d`, `nav` | `Scripts`; step `SyncIn`, `Integrate`; builder: `initialize`, heightfields; `scene_unloading`: `clear` | `PhysicsSystem` |
| first-person | standard, `physics` | flint-first-person (new, from `character.rs`) | `character_controller` | `player` | | `Scripts`; step `Control`; `View` | writes `PlayerView` |
| audio | standard | flint-audio, flint-audio-worklet | `audio_source`, `audio_trigger` | | `audio` | `PreScripts`, `Physics`, `PostPhysics`, `Sound` | `AudioEngine` |
| rig | standard | split from flint-animation (#1613) | `skeleton`, `ik_two_bone`, `bone_probe` | | none of today's 11 unless #1613's record moves one | the clock's track kinds (#1613); `Pose` | its poses and palettes |
| particles | standard | flint-particles | `particle_emitter`, `particle_effect` | | `particle` | builder (effects, textures); `Animation`; `frame_inputs` | `ParticleSystem` |
| terrain | standard; `[with.procgen]` | flint-terrain | `terrain` | `terrain` | `terrain` | asset maker `scene` | publishes the kernel's ground |

The kernel keeps the animation group's `animator` (the clock's component,
beside `sprite_animator`) and the `animation` and `tween` modules, as
#1613's factoring has them, and the `character` archetype (a transform
alone). Vehicle gains `[requires] physics` with #1617.

## Alternatives considered

- **Physics stays the kernel's, called by name.** That was ADR 0134's
  answer, and Phase 3's premise rules it out: a 2D game carries Rapier's 43
  crates, and every other controller has to pose as physics' one.
- **An after-step hook for #1647** (ADR 0140's second option). Once physics'
  state is a resource, every stage lends it, so `Stage::Physics` reads the
  last step's result and the hook would add nothing. A package that needs
  each step's result, not the frame's, can read it at the next step's
  `SyncIn`.
- **`take_module_state` for #1648**, so a package reads at `Stage::Scripts`
  what its functions recorded. It splits one call into two paths (state for
  some functions, commands for others), and still doesn't reorder a
  command. A mark on the command keeps one path.
- **Moving every module command to the early drain.** It's simpler, but
  `set_velocity_2d` and the audio calls would act a frame earlier, and no
  golden or playtest would stay byte-identical across the moves.
- **`PlayerView` in flint-package.** flint-script can't depend on
  flint-package (flint-package depends on it), and `is_near_player` reads
  the player. flint-runtime is below both.
- **A `player = true` field instead of the first `character_controller` by
  name.** ADR 0134 refused it for reasons that still hold. A controller says
  who its player is; `set_player` covers a script's own.
- **A `camera` package now**, with first-person, chase and orbit as kinds.
  Only first-person exists. Chase is vehicle's (#1631) and orbit is
  unwritten, so the shared package waits for a second client.
- **The plain particle types in flint-core** rather than flint-render. They
  are the GPU's layout (`bytemuck`, the pipeline's blend key), so they
  belong with the pipeline. flint-particles already sits above flint-render
  in the order.
- **Closing `[requires]` over in the rule.** It would let a package lean
  on a crate that its manifest never mentions, and the use would stop
  building when the middle package dropped its requirement.
- **Allowing an optional reverse edge** (procgen's `[with.living]`). It
  would let a required package grow parts of its requirer, which is the
  tangle the phase removes. #1659 moves the part instead.
- **The 2d template drops every standard package**, as the epic's gate
  reads. A 2D game wants sound and the F3 panels. A template that drops
  audio and debug-hud would teach the user to put them back, so the gate's
  job drops them itself.

## Consequences

- #1614 adds `PlayerView` and moves its six readers; physics writes it until
  #1618. #1615 moves the plain types and the terrain loader. #1616 moves
  `Resources` to flint-core, drops `FixedCtx::physics` and moves vehicle in
  the same PR. #1617 adds `StepStage`, `Stage::View` and the physics
  package, with `contacts_of` and `before_steps`. #1619 adds `Stage::Sound`.
  #1620 and #1621 add `Stage::Pose` with rig. #1622 and #1623 move
  particles and terrain. #1624 is the check, and #1659 ends its one
  exception.
- `Subsystem` gains `step`, and `StageCtx` gains `events` and `capture`.
  Because flint-package is below 1.0 (ADR 0137), each addition is a minor
  change with a changelog line. Three new stages appear in
  `Stage::UPDATES`, and a game extension hears them.
- A package's frame order is the registry's order (requires first, then
  tier, then name). Physics before first-person and particles before the
  feature packages both depend on it. A test in `packages::tests` already
  holds the order, and each move adds its pair.
- ADR 0140's gaps 1 to 3 close with #1616 and #1617. Its section 9 notes
  each as the vehicle reads fresh state.
- ADR 0134's decision 4 is superseded when #1618 merges, and its status line
  says so then.
- `ExtensionCtx::audio` (ADR 0084) moves into the audio package as
  `flint_audio::player::audio(&mut ctx.resources)`. No game uses it outside
  this repository (Float, FlintKart and Kandinski have no Rust of their own
  outside `engine/`), so it gets a changelog line and no forwarding field.

## Measured

- **The rule's prototype** (`cargo metadata --no-deps`, `main` at `71cb690d`,
  2026-11-27) refuses 25 edges, listed in Context. With the planned
  `[requires]` lines (first-person and vehicle on physics, music on audio),
  rhythm's edge to audio still fails until rhythm lists audio itself: that
  is rule 2's "read as written".
- **Dependency counts and lines**, from `cargo tree -e normal` on each
  crate: listed in Context.
- **Call counts** are from ADR 0137's lookups (1.5 ns) and calls (1 ns).
  This record measured no frame, and each move's goldens and playtests are
  its check.

## Decisions

Each one is Chris's. The recommendation is written first.

1. **State as run resources** (decision 1). `Resources` moves to
   flint-core; `FixedCtx::physics` goes, with vehicle moved in #1616's PR
   and no forwarding accessor; `contacts_of` for #1649. Recommended as
   written.
2. **The stages** (decision 2): the four step stages (`Control`, `SyncIn`,
   `Forces`, `Integrate`), and `View`, `Sound` and `Pose` added to the
   frame, with `StageCtx::events` and `capture`. Or an ordered "before /
   after physics" pair of hooks, without names. Recommended: the named
   stages, checked against `step` above.
3. **Script state** (decision 3): reads through lent `Resources`, per-frame
   values as module state, writes as module commands kept at their drain,
   and `before_steps` for #1648. Recommended as written.
4. **The player and view** (decision 4): `PlayerView` in flint-runtime,
   `set_player` for a script's own controller, a `first-person` package,
   camera kinds in the package of what they follow, and ADR 0134 decision 4
   superseded. Recommended as written.
5. **The renderer's plain data** (decision 5): the table above, terrain as
   an asset maker, `GroundField` to flint-geometry, and procgen's
   `terrain_v1` as terrain's `[with.procgen]` part. Recommended as written.
6. **The rule** (decision 6): normal and build edges held, dev not;
   `[requires]` as written; `[with.Q]` for optional parts; generated
   launcher lines allowed; exceptions listed with the issue that ends each.
   Recommended as written.
7. **The one exception**: flint-procgen → flint-wooze until #1659.
   Recommended. The other way is `[with.living]` on procgen, which allows
   reverse edges.
8. **The 2d template** writes five `= false` lines and keeps audio,
   procgen and debug-hud, while the gate's job drops those three too. Or it
   writes all eight `= false` lines, as the epic's gate reads.
   Recommended: five, restating the gate as ADR 0137 decision 5 did.
9. **Sizes reported as context, gated nowhere.** Already Chris's
   (2026-11-27); recorded here.

## As built: the renderer's plain data (#1615)

Decision 5 as it landed. `cargo tree -p flint-render -e normal` shows
neither flint-terrain nor flint-particles, and no golden moved.

- **Particles, as written.** `ParticleInstance` (unchanged: the same fields
  and `repr(C)`, 64 bytes, which `particle_shader.wgsl` and a capture's
  `particles.bin` read), `ParticleBlendMode` and `ParticleDraw` are
  `flint_render::particles`'s; flint-particles depends on flint-render and
  re-exports them. `FrameInputs::particles` is `Option<&[ParticleDraw]>`,
  from `ParticleSync::draw_data()`, and `ParticlesState::of` writes
  `particles.bin` from the same slice. The renderer's own upload type
  (`ParticleDrawData`, four fields) went into `ParticleDraw`, so a replay
  and a host's own draws take the one type. The hosts build the slice where
  they built the frame's inputs, not in a `frame_inputs` hook: the
  particles aren't a package yet (#1622 moves them).
- **`TerrainChunk` and `GrassConfig` are flint-geometry's**
  (`terrain_chunk`, `grass`), re-exported from `flint_render` and
  `flint_terrain`, not defined in flint-render as the table says. Defined in
  the renderer, they would make flint-terrain link the renderer in every
  build, and so flint-physics, flint-script and flint-procgen (which read
  heights, never draw) would carry wgpu. flint-geometry is the kernel's
  plain data both sides read (the ocean's spectrum is there already), where
  `GroundField` is going too.
- **The terrain loader is in flint-terrain behind a `render` feature, called
  by the hosts.** `flint_terrain::scene::bind_terrain(renderer, device,
  queue, world, &mut bound) -> Option<LoadedTerrain>` is `load_scene_terrain`
  moved; `bind_scene` loads only the `grass_ground`, and the terrain's grass
  goes into `bound.grass` unless that holds one, as before. It is not yet
  an asset maker: a maker is a `flint_package::assets::AssetMaker`, and
  flint-package still depends on flint-terrain (through flint-script's
  `GroundField`), so flint-terrain can't depend on it without a cycle. The
  player (`stage_scene_in_renderer`), the viewer (`stage_scene`) and
  `flint render` (`bind_capture_scene`) call it right after `bind_scene`,
  where it ran. #1623 makes it the maker once #1616 and the `GroundField`
  move end the cycle, and drops these calls.
- **Edges this adds and ends.** flint-viewer (kernel) gains a normal edge
  to flint-terrain, beside the flint-particles one it has (#1622); #1623
  ends both terrain calls' edges. Ended here as well as flint-render's:
  flint-living's and flint-debug-ui's edges to flint-terrain (they read only
  `GrassConfig`, now `flint_render::GrassConfig`), and flint-package's direct
  one, assigned to #1616 in the Context's table, for the same reason.

## As built: the API names no standard crate (#1616)

Decision 1's API half as it landed. `crates/flint-package/Cargo.toml` names
none of flint-physics, flint-audio, flint-particles or flint-terrain, and
`cargo tree -p flint-package -e normal` shows no flint-audio and no
flint-particles at all. flint-physics and flint-terrain are still in that
tree, one level down, through flint-script, whose `physics`, `nav` and
ground-field modules #1617 and #1623 move; the API itself reaches neither.

- **`Resources` is flint-core's** (`flint_core::resources`), re-exported as
  `flint_package::Resources`, as written. It gains `Resources::lend(&mut
  value, stand_in, f)`: `value` is the resource of its type while `f` runs,
  then back where it was, with a stand-in made once a run and kept between
  lends under its own key. That is how a host lends state it still owns.
- **Physics, as written.** `FixedCtx::physics` and `pub use flint_physics
  as physics` are gone. `PhysicsSystem` is a run resource for the whole
  run: `GameCore::new` puts it in and the player reads it through
  `GameCore::physics()` / `physics_mut()` (or `physics_of_mut(&mut
  core.resources)` beside another field's borrow). `flint_physics::of` /
  `of_mut` are the lookups, and `PhysicsSystem::fixed_update_in(resources,
  world, dt, events, between)` runs the sync-in, `between` lent the
  resources, then Rapier's step, as `fixed_update_with` did. Every
  `StageCtx` now lends physics too, so #1647's after-steps read is there
  for #1617 to use. Vehicle, the test toy and the fixture toy package moved
  in this PR.
- **The sound is lent, not yet resident.** `SceneCtx::audio` and
  `StageCtx::audio` are gone. The engine stays inside the player's
  `AudioSystem` (17 player files use it, which is #1619's move), and the
  player lends it for each hook with `flint_audio::engine::lend`, a silent
  engine standing in meanwhile. Music and rhythm read
  `flint_audio::engine::of_mut(ctx.resources)`. Moving the engine is cheap
  because its mixer is boxed (the kira manager never moves), and the lend
  happens only when a scene has subsystems.
- **The particles are lent to the panels.** `PanelCtx::particles` became
  `PanelCtx::resources`. The player holds `DebugPanels` out of the
  resources while the panels sync and lends `GameCore::particles` in as the
  `ParticleSystem` the Particles panel reads. #1622 makes it resident.
- **Terrain** was already off the API's own list (#1615).

## As built: particles as a package (#1622)

Decision 1's particles row and the table's particles line, as they landed.
`packages/particles/` holds `flint-particles`, the `particle_emitter` and
`particle_effect` schemas, the `particle` module, the F3 Particles panel and
the effect editor. With `particles = false` neither launcher's `cargo tree`
has `flint-particles`, and flint-render, flint-package, flint-script,
flint-viewer and flint-debug-ui name it in no build.

- **The system is resident.** The package's `run_resources` puts one
  `ParticleSystem` in for the run, where `GameCore::particles` was. Its
  subsystem is built for every scene, since an effect needs no emitter in
  the scene. It loads the scene's effects and finds the world's emitters
  at the scene's build, right where the player loaded them, just before
  the other builders. It steps at `Stage::Animation` under the `particles`
  policy, and clears at `scene_unloading`. `Stage::Animation` now runs
  before the frame's `AnimFx` phase mark, so the step stays timed with the
  animation. The textures need a device at the build, so `SceneCtx` gains
  `gpu` (the window's device and queue, `None` in a capture, a test and a
  playtest, exactly where the player skipped them before).
- **`frame_inputs` doesn't carry the draws; a kernel point does.**
  `Subsystem::frame_inputs` lends `&'s self`, and the draws borrow the
  system, which lives in the resources rather than the subsystem. So the
  kernel declares `flint_render::ParticleSource` (load, step,
  `simulate_to`, the motion stamp, pack, `draw_data`, counts) and
  `flint_package::particles::Particles` (`new`, `of`, `of_mut`,
  `set_drawn`, `emitter_problems`). The package fills the point on the
  player's registry and on the command line. The player's frame stamps the
  motion clock in `bones_posed`, packs before `prepare` and reads the draws
  for `prepare` and for a frame capture through it, each call where the
  frame made it on `GameCore::particles`. `flint render`'s stills,
  sequences and stages, `flint check`'s `bad-emitter`, and the scene
  viewer use the same point. A host without the point draws none, and
  `--particle-time` names the package.
- **Commands, as written.** `EmitBurst`, `PlayEffect`, `StopEffect` and
  `SetEffectParam` became the package's `Particle` payload. The subsystem
  takes them at the drain where the core took them before. A capture that
  draws no particles drops them with the same one line. That line moved
  from `HeadlessGame::refuses`, which named the four functions, into the
  subsystem, which reads the capture's `CaptureDrawn` resource.
- **The policy name is the package's.** It is registered with the package
  and listed under `[without] policies`. `flint_runtime::state::PARTICLES`
  is gone rather than kept as a constant, as decision 7 had it, because
  `scripts/package-names-check.py` refuses a kernel literal that is a
  package's name and nothing read the constant any more.
- **The panel moved to the package.** It moved from debug-hud to the
  kernel's `ScenePanels` point, as vehicle's panel did, so debug-hud needs
  no `[with.particles]` part. Particles registers before every feature
  package, so its panel keeps its place among the F3 panels, right after
  Camera.
- **The editor is a point too.** `flint edit <x>.particles.toml` asks
  `flint_viewer::host::EffectEditors`, as a `.procgen.toml` asks
  `SpecEditors`. Without it the router names the package from the
  manifest that provides `particle_effect`, before the file is read.
- **Three literals are excused.** These are the asset search's
  `particles/` folder (flint-core's resolver and `flint check`'s
  reference walk) and the load report's `particles` row. Each is the name
  of a folder or a load stage, not the package.
## As built: physics as a package (#1617)

Decisions 1 to 3 for physics, as they landed. `packages/physics/` holds the
crate (`crates/flint-physics`), the `collider`, `rigidbody` and `joint`
schemas and a manifest (`tier = "standard"`, `[build] features =
["physics"]`, `[without] policies = ["physics"]`). A project with `physics =
false` builds a player and a CLI whose `cargo tree -e normal` has neither
flint-physics nor Rapier: 28 crates fewer for the player (353 against
381 with the other standard packages), 27 for the CLI (370 against 397).
`flint check` and `flint play` name the package for a scene's `collider`
there (`unlisted-package`), and the player says once that its bodies stand
where they are written.

- **The player names no physics.** `flint_physics::player::PACKAGE` puts the
  `PhysicsSystem` in each run's resources, and its subsystem is built for
  every scene. The builder does what `enter_scene` did by name, in the same
  order: the host's surfaces, the layers and the mesh colliders' triangles,
  `initialize`, then the player into `PlayerView`. The builders now run
  after audio starts, and physics' after the animation and particles
  packages' builders. None of those writes the world, so the bodies are
  made from the same poses. `initialize` returned
  only `Ok`, so a start no longer fails on it; it warns.
- **Three kernel seams the record didn't name.** `Registry::scene_read` is a
  package's look at the scene before the models load. A model of several
  nodes takes its parent's `model`, so physics reads its mesh colliders'
  model names there. `flint_package::surfaces` is plain data the renderer's
  share of the entry leaves in the resources: the splines' swept triangles
  (`Surfaces::meshes`) and each terrain's height grid (`grids`). The builder
  takes it, so no package names flint-render's spline type or
  flint-terrain. `MadeTriangles` is the asset makers' `MadeMeshes`, kept in
  the resources; the player takes it out around a maker's call.
  `ColliderMesh` is now the kernel's `flint_geometry::triangles::Triangles`,
  so a made mesh's `Arc` reaches physics unconverted, and the same `Arc`
  again is still a no-op. `Subsystem::chunk_loaded` / `chunk_unloading` and
  `SceneCtx::model_config` carry a chunk's bodies and the mesh files'
  search.
- **The step, as written.** `StepStage` (`Control`, `SyncIn`, `Forces`,
  `Integrate`) and `Subsystem::step`, whose default calls `fixed_update` at
  `Forces`; the kernel's loop walks the four over the subsystems on each
  step that steps. `Stage::View` comes between the steps and the camera,
  with the view's eye cleared just before. Physics' frame share is at
  `Stage::Scripts`: the triangles, `begin_frame` and the character's look.
  The ocean hulls' velocities are read at `Stage::MadeAssets` and lent
  through `frame_inputs`.
- **Scripts read the resources** (decision 3): `ScriptCallContext::resources`
  and `Call::resource::<T>()` replace the physics pointer, and
  `ScriptSystem::state_scope` takes `&Resources`. `physics`, `physics_2d` and
  `nav` are the package's `ScriptModule`s, enabled in every scene of a run
  with it. `get_joint_position` moved from `entity` to `physics`: it asks the
  simulation. `set_velocity_2d` is a module command the subsystem takes at
  the same drain (`ScriptCommand::SetVelocity2D` is gone), and a capture
  still refuses it by name.
- **Not done here.** `contacts_of` (#1649) and `ModuleCommand::before_steps`
  (#1648) stay open. A capture still runs no physics: its subsystem only
  finds the player by the controller's rule and enables the modules. The
  character controller and `character_controller` stay in this crate and the
  kernel's schemas until #1618. Vehicle has `[requires] physics`, as its
  manifest's note said it would, and stays a package of its own (ADR 0140).
- **Measured, before and after** (debug builds, `main` at `eb4a7126`):
  the walk template's playtest (walk, turn, sprint, strafe, two jumps, a ray
  over the plinth every 15 frames) has the same report at 1/60, 1/30 and
  1/144 s frames. The vehicle lap is 12.092, 12.104 and 12.094 s, identical
  reports. A 30 s FlintKart race (`oval_plus`, five AI karts, rendered) has
  the same 360 kart positions and the same 334,691 draw calls. A run with
  FlintKart's old manifests, which don't know a `physics` package, left
  physics out and differed. That run was the control. All three hold again
  after merging `main` with #1620's animation package and #1622's
  particles package, whose builders now run before physics' (the
  registry's order by name): both only read the world.

## As built: terrain is a package (#1623)

Decision 5's terrain half as it landed. `flint-terrain` is
`packages/terrain/crates/flint-terrain`, with the `terrain` component and
archetype (`packages/terrain/schemas/`), the `terrain` script module and the
terrain editor. A player and a `flint` built with every package but terrain
have no flint-terrain in `cargo tree -e normal`, and flint-package and
flint-script link none at all.

- **`GroundField` and its height grid are flint-geometry's, as written**
  (`ground`, `heightmap`), and so is `HeightGrid`. The grid there is plain
  (`from_raw`, `sample`, `compute_normal`); reading and writing its PNG and
  sampling it in a terrain's frame stay with the terrain, as the
  `HeightmapFiles` trait, so flint-geometry gains no image codec.
- **The scene's terrain is a scene binder, not an asset maker.** A maker
  runs only where a host makes assets: the player always, `flint render`
  only with living props or `--procgen`, the viewer never, and it is lent no
  `BoundScene`, whose grass a terrain's yields to. `Registry::scene_binder`
  (`flint_package::bind`: `BindCtx { gpu, world, bound }`, returning the
  scene's ground) runs right after `bind_scene` in all three, and the hosts
  name no package. A binder belongs to its package, so a project that
  leaves terrain out builds none. The viewer has no player registry: flint-cli
  puts the registry's binders in its points (`SceneBinders`), which the
  viewer reads.
- **"Publishes each terrain's `GroundField`" became the kernel's
  `SceneGround`** (flint-geometry): the entity, the drawn vertex grid and
  where it lies, the heightmap and its world size and scale. The player
  pushes its grid into the kernel's `Surfaces` (#1617), which the physics
  package makes a static heightfield of, and keeps the ground as a run
  resource, which the terrain's subsystem hands `terrain_height` as module
  state. It is not given to the projector: `screen_to_heightfield`
  and `ground_height` still read only a game's own ground (ADR 0095), as
  before; #1681 asks whether a terrain should be the default ground. Three
  spellings of `"terrain"` stay in kernel source as scoped
  `package-names-check.py` exceptions, which #1680 removes.
- **`terrain_v1` is procgen's `[with.terrain]` part, not terrain's
  `[with.procgen]`.** The generator is registered by
  `GeneratorRegistry::with_built_ins`, which every host asks; as terrain's
  part it would need every host to register it from outside, and terrain's
  crate would depend on procgen's. As procgen's (`flint-procgen/terrain`),
  the hosts stay as they were, and procgen without it says which package a
  `.terrain.toml` needs.
- **The editor is terrain's `cli` part**, on a kernel point
  (`flint_viewer::host::TerrainEditors`) as procgen's spec editors are; the
  kernel keeps `flint terrain-edit`'s arguments and names the package
  without it, so `flint --help` is the same in every build.

## As built: first person as a package (#1618)

Decision 4's `first-person` package, as it landed. `packages/first-person/`
holds the crate (`crates/flint-first-person`), the `character_controller`
schema and the `player` archetype, and a manifest (`tier = "standard"`,
`[requires] physics`, `[build] features = ["first-person"]`). The six action
names are `flint_first_person::ACTIONS`, and `find_player` is the one rule
(the first `character_controller` by name). This supersedes ADR 0134
decision 4, whose status line now says so.

- **The controller works on physics' public API.** `CharacterController`
  takes the `PhysicsSystem` it walks in (`update(input, world, physics,
  dt)`), and reads the body and collider through `body_of`, the new
  `collider_of` and `gravity`, and sweeps with Rapier's controller over
  `bodies()`, `colliders()` and `queries()`. The arithmetic and its order
  are the old `character.rs`'s, line for line.
- **Physics no longer knows the component.** `update_kinematic_bodies`
  skipped every entity with a `character_controller` by name. It now skips
  the bodies a controller named with `PhysicsSystem::drive(entity)`, which
  `CharacterController::enter` calls for its player and `clear` forgets.
  A second `character_controller` (`second-player`) is no longer skipped:
  its kinematic body follows its transform, as any kinematic body's does.
- **The stages.** The builder runs after physics' (requires first, so the
  bodies exist), finds the player, faces its `start_yaw`, drives its body
  and writes `PlayerView` (player, mouse look, controls). A scene with no
  player gets no subsystem. `Stage::Scripts` takes the look and the jump
  press after physics' `begin_frame`, under the physics policy; step
  `Control` walks, before physics' `SyncIn`; `Stage::View` places the eye.
  A capture's builder sets the view's player alone and makes nothing.
  Physics' subsystem lost the four, and its builder no longer writes the
  view. The walk's profiler scope is `first_person::walk`.
- **The order.** The registry places first-person right after physics
  (`engine, animation, debug-hud, flipbook, particles, physics,
  first-person, procgen, terrain, ...`), so every stage meets physics' share done,
  as the old calls inside physics did.
- **`physics = false` now needs `first-person = false` beside it**, the
  rule `flipbook` and `animation` already follow (#1620): a standard
  package that requires a package set false is a `bad-manifest` naming
  both lines. #1624's 2d template writes both.
- **A package's archetype in a build without it.** A player built without
  first-person read the walk template's `player` archetype as
  `unknown-archetype` beside the package's one line. `[provides]
  archetypes` now reach the built-without list (`Without::archetypes`,
  `BuiltWithout::archetypes`), so the summary leaves that to the line as it
  does a component.
- **Not done here.** `flint_scene::diagnose` still raises `second-player`
  by the component's name, and `flint_core::components` still holds
  `CHARACTER_CONTROLLER`: the kernel's scene pass carries the package's
  rule, as it carries physics' `lone_collider` (filed as #1688).
- **Measured, before and after** (debug builds, `main` at `08afa839`): the
  walk template's playtest (walk, sprint, turn, strafe, two jumps, a step
  back) has the same report, perf aside, at 1/60, 1/30 and 1/144 s frames,
  and a rendered run at 1/60 the same five 960x540 screenshots, byte for
  byte (0 of 2,592,000 pixels). With `first-person = false` the same run
  passes with the player standing at its start, and the loader names the
  package: that run is the control. All of it holds again after
  merging `main` with #1623's terrain package.
