# ADR 0137: The public Package API: `Subsystem` promoted, typed resources, a fixed-step hook, one registry, generated launchers

Status: proposed, 2026-11-25 (#1549). Waits on Chris's word on each item
under "Decisions" at the end. Epic #1548 (Modularity, Phase 2a).
Builds on [ADR 0117](0117-player-subsystems-and-one-game-step.md) (subsystems
and one game step), [ADR 0133](0133-a-game-binary-adds-cli-commands.md) (a
game binary's `flint` commands) and [ADR 0136](0136-the-package-model.md)
(the package model). Beside [ADR 0071](0071-player-extensions-in-a-game-binary.md)
(player extensions).

## Context

ADR 0136 decision 2 says that from Phase 2 a launcher lists the packages and
hands each one to the kernel through a public `Package` API, and that
`flint-player` names no package. Today none of that API exists:

- **`Subsystem` is `pub(crate)`** in `crates/flint-player/src/player_app/subsystem.rs`,
  with its four contexts (`SceneCtx`, `StageCtx`, `EncodeCtx`, `ReportCtx`).
  Code outside `flint-player` can't implement it. That includes `vehicle`
  (Phase 6, which starts as soon as 2a's gate passes) and a game's own
  package.
- **The contexts lend package-owned state under `cfg`.** `SceneCtx` and
  `StageCtx` hold `resolver: &mut ProcGenResolver` (feature `procgen`) and
  `living: &mut LivingProps` (feature `living`), and `StageCtx` holds
  `panels` (feature `debug-hud`). `EncodeCtx` and `ReportCtx` repeat the
  first two. `CaptureStart::wooze`, `SceneSettings::{living, wooze}` and
  `LaunchOptions::compare` are living's, inside kernel structs. On this
  branch, `crates/flint-player/src` has 53 `cfg(feature = "living")` lines
  in 18 files and 145 `cfg(feature = "debug-hud")` lines.
- **Registration is a build script.** `crates/flint-player/build.rs` lists
  every file in `player_app/subsystems/` and reads three constants from
  each: `POLICY`, `SCRIPT_MODULES` and `DEBUG_KEYS`. That works only for
  files inside `flint-player`.
- **No hook reaches the fixed physics step.** `GameCore::tick_fixed_physics`
  (`player_app/game_core.rs`) runs the character controller and Rapier's
  `fixed_update` once a step, between `Stage::Scripts` and `Stage::Physics`.
  No subsystem hears a step, and `StageCtx` lends no physics. A raycast
  vehicle needs both.
- **The leave-out rule is spread over the package code** (#1528).
  `subsystems/living.rs` and `subsystems/music.rs` each ask
  `ctx.leaves_out(..)`, `procgen_assets::resolve_scene` asks
  `core.leaves_out("living")`, and `missing_features.rs` writes one line for
  each package by name.

Seven subsystems are real (not tests): music, rhythm, living, field,
expression feed, inspect and world dump. Their shapes are known well enough
to write the API down. ADR 0117 named borrowing as the hard part, and this
record is mostly about borrowing.

## Decision

### 1. Two traits in a new kernel crate, `flint-package`: `Package` registers, `Subsystem` runs

`flint-package` (ticket #1550) is a kernel crate that depends only on crates
below `flint-player`: flint-core, flint-ecs, flint-runtime, flint-script,
flint-render, flint-audio and flint-physics. flint-physics stays there until
Phase 3 makes physics a package. It holds `Stage`, `Subsystem`, the
contexts, `Resources` (decision 2) and `Registry` (decision 4). A package's
crates depend on `flint-package` and never on `flint-player` or `flint-cli`.
Otherwise the stock binaries could not depend on the packages, because
Cargo refuses a dependency cycle.

```rust
/// What a launcher lists: one value per package, named as its package.toml is.
pub trait Package: Sync {
    fn name(&self) -> &'static str;
    fn register(&self, registry: &mut Registry);
}

/// What runs: today's trait, made public (section 3 lists what changes).
pub trait Subsystem: Any { /* the hooks */ }
```

A package's crate exports `pub const PACKAGE: &dyn flint_package::Package`.
Registration happens once a run, before any scene loads, and goes in
`[requires]` order (ADR 0136 decision 3; ties go by name). So `procgen`'s
registration points exist before `living` adds to them. Every package
calls `register`, including one the project's `[packages]` leaves out. What
the project leaves out is decided at run time (decision 8), so one binary
serves every project that lists a subset of it.

`flint_core::package::Package`, the parsed manifest, is renamed
`flint_core::package::Manifest`, so there is only one `Package` in the API.

`flint-package` stays below 1.0 until Phase 5 (ADR 0136 decision 5). Its
version is the engine API version that every `package.toml`'s `engine =`
range names.

### 2. One package lends state to another through typed resources

The kernel keeps a `Resources` map, `GameCore::resources`, which lives for
the run. It holds one value per Rust type, keyed by `TypeId`, and every
context lends it:

```rust
impl Resources {
    pub fn insert<T: Any + Send>(&mut self, value: T);
    pub fn get<T: Any>(&self) -> Option<&T>;
    pub fn get_mut<T: Any>(&mut self) -> Option<&mut T>;
    /// Two at once (living's props and procgen's resolver), on
    /// `HashMap::get_disjoint_mut`; `None` if either is missing.
    pub fn get_two_mut<A: Any, B: Any>(&mut self) -> Option<(&mut A, &mut B)>;
}
```

- **The type is the contract.** `procgen` inserts its `ProcGenResolver`.
  `living` names that type because its crate depends on flint-procgen, and
  `[requires] procgen` guarantees it is there. The kernel names neither.
- **A missing resource means the package isn't in the build.** The answer is
  `None`, which replaces today's `#[cfg(feature = ..)]` field. For example,
  rhythm asks for `DebugPanels` (debug-hud's) and adds no panel when the
  answer is `None`.
- **Scene-scoped state stays in the subsystem** and is dropped with it.
  `Resources` holds what outlives a scene (the props' pose cache, the
  resolver's specs) and registration points (decision 4).
- **What it costs.** On this machine, in a release build, a `get_mut`
  takes 7.0 ns with std's SipHash and 1.5 ns with a pass-through hasher
  (`TypeId` is already a hash). Getting two at once takes 5 ns, and a
  virtual call takes 1 ns (20 million rounds, three runs each). The map uses
  the pass-through hasher.

### 3. The hooks: what becomes public as it is, and what changes

| Hook | Public as it is? | Change |
|---|---|---|
| `name`, `lasts`, `presentation_only` | yes | |
| `scene_loaded`, `scene_unloading` | yes | `SceneCtx` loses `resolver`, `living`, `scene: &SceneSettings` and `launch: &mut LaunchOptions` and gains `resources`; `capture` becomes a kernel `&Capture` (`moment`, `scripts`), with a package's own capture inputs (living's wooze field) in `resources` |
| `update(Stage, &mut StageCtx)` | yes | `StageCtx` loses `resolver`, `living` and `panels` and gains `resources`; `note` becomes `flint_package::Note`, a kernel type |
| `script_modules`, `script_command` | yes | |
| `debug_keys`, `on_debug_key` | yes | the file constant `DEBUG_KEYS` becomes a `Registry` call |
| `on_input(PlayerInput, ..)` | changes | `PlayerInput::Click` carries `flint_package::Button` and `pressed: bool`, not winit's `ElementState` and `MouseButton`, so winit is not in the API |
| `frame_inputs(&FrameCtx, &mut FrameInputs)` | yes | `FrameInputs` is flint-render's (kernel until Phase 4) |
| `post_encode(&mut EncodeCtx)` | yes | `EncodeCtx` loses `living` and gains `resources`; wgpu stays in the API, as flint-render already exposes it |
| `ui` | changes | `ui(&mut egui::Ui)`, the frame's root `Ui`, as `PlayerExtension::ui` has been since ADR 0121; egui is re-exported as `flint_package::egui` (decision 6) |
| `exit_report(&ReportCtx, ..)` | yes | `ReportCtx` loses `resolver` and `living` and gains `resources: &Resources` |
| **new** `fixed_update(&mut FixedCtx)` | | decision 3 below |

`Stage` and `Stage::label` become public in `flint-package`.
`flint_player::Stage` re-exports it, so a game's `PlayerExtension::update`
compiles unchanged. `Stage` is already `#[non_exhaustive]`.

### 3a. The fixed-step hook: inside each physics step, between the sync-in and Rapier's step

`Subsystem::fixed_update(&mut self, ctx: &mut FixedCtx)` runs once for each
fixed step that actually steps. These are the steps of the `while
self.clock.should_fixed_update()` loop in `tick_fixed_physics` where
`steps && config.runs(PHYSICS)` is true (#727). Within a step the order
becomes:

1. `physics.update_character(input, world, dt)` (when there is an FPS player), as today
2. `PhysicsSystem::fixed_update`'s sync-in: new bodies and joints, kinematic poses, colliders, joint targets
3. **each subsystem's `fixed_update`**, in the order they were added
4. Rapier's step (`physics_world.step`), then this step's collisions onto the bus, as today

Running before Rapier's step is what a raycast vehicle needs. Rapier's
`DynamicRayCastVehicleController::update_vehicle` casts its wheel rays
against the query pipeline, which still shows the last step's colliders, and
applies forces that Rapier's step then integrates. Running after the sync-in
means a body spawned this frame already has its handle. #1551 splits
`PhysicsSystem::fixed_update` into a sync-in and a step-and-publish so the
hook fits between them. `fixed_update` itself stays and calls both, so with
no hook the frames are unchanged.

```rust
pub struct FixedCtx<'a> {
    pub world: &'a mut FlintWorld,
    /// Bodies by entity, Rapier's body and collider sets, the query
    /// pipeline; rapier3d re-exported as `flint_package::physics::rapier`.
    pub physics: &'a mut PhysicsSystem,
    pub dt: f64,
    /// This step's index in the frame and the steps the frame runs (0-based).
    pub step: (u32, u32),
    /// Held actions, as `update_character` reads them.
    pub input: &'a InputState,
    /// Events a step raises (contacts already go here).
    pub events: &'a mut EventBus,
    pub policy: &'a StateConfig,
    pub resources: &'a mut Resources,
}
```

It does **not** lend the scripts, the renderer, the camera or the sound. A
step is not a frame: a package reacts in scripts and sound at the frame's
stages (vehicle state for scripts at `Stage::PreScripts`, wheel poses at
`Stage::Physics`). `Stage::FixedStep` is added between `Scripts` and
`Physics`. As with `PreRender`, `update` never hears it. It is named there
so the schedule is complete in one place. A capture (`HeadlessGame::capture()`)
runs no physics, so no `fixed_update` runs there either. A paused state
skips it, as it skips Rapier.

**Checked against `tick_fixed_physics`** at `d2c15ad8`.
`character_frame_input` runs once before the loop, `update_character` and
`fixed_update` run in the loop under `steps && config.runs(PHYSICS)`,
`consume_fixed_step` runs on every iteration (including the skipped ones),
and the first-person camera follows after the loop. The hook goes inside
`fixed_update`, after `update_character`, and changes none of the rest.
`fixed_steps` still counts the steps that ran.

PhysicsSystem gains what a vehicle reads (`body_of(entity)`,
`bodies_mut()`, `colliders()`, `queries()`). These are flint-physics
methods, not a new kernel layer. Phase 3 moves `FixedCtx::physics` into
physics' own resource when physics becomes a package.

### 4. Registration: one `Registry`, filled by each package's `register`

| What | How a package registers it | Today |
|---|---|---|
| Scene subsystems | `registry.scene_subsystem(build)`, where `build: fn(&mut SceneCtx) -> Option<Box<dyn Subsystem>>` | `for_scene` in a file of `player_app/subsystems/`, listed by `build.rs` |
| Policy names | `registry.policy("living")` | the file's `POLICY` |
| Script modules | `registry.script_modules(&[&MODULE])`, declared for every scene and enabled while the subsystem runs (#698) | `SCRIPT_MODULES`, gathered by `flint_player::script_system()` |
| Debug keys | `registry.debug_keys(GROUP)` (#717) | `DEBUG_KEYS` |
| Styles | `registry.styles(&[..])`, into `GameCore::styles` (ADR 0129) | `PlayerExtension::styles` only |
| Run resources | `registry.resource(value)` | fields of `GameCore` under `cfg` |
| Registration points of another package | `registry.resource_mut::<T>()` on a type the other package inserted: procgen's `GeneratorRegistry` (ADR 0119), debug-hud's `DebugPanels`, music's chart judge (rhythm), procgen's resolve steps (living's props) | `cfg` blocks in the player |
| Scene settling | `registry.settle_scene(fn(&mut toml::Table, &Project))` (living's `[wooze]` presets) | `flint_procgen::settle_scene` under `cfg` in main.rs, flint_web.rs and `apply_scene_settings` |
| Player flags | `registry.player_args::<A: clap::Args>()`, flattened into `flint-player` and `flint play`, with the parsed value inserted as a resource (living's `--compare`, `--split`) | `PlayerArgs` and `LaunchOptions::compare` |
| Schemas | not registered in 2a: they stay the `schemas/` folder beside `package.toml` (ADR 0136 section 8); Phase 2b embeds them in the package crate (with #1529) | the same |
| Generators | procgen's registration point (row above) | `PlayerExtension::register_generators` |
| Debug panels | debug-hud's registration point; the F3/F4 machinery moves with debug-hud (#1559) | `create_scene_debug_panels` under `cfg` |
| CLI commands | `CliExtension` (ADR 0133), unchanged in shape: a package's CLI side exports `pub fn cli() -> Box<dyn CliExtension>`, and the CLI launcher hands those to `run_with` | the engine's `Commands` enum under features (#1333) |

`build.rs`'s subsystem listing goes away. The kernel's own subsystems (field,
expression feed, inspect, world dump) register through the same `Registry`,
from `flint-player`'s own registration function, so the kernel has no
second path. The test toy in `subsystems/toy.rs` becomes a registration
inside a test.

`CliExtension` and the output helpers it re-exports (`emit`, `Answer`,
`Report`, `document`, `failed`, `json`, `say`) move to `flint_package::cli`,
so a package's CLI code doesn't depend on flint-cli. `flint_cli::extension`
re-exports them, so ADR 0133's game binaries compile unchanged (#1553).

### 5. The launchers

**How a package joins the workspace.** The root `Cargo.toml` gains
`"packages/*/crates/*"` in both `members` and `default-members`, once, in
#1552. After that, a package's crates at `packages/<name>/crates/<crate>/`
join with no edit to it. Measured with cargo 1.99.1, both lists take the
glob, and a package folder with no `crates/` matches nothing without
complaint. flint-android stays out of `default-members` by name.

**The stock binaries stay where they are** (`flint-player` in
`crates/flint-player`, `flint` in `crates/flint-cli`), so `cargo run --bin
flint-player`, `cargo run --bin flint`, the 69 flint-cli integration test
files that spawn `CARGO_BIN_EXE_flint` and the games' `build.bat` lines don't
change. Each crate's `Cargo.toml` carries a **generated block**: one optional
dependency per package crate, and one feature, `stock`, on by default, that
turns them all on. That is the "no `[packages]` key means every package" of
ADR 0136 decision 3. The libraries name no package: only the binaries use
the block, through a list `build.rs` writes into `OUT_DIR` from the
`CARGO_FEATURE_*` it sees.

- `scripts/package-sync.py` writes the blocks from `packages/*/package.toml`.
  It is a script rather than a `flint` command because after a package
  folder is deleted, every cargo command fails on the missing path until the
  block is rewritten. `scripts/ci.sh` runs it with `--check`, and its failure
  names the command.
- So **adding a package is creating `packages/<name>/` and running
  `scripts/package-sync.py`**. That command rewrites the two generated
  blocks, and the next cargo command adds the crate to `Cargo.lock`.
  Removing a package is deleting the folder and running the script again.
  This restates the epic's gate (decision 5 asks).

**A game's launcher is generated into the engine workspace**, by `flint
build`, at `engine/launch/<game id>/`. That folder is gitignored, and
`launch/*` joins the members glob (`launch/README.md` is committed, because a
glob whose folder is missing fails the whole workspace: `os error 3`,
measured). The generated crate has bins `flint-player` and `flint`, and
depends on `flint-player` and `flint-cli` with `default-features = false`,
on the engine packages the game's `[packages]` lists, and on the game's own
packages by path (`../packages/<name>/crates/<crate>`, outside `engine/`).
`flint build` runs cargo from `engine/`, so the engine's `.cargo/config.toml`
applies (the `/STACK` flag, #1338), and afterwards puts `engine/Cargo.lock`
back byte for byte, so a game's subtree stays clean. Measured with a scratch
workspace: kern (a kernel crate), toy (a package) and kart (a game-local
crate):

- **The launcher as a member of the engine workspace:** kern and toy are
  `Fresh`, and only the launcher compiles. A game-local crate outside the
  workspace root works as a path dependency, and the engine crates stay
  `Fresh`.
- **The launcher in its own workspace in the game's repo** (with the engine's
  lock and profiles copied, cargo run from `engine/` with the engine's
  `--target-dir`): registry crates are `Fresh`, but **every engine path crate
  recompiles**, kern and toy here. A crate's build depends on whether it is a
  member of the workspace being built. In the real engine that is all 33
  crates, which is why the launcher lives in `engine/`. Run from the game's
  root, without the engine's config, the registry crates recompile too.
- **Putting `Cargo.lock` back** after a build makes the next build re-add
  the launcher's lines, and nothing compiles: every crate `Fresh`.

Until a package has moved (#1554-#1559), the launchers turn it on with
today's Cargo features (ADR 0136 decision 2). The generated block and the
features live side by side during 2a.

### 6. egui stays in the API, as the root `Ui`

`ui(&mut egui::Ui)` matches `PlayerExtension::ui` (ADR 0121). egui is in the
kernel anyway: the script HUD draws through it, with or without debug-hud.
`flint_package::egui` re-exports the engine's egui so that a package builds
against the same version. One package uses `ui` today: living's A/B labels.
Putting egui behind debug-hud would leave a package with no egui at all for
a window of its own, while the kernel still links egui.

### 7. `PlayerExtension` stays its own trait, built from the same lent parts

A game binary's `PlayerExtension` (ADR 0071) keeps its signatures, so no
game breaks. `ExtensionCtx` is built from the same `Lent` and core fields as
`StageCtx`, and gains `resources`, so a game's extension can read a package's
state (a vehicle's wheel contacts). The walk stays separate: at each stage
the packages' subsystems run first, then the extensions (ADR 0117
decision 5, #754). **New hooks go on `Subsystem` only.** `PlayerExtension`
grows no further, and a game that needs more writes a game package
(Phase 2b, `flint init --package`).

### 8. Two lists, and one place that leaves a package out

- **What the binary was built with:** `Registry::built_with() -> &[&str]`,
  the names of the packages whose `register` ran.
- **What the project lists:** `flint_core::package::InUse`, as today.
  `GameCore::packages` holds it for the player, and
  `SchemaRegistry::packages()` holds the one the schemas were read under.

The kernel applies the project's list in one place, so no package asks
`leaves_out` for the common paths:

- `build_for_scene` skips the builders of a package that `InUse` leaves out;
- `enable_script_modules` drops its modules (as today);
- the debug-key table drops its keys;
- `DebugPanels` refuses its panels.

The one line a scene gets is written by the kernel, from the manifest's
`[provides]` (its components and scene tables, counted in the scene's world
and tables). `missing_features::left_out_line` becomes generic, and
`missing_features.rs` names no package. A package's registration-point code
that runs inside another package (living's resolve step in procgen) still
asks `ctx.packages`. A capture's `HeadlessGame` gets the same `InUse`
when #1528's capture wiring lands.

### 9. A package's optional part for another package

The part lives with the package that needs it. Cargo's feature unification
then does most of the work. `flint-living` depends on
`flint-procgen = { features = ["living"] }`, so procgen's `living` part is on
whenever living is linked, and procgen's manifest says nothing. When the
living code moves out of flint-player, flint-viewer, flint-debug-ui and
flint-cli, their `living` features go with it.

A part that should exist only when **both** packages are listed, and neither
requires the other, is declared in the manifest of the package that holds
it:

```toml
# packages/living/package.toml
[with.debug-hud]              # when debug-hud is listed too
features = ["flint-living/debug-hud"]
```

The launcher turns on each `[with.<name>]` feature whose name the game lists.
Living's `debug` script module, which needs debug-hud (#854), is the first
example.

### 10. `package.toml` names a package's Rust

```toml
[rust]
player = "flint-music"          # the crate whose PACKAGE the player launcher lists
cli = "flint-music/cli"         # optional: crate[/feature] whose cli() the CLI launcher lists
```

A package with no `[rust]` has no Rust: schemas, scripts and assets only.
`scripts/package-sync.py`, `flint build` and `flint package list` read these
lines. An unknown key is an error, as everywhere else in the manifest.

### What each of the seven subsystems changes

| Subsystem | Where it goes | What changes for it |
|---|---|---|
| **music** (`music::MusicSubsystem`, builder in `subsystems/music.rs`) | `packages/music/` with flint-music (#1555) | Registered by music's `register`; the builder's `leaves_out` check and `music_left_out_line` go (decision 8). It reads only kernel fields (clock, script, capture, audio, world, scene path, project), so its hooks don't change. It inserts a chart-judge registration point, which rhythm fills, so its builder no longer has to ask whether the build has `rhythm`. |
| **rhythm** (`music::rhythm::RhythmSubsystem`) | `packages/rhythm/` with flint-rhythm and flint-input-capture (#1556) | Fills music's chart judge at `register`, so music's builder hands it the charted session. `ctx.panels` becomes `resources.get_mut::<DebugPanels>()`, `None` without debug-hud. `DEBUG_KEYS` and the `conducted` module become `Registry` calls. Pads, note, audio, styles, requests, quit and input stay kernel fields. |
| **living** (`LivingSubsystem`, `living_tuning.rs`, `living_ab.rs`) | `packages/living/` with flint-living and flint-wooze (#1558) | The most work. `ctx.living` and `ctx.resolver` become `resources.get_two_mut::<LivingProps, ProcGenResolver>()` in `StageCtx`, `SceneCtx`, `EncodeCtx` and `ReportCtx`. Props are resolved at load through procgen's resolve steps, not `procgen_assets`' `cfg` block. `SceneSettings::{living, wooze}` are read from `ctx.tables` and settled by its `settle_scene`. `--compare`/`--split` become its `player_args`. `CaptureStart::wooze` and `HeadlessGame`'s living methods become resources that flint render's living side sets and reads. The Living Props panel goes in through `DebugPanels`. `ui` takes `&mut egui::Ui`, and `on_input` takes the kernel `PlayerInput`. |
| **field** (`subsystems/field.rs`) | stays in the kernel (`[field]` and the `field` module are kernel, ADR 0136 section 7) | Registered by flint-player's own `register` instead of `build.rs`. Its hooks (`script_modules`, `scene_unloading`) and fields (tables, script, scene path, capture) don't change. |
| **expression feed** (`ExpressionSubsystem`) | stays in the kernel (the `expression` module is kernel) | Still made from `LaunchOptions::expression_feed` at `Start`. `ctx.panels` becomes the `DebugPanels` resource. Its F3 panel (`expression_panel.rs`) moves with debug-hud (#1559) and reads the feed's record, which the subsystem publishes as a resource. |
| **inspect** (`InspectSubsystem`) | stays in the kernel | None to its hooks: it reads world, scene path and clock at `Stage::Sync`. It stays a run subsystem the host makes from `LaunchOptions::inspect`. |
| **world dump** (`WorldDump`) | stays in the kernel | None to its hooks: it reads the same fields as inspect. `run_failure` still finds it with `Subsystems::find`. |

### How many calls a frame

Today, with S subsystems in the scene's list, a player frame makes 13 S
virtual calls: 10 `update`s (`Clock` to `Sync`), `frame_inputs`,
`post_encode` and `ui`. With no subsystem it makes none (`is_empty`). This
record adds S `fixed_update` calls for each step (one a frame at 60 Hz, two
at 30) and the resource lookups a package makes. For living, the heaviest
client, that is about ten lookups a frame. The woozy grove (S = 1) then
costs about 14 calls at 1 ns and 10 lookups at 1.5 ns, about 30 ns of a
16.7 ms frame. The launcher ticket (#1552) measures it on the phone with a
control row (`scripts/perf-compare.py`), as #1549's risk table asks.

## Alternatives considered

- **One trait for registration and running.** A `Package` with every hook
  would make each package object live for the run and be built per scene at
  the same time. The builders already separate "does this scene use me"
  from "run", and #1252's captures depend on that.
- **A dependency handle** (`register` returns a `Dep<ProcGen>` that a context
  resolves) instead of types in a map. It carries the same information as
  the type, plus a handle to thread through every context. The type is
  already unique, because the owning crate defines it.
- **Generic contexts** (`StageCtx<P: Lends>`). Every hook would become
  generic, `dyn Subsystem` would stop being object-safe, and the list would
  become a tuple fixed at compile time. That is ADR 0117's rejected
  scheduler by another name.
- **The fixed hook after Rapier's step**, or at both ends. A vehicle's
  forces have to be in before the integration, and contacts are already on
  the bus for `Stage::Physics`. A second point can be added when a client
  needs one (`Stage` is `#[non_exhaustive]`).
- **Stock launchers in a new crate** (`launch/flint`), with the binaries
  moved out of flint-player and flint-cli. It would keep `Cargo.toml`
  generation out of the two kernel crates, but it renames every `cargo run
  -p` and breaks `CARGO_BIN_EXE_flint` in 69 test files.
- **Gitignored stock launchers, generated on demand** (the epic's gate taken
  literally). Adding a package would then be the folder alone, but a fresh
  clone has no `flint` or `flint-player` until something generates them,
  and every agent's first `cargo run` fails. Cargo reads dependencies only
  from a manifest on disk, so there is no third way: either a list is
  written down, or it is generated before cargo runs.
- **The game's launcher in the game's repo** (its own workspace, path
  dependencies into `engine/`). Measured above: it recompiles every engine
  crate. Making it a member of the engine's workspace from the game's repo
  would need the engine's `Cargo.toml` to name the game.
- **egui behind debug-hud.** See decision 6.
- **`PlayerExtension` folded into `Subsystem`** through an adapter. The
  extension hooks would run interleaved with the subsystems, not after
  them, and `ExtensionCtx`'s fields (persist, rays, fired events) would all
  have to move onto `StageCtx`. That is a breaking change for games, and it
  buys nothing 2a needs.

## Consequences

- #1550 builds `flint-package` and moves the seven subsystems onto it, still
  inside flint-player, with the `cfg` fields replaced by resources. #1551
  adds `fixed_update` and splits `PhysicsSystem::fixed_update`. #1552 writes
  the globs, the generated blocks, `package-sync.py` and the game launcher.
  #1553 moves `CliExtension`.
- From then on a package is a folder with a manifest, its crates and a
  `register`. Adding or removing one runs one script (decision 5 asks
  whether that is acceptable).
- Cargo unifies features, so one in-tree crate that takes `flint-player` or
  `flint-cli` with default features would turn `stock` on in a game's
  launcher and link every package. flint-cli takes flint-player with
  `default-features = false`, and so does the root `[workspace.dependencies]`
  entry, where a member's own `default-features = false` is ignored (#1333).
  #1552's test builds a game launcher and checks that a left-out package's
  crate is not in its `cargo tree`.
- `flint build` writes `engine/launch/<id>/` and restores `engine/Cargo.lock`.
  A game's `target/` then holds the game's `flint.exe`, as it does today.
- `flint-package` is the API surface a package builds against, and it stays
  below 1.0 until Phase 5. A change to a context field is a change to every
  package. ADR 0136's risk ("the API freezes too early") is held off by the
  version number, and by `vehicle`, the first client written against the
  public API rather than moved onto it.
- The test that holds 2a's gate (#1560) greps `crates/flint-player/src` and
  `crates/flint-cli/src` for package names. It does not grep the generated
  blocks in their `Cargo.toml` files, which name every package by design.

## As built: the launcher (#1552)

- `Package` and `Registry` are in `flint-package` (decision 4's rows for
  scene subsystems, policy names, script modules and debug keys, plus
  `when_left_out`: the line a scene hears when the project leaves the
  package out, so the kernel skips a left-out package's builders itself).
  `flint_player::packages::launch(&list)` composes the run's registry once,
  the kernel's own registration (`packages::ENGINE`) first; `registry()`
  is the stock list when no launcher called it.
- The stock `flint-player` and web player call `launch(&stock())`; `stock()`
  lists the packages the build's features have, in `[requires]` order.
  `packages::lacks` (and so `Unbuilt`) asks the registry, not the features.
- `flint build` selects the stock crate's features, and its plan names the
  launcher's packages. For engine packages that is the launcher: a
  generated `engine/launch/<id>/` crate is needed only for a game-local
  package with Rust (Phase 2b).
- Not built yet, as nothing has a crate of its own to join: the members
  glob, `[rust]` in `package.toml`, `scripts/package-sync.py` and the
  generated blocks. They come with the first move (#1568).

## As built: the CLI launcher (#1553)

- `CliExtension`, `subcommands`/`Derived` and the output path (`set_json`,
  `json`, `stdout_reserved`, `say`, `progress`, `Report`, `Answer`, `emit`,
  `document`, `when_json`, `failed`) are `flint_package::cli`. The state
  (`--json`, stdout reserved, a document printed) is one set of statics
  there, so an engine command and a package's answer through the same
  switches. flint-cli's `output` re-exports them and keeps the exit path
  (`finish`, with `MissingPackage` and `Unbuilt`) and `refuse_interactive`.
  `flint_cli::extension` re-exports the rest, so ADR 0133's game binaries
  compile unchanged (`tests/cli_extension.rs`).
- **A package's command can sit under a kernel command.** The ai package's
  commands are subcommands of `asset`, so the trait gained
  `under() -> &[&str]` (default empty: the top level, as ADR 0133 had it),
  and `Derived::under(&["asset"])`. `extension::Added` checks names per
  place (an extension under `asset` may not take `import`, but may take
  `render`), adds each place's commands after that command's own, and
  dispatches by path. `run` gets the matches of the command it sits under,
  which is the root's for a top-level command, as before.
- **A package's `cli()` returns a `Vec<Box<dyn CliExtension>>`**, not one,
  so a package can add commands at two places. The ai package's is
  `flint_asset_gen::cli::extensions()`, behind flint-asset-gen's `cli`
  feature, which flint-cli's `ai` turns on. It holds the six `asset`
  subcommands its manifest lists and the cost ledger (`cli::costs`, once
  flint-cli's `ai_costs.rs`). Its test holds the commands it adds to the
  manifest's `[provides] commands`.
- **The launcher's list** is `flint_cli::stock()`, the extensions of each
  package the build's features have, as `flint_player::packages::stock()`
  is (#1552). `run_with(extensions)` is `stock()` and then the given ones,
  so a game binary keeps the packages' commands, and the stock `flint` is
  `run_with(Vec::new())`. The generated list replaces the `cfg` lines in
  `stock()` with #1568.
- **A left-out package's command is named from its manifest.** A build
  without the package has none of its commands in the parser or in
  `--help`. When clap refuses an unknown subcommand,
  `absent::missing_command` matches the line's words against every
  manifest's `[provides] commands`. `build.rs` compiles in
  `packages/*/package.toml`, so a copied binary knows them too, and the
  source names no package to do it. A match is the same `MissingPackage`
  as a stub's: exit 2, naming the package to list, with `package` in the
  `--json` error document. A stub (`absent::stub!`) is still how a package
  whose commands live in flint-cli keeps them (music, rhythm, procgen). It
  goes when its package's commands move to its crate (#1555, #1557).
- **Package flags on a command the package doesn't own** (ADR 0136
  section 7: `gen --ai-*`, `--max-cost`, `--cost-report`, `--style-guide`,
  `render --ai-refine`, `render --sweep wooze.*|living.*`, `asset resolve
  --strategy ai_*`): decided, not yet built (#1572). Each flag changes how the
  owning command runs, at one point in its run. The owner declares that
  point as a registration point (section 4's "Registration points of
  another package" row): a trait object the flag's package registers. The
  package also brings the flags, as a `clap::Args` group the extension
  adds to the owner's command. The kernel's commands (`render`, `asset
  resolve`) declare theirs in `flint_package::cli`. Procgen's `gen` declares
  its own when `gen` moves to procgen's crate (#1557). Only then do the
  points have a type outside flint-cli to take. Until then these flags stay
  declared by the command, refused with exit 2 naming the package in a
  build without it, as #1333 made them. `--help` is the same in every
  build, and nothing an agent scripts changes. `gen --cost-report` is the
  one flag that is a different answer (it is `asset costs`), not a step of
  `gen`. It stays as it is, so the path keeps working.
- Measured (`stack_tests`, debug, Windows): the parser with the stock
  extensions fits Windows' default 1 MB for `--help`, `render --help`,
  `asset generate --help` and a render line, as before. The derived
  `AiAssetCommands` builds its `Command` at run time, as `Cli`'s derive
  did, so nothing moved onto a deeper frame.

## As built: the first move, ai (#1554)

- **The crates moved, and the glob joined.** `flint-asset-gen`,
  `flint-procgen-ai` and `flint-ai-core` are at
  `packages/ai/crates/<crate>/`. The root `Cargo.toml` names
  `"packages/*/crates/*"` in `members` and `default-members` (section 5),
  and the three member lines went. The workspace dependency paths point at
  the new folders, so no crate that depends on them changed.
- **`[rust] cli` in `package.toml`** names the package's crate whose
  `cli::extensions()` the CLI launcher lists (`flint_core::package`: one of
  `[provides] crates`, else a `bad-package` mistake). This is the `cli`
  half of #1568's `[rust]` table. `player` comes with the first package
  that registers a `Package` from its own crate (#1555).
- **The CLI launcher's list is written by `build.rs`**, from the manifests
  it already compiled in: `stock()` is `include!` of
  `OUT_DIR/stock_extensions.rs`, and `absent::built_features()` reads
  `OUT_DIR/package_features.rs`, so `crates/flint-cli/src` names the ai
  package nowhere in either. flint-cli's `Cargo.toml` still declares the
  `ai` feature and the three optional dependencies by hand. #1568's
  `package-sync.py` generates that block. Until then a new package's crates
  are one hand edit there.
- **Not done: the flags.** `gen --ai-*`, `render --ai-refine` and `asset
  resolve --strategy ai_*` are still declared by their commands, with their
  code in flint-cli behind `feature = "ai"`: 39 lines in
  `crates/flint-cli/src`, against 41 before (the two that went are
  `stock()`'s and the built-features row). The code behind them (`gen_ai`,
  `gen_refine` and `look_refine`, about 1,800 lines) calls flint-cli's
  previews, look measures and sweeps, so it moves with the registration
  points of the "As built: the CLI launcher" section (#1572), not here.
- **Paths that walk the tree** take `packages/*/crates/*` too: the
  arch-analyzer (members globs, the book's crate table), `docs-check.py`'s
  crate and member counts, the env, schema and scene-table tests, the clippy
  tool-crate list and the Play site's path filter.

## As built: the second move, music (#1555)

- **The crate is in the package's folder**: `packages/music/crates/flint-music`,
  joined by #1554's members glob, its workspace dependency path pointed
  there. Its two parts are features: `player` (`player::PACKAGE`) and
  `cli` (`cli::extensions()`).
- **`[rust] player`** joins #1554's `cli` in `package.toml`: the crate
  whose `player::PACKAGE` the player launcher lists. Music's manifest names
  `flint-music` for both. flint-player's feature named for the package
  turns that crate's `player` feature on (`music = ["dep:flint-music",
  "flint-music?/player"]`), as flint-cli's does `cli`, so the Cargo
  features of ADR 0136 decision 2 are still the switch.
  `crates/flint-player/launch_list.rs`, which flint-player's build script
  includes by `#[path]`, reads the manifests and writes
  `OUT_DIR/package_list.rs`: `moved()` (each `[rust] player` crate whose
  package's `[build] features` are all on), the registration `ORDER`
  (requires first, then tier, then name, which is the order the stock list
  had by hand) and the feature tables (`PLAYER_PACKAGES`, `BUILT`). The
  CLI's list is #1554's `stock_extensions.rs`, which lists music's
  commands from its `[rust] cli`. No `package-sync.py` and no generated
  Cargo block yet: the dependency and feature lines were written by hand
  (#1568).
- **Music's registration point for rhythm**: `Registry::insert_point`,
  `point_mut` and `point` (one value per type, `Send + Sync` so the run's
  registry stays a static), and `SceneCtx::registry`, so a builder reads a
  point and asks `has(package)`. Music inserts `ChartJudge`; rhythm,
  still in flint-player, fills it at its `register` with its subsystem's
  constructor. A build without rhythm leaves it empty and a charted suite
  plays as music, as before. The kernel's "built without music" line is a
  builder `ENGINE` registers in every build, which speaks only when the
  registry lacks `music`.
- **The `conducted` module stays declared by music** (as before: music
  enables it, neutral, for a charted scene nothing judges), though the
  manifests name it rhythm's. It moved with music into
  `flint_music::player::conducted`, and rhythm's session writes its
  snapshot there. #1556 decides whether rhythm takes it.
- **One left-out wording**: `flint_package::left_out_line`, which the
  kernel's lines and a package's share; music's is
  `flint_music::player::left_out_line`.
- **The CLI**: `music` is the music package's top-level command
  (`flint_music::cli`, `validate`, `play`, `render`), and rhythm's
  `calibrate` and `chart` are an extension under it. `extension::Added`
  now takes a place that an earlier extension added (the tree is grown
  extension by extension), and dispatches a line to the deepest owner
  (`music chart play` is rhythm's, `music play` music's). `music
  validate` reaches flint-rhythm's contract through
  `flint_music::cli::judge_rhythm_suites_with`, a static that rhythm's
  extension sets when it is listed: the CLI has no typed registration
  points until #1572. `MissingPackage` and `missing` are
  `flint_package::cli`'s, so a package's command can name the package a
  part of it needs. The music and rhythm stubs are gone: a build without
  either has none of its commands, and `absent::missing_command` names the
  package from the manifests. `flint --help` lists `music` after the
  engine's commands, as extensions are.

## As built: the third move, rhythm (#1556)

- **Three crates in the package's folder**: `packages/rhythm/crates/`
  holds flint-rhythm and flint-input-capture, moved as they were, and a new
  one, **flint-rhythm-game**, which is the package's two parts (`player`,
  `cli`) as features, the shape music's crate has. It is its own crate, not
  features of flint-rhythm, because the session needs both of the others
  and flint-input-capture depends on flint-rhythm. The manifest's `[rust]`
  names it for both parts. Its `debug-hud` feature (the guide panels and
  the three keys) is turned on by flint-player's `debug-hud` through
  `flint-rhythm-game?/debug-hud`, the Cargo form of decision 9's
  `[with.debug-hud]`, which no launcher reads yet.
- **The ladder's base is a context field**: `SceneCtx::post`, the scene's
  `[post_process]` as the host read it (its look laid in), in place of
  #1550's lent `ScenePost` resource. The raw `[post_process]` table in
  `SceneCtx::tables` would have lost the look. flint-package takes
  flint-scene for the type, a kernel crate.
- **The pads are lent as they were**: `Pads::take` / `give_back` on the
  context's `Pads`, now from another crate. Nothing on the capture thread
  changed: the same `flint_input_capture::attach_session_input`, the same
  channel and the same drain at `Stage::Clock`.
- **The `conducted` module stays music's to declare**. Music enables it,
  neutral, for a charted scene that nothing judges (a capture, a build
  without rhythm), so a script reading `conducted_lean()` works there too.
  Rhythm's manifest still lists it, since rhythm is what fills it.
- **The CLI list is in registration order**: flint-cli's build script
  includes flint-player's `launch_list.rs` by `#[path]` and lists the
  `[rust] cli` crates requires first, so rhythm's extension, which sits
  `under(&["music"])`, comes after music's whatever the folder names. Its
  tests ask for a list without rhythm by dropping the extension that adds
  `chart`, so they run in every build.
- **Measured**: four synthetic replays of one chart over the tempo-change
  suite (`perfect`, `late:30`, `late:-45`, `neglect`) gave byte-identical
  judgment logs, console text and `--json` documents from a binary built
  before the move and one after. The eight `flint music` help pages are the
  same bytes. `git grep 'feature = "rhythm"' crates/flint-player
  crates/flint-cli` finds nothing.

## Measured

- **Cargo globs** (cargo 1.99.1, a scratch workspace, 2026-11-25): `members`
  and `default-members` both take `packages/*/crates/*`, and
  `workspace_default_members` lists the globbed crate. A package folder with
  no `crates/` matches nothing. A glob whose base folder is missing
  (`launch/*` with no `launch/`) fails every cargo command with `The system
  cannot find the path specified. (os error 3)`. `Cargo.lock` gains a member's
  `[[package]]` entry, and `--locked` then refuses: `cannot update the lock
  file ... because --locked was passed`.
- **Launcher placement**, the same workspace with `.cargo/config.toml`
  rustflags and a `[profile.dev.package.kern]` override, as the engine has:

  | Launcher | engine path crates | registry crates |
  |---|---|---|
  | a member of the engine workspace | `Fresh` | `Fresh` |
  | the same, plus a game-local path crate outside `engine/` | `Fresh` (the game crate compiles) | `Fresh` |
  | its own workspace, cargo run from `engine/`, engine `--target-dir` | recompiled | `Fresh` |
  | its own workspace, cargo run from the game's root | recompiled | recompiled |
  | a member, after `Cargo.lock` was put back | `Fresh` | `Fresh` |

- **Lookups and calls** (release build, this machine, 20 M rounds, three
  runs): `HashMap<TypeId, Box<dyn Any>>::get_mut` with SipHash 6.97-8.10 ns,
  with a pass-through hasher 1.43-1.72 ns; two at once through
  `get_disjoint_mut` 4.84-5.65 ns; a `Box<dyn Trait>` call 0.90-1.02 ns.

## Decisions

Each one is Chris's. The recommendation is written first.

1. **Two traits in `flint-package`** (`Package` registers, `Subsystem`
   runs), with the manifest struct renamed `Manifest`, and package crates
   never depending on flint-player or flint-cli. Recommended as written.
2. **How one package lends state to another:** typed resources (a
   `TypeId` map with a pass-through hasher, `get_two_mut` for two at once),
   a dependency handle, or generic contexts. Recommended: typed resources.
3. **The fixed-step hook:** once a step, after the character controller and
   the sync-in, before Rapier's step, lending world, physics, dt, step
   index, input, events, policy and resources, and no scripts, renderer,
   camera or sound. Recommended as written.
4. **The registration list** in section 4, with schemas left as folders
   until 2b and CLI commands through a relocated `CliExtension`.
   Recommended as written. Package player flags (`registry.player_args`)
   are new and count against #990's dozen-flag rule.
5. **The epic's gate** says a toy package is added "by creating
   `packages/toy/` and nothing else". Restate it as "by creating
   `packages/toy/` and running `scripts/package-sync.py`, which writes only
   generated lines", or keep it literal and generate gitignored stock
   launchers (the bins move out of flint-cli and flint-player).
   Recommended: restate it. #1560 then checks that the diff outside
   `packages/toy/` is only the generated blocks and `Cargo.lock`.
6. **Where a game's launcher lives:** generated into `engine/launch/<id>/`
   (gitignored, a workspace member, `Cargo.lock` put back), or the game's
   repo. Recommended: `engine/launch/`, for the measured reuse.
7. **egui:** keep `ui(&mut egui::Ui)` with egui re-exported, or put it
   behind debug-hud. Recommended: keep it.
8. **`PlayerExtension`:** keep it as its own trait over the same lent
   parts, with `resources` added and no new hooks; or fold it into
   `Subsystem`. Recommended: keep it.
9. **Leaving a package out in one place** (decision 8), with the lines
   written from the manifests. Recommended as written.
10. **`[with.<package>] features`** for a part that needs both packages,
    and `[rust] player` / `cli` naming a package's crates. Recommended as
    written.
