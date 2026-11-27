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

## As built: a game's own binary (#1532)

- Before any generated launcher, a game that embeds `PlayerApp` in a crate
  of its own at its root is built by `flint build` as it is: `cargo build
  -p <game> --no-default-features --features flint-player/<f>,...` in the
  game's root, from the same plan as the stock player. That run is the
  game's own workspace, so it shares no compiled crate with the engine's
  (decision 6's measurement); `engine/launch/<id>/` (#1562) is what
  removes the second build.
- A launcher dependency that would bring a left-out package back is
  refused before anything builds: one that takes flint-player's default
  features, or names a left-out package in `features`.
- `PlayerApp` holds the project's list to its run's registry at its first
  resume, as `PlayerArgs::prepare` does for the stock player
  (`PlayerApp::unbuilt_packages`), so a game's binary that lacks a listed
  package refuses to start, naming `flint build`.

## As built: a game's own packages (#1562)

- Decision 6 as recommended. A game's `packages/<name>/` (`tier = "game"`,
  `Packages::read_with_game` refuses another tier there) is read beside the
  engine's wherever a `[packages]` list is resolved, and its schemas follow
  the game's own schema directory (`project::game_packages_beside`). It is
  listed by name like a feature package; the table form stays refused.
- When a listed game package has `[rust]`, `flint build` writes
  `<engine>/launch/<id>/` (bins `flint-player` and `flint`: `stock()`, then
  each game package's `player::PACKAGE`, launched before anything asks; the
  CLI adds each `cli::extensions()`) and runs `cargo build -p
  flint-launch-<id>` in the engine, then puts `Cargo.lock` back byte for
  byte. `launch/*` is in `members` only, never `default-members`.
- A launcher naming a crate that is gone fails every cargo command in the
  engine, `cargo metadata --no-deps` included, so `flint build` removes its
  own such launcher before planning, and any launcher it wrote once the game
  lists no package with Rust.
- Measured on a scratch game (debug, this machine): the first build 55.8 s,
  with every engine crate below flint-package `Fresh`; after deleting the
  package, the stock pair again in 13.4 s.
- The stock binaries know a game package from its manifest only, so with one
  listed they refuse with `unbuilt-package` (`packages::unbuilt`: a game
  package with a player part the run's registry lacks). A game's own binary
  (#1532) still links only the engine's packages.

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
  whose commands live in flint-cli keeps them (music, rhythm; procgen's
  went with #1557, below). It goes when its package's commands move to its
  crate (#1555).
- **Package flags on a command the package doesn't own** (ADR 0136
  section 7: `gen --ai-*`, `--max-cost`, `--cost-report`, `--style-guide`,
  `render --ai-refine`, `render --sweep wooze.*|living.*`, `asset resolve
  --strategy ai_*`): decided here, built by #1572 (its own as-built section
  below says where the decision moved). Each flag changes how the
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
  Moved since: "As built: a package's flags on another's command" below.
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

## As built: procgen moves into its package (#1557)

- **Its crates are in its folder:** `packages/procgen/crates/flint-procgen`
  (moved whole, history kept) and a new `flint-procgen-cli`, both members
  by #1554's `packages/*/crates/*` glob. `package.toml` names them under
  `[rust]` in #1554's and #1555's form: `player = "flint-procgen"` (its
  `player` feature holds the player part, `player::PACKAGE`) and
  `cli = "flint-procgen-cli"` (its `cli::extensions()`). The launchers'
  build scripts list both from those lines (`package_list.rs`'s `moved()`,
  `stock_extensions.rs`), so `src/` names no procgen crate. The hand-written
  optional dependency and the `procgen` feature stay until #1568
  generates them.
- **The player's half is a registration point the kernel defines**,
  `flint_package::assets::AssetMaker`, called at five moments: the run's
  start (the disk tier), a scene's load before the model loader, spawned
  entities, each frame before `Stage::Procgen`, and a teardown. It also
  answers `pending(name)` for a model's collider. The kernel lends it a
  `MakeCtx`: the world, the `ModelLoadConfig`, a `Gpu`, the colliders as
  `&mut dyn MadeMeshes`, the resources, the registry and `[packages]`. A
  package also registers `run_resources`, which put its resolver in each
  core's `Resources`. Its own registration points use #1555's
  `insert_point` / `point_mut` / `point`. That is how living's props join
  procgen's load: a `LoadStep` pushed onto procgen's `LoadSteps`, in
  `[requires]` order. This record's section 4 listed that point as
  "procgen's resolve steps".
- **`PlayerExtension::register_generators` became `resources(&mut
  Resources)`.** It was the one hook that named a package's type, so it
  couldn't stay without a `cfg`. A game finds the registry with
  `flint_procgen::player::generators_mut`. Decision 7 keeps
  `PlayerExtension` as it is apart from this; no game in the three repos
  used the old hook.
- **The CLI's commands stay the kernel's**, so `--help` is unchanged in
  every build, and `flint gen x.glyphs.toml` stays the kernel's, as #1543
  decided. The kernel's `flint gen`, `flint edit`, `gen-preview` and
  `tex-edit` ask registration points on the command line,
  `flint_package::cli::Points`. That is one typed map for the run, filled
  before parsing by each `CliExtension::register` and then by the binary's
  own parts. Procgen's extension adds no command. It registers `SpecGen`
  (the run of a spec; `GenArgs` moved into `flint_package::cli::gen` so
  the package can take it), `flint_viewer::host::SpecEditors` (its two
  editors as `Box<dyn Editor>`) and `SpecChecks` (parse, params, a terrain
  spec's issues). Procgen's own `gen` declares two points of its own,
  `AiSteps` and `StripSheet`. The binary fills them from its ai and living
  code under those features. That is #1572's shape for `gen --ai-*`,
  built for `gen` alone. A missing point is `MissingPackage`, naming the
  package whose manifest provides the command's words. With procgen's two
  `absent::stub!`s gone (music's and rhythm's went with #1555), no
  command is stubbed, and the macro went too.
- **Living's tables are living's in the CLI too.** The `[living]` and
  `[wooze]` checks, the settle at load and the echo sweep check were
  behind `procgen` only because flint-procgen holds their types. They are
  behind `living` now. A procgen build without living no longer checks
  tables its player never reads.

## As built: the fourth move, rhythm (#1556)

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

## As built: living moves into its package (#1558)

- **Its crates are in its folder**: `packages/living/crates/flint-living`
  and `flint-wooze`, moved whole. flint-living's parts are features, as
  procgen's are: `player` (`player::PACKAGE`: the subsystem, its builder,
  the props as a run resource, its step of procgen's `LoadSteps`, its
  script modules, keys and policy name), `debug-hud` (the Living Props
  panel, moved from flint-debug-ui, and F8's pick) and `cli`
  (`cli::extensions()`, which adds no command). The manifest names the
  crate for both launchers under `[rust]`, and both build scripts list it.
  flint-debug-ui links no flint-procgen now, and flint-viewer neither
  flint-procgen nor flint-wooze.
- **`[with.<package>] features`** (decision 9) is a manifest table now:
  `flint_core::package::Package::with`, held to name another package by
  the cross-file check. Living's is `[with.debug-hud] features =
  ["flint-living/debug-hud"]`. flint-player's `debug-hud` feature turns it
  on by hand (`"flint-living?/debug-hud"`) until #1568 generates such lines
  from the table.
- **Decision 4's "scene settling"** is `Registry::settle_scene(fn(&mut
  SceneFile, &Path) -> Result<()>)`, not the `toml::Table` form the table
  sketched: living's preset can set `[post_process]`'s boil, which lives on
  the `SceneFile`, not in its tables. `flint_player::settle_scene` runs every
  package's. The player (`prepare`, a transition, a playtest, the web
  entry, `apply_scene_settings` on a copy) and `flint render`'s
  `load_settled` call it. The viewer can't depend on flint-player, so it
  asks its own point for the settle (next bullet but two).
- **The living builder reads its own tables.** `SceneSettings::{living,
  wooze}` went. `apply_scene_settings` keeps the settled tables in
  `scene.tables`, and the builder reads `[wooze]` and `[living]` from
  `SceneCtx::tables`. The props' defaults (`[living]`) are set in living's
  load step, which reads the scene's tables from procgen's `LoadCtx`. They
  come from the kernel's `MakeCtx::tables`, a new field, filled by the
  player and by `HeadlessGame::make_assets`, which takes the tables now.
- **`--compare` and `--split` are not yet a package's `player_args`.**
  They stay declared by `PlayerArgs`, so the help and #990's flag count
  don't change. Their values reach the package as `flint_package::LaunchFlags`
  (by long name, in the run's resources, `PlayerApp::launch_flags_mut`).
  Living's `LivingStart::of` reads them the first time a scene is built.
  `LaunchOptions::{compare, compare_at_start}` went. (#1585 made them
  living's `player_args`; see its section.)
- **A capture's living state**: `CaptureStart::wooze` and `HeadlessGame`'s
  living methods went. The render declares `flint_package::cli::render::StagePart`,
  and living's CLI part fills it (`flint_package::cli::Stages`). It puts the
  stage's wooze field in the capture's resources for the builder
  (`HeadlessGame::resources_mut`), lays a preset column's boil, blends the
  props before each capture, and answers `--report json`'s props. It reads
  its own subsystem through `HeadlessGame::capture_game()`, the scene's
  subsystems and resources as `flint_package::CaptureGame`, found with
  `flint_package::find`. The report's prop types (`PropReport` and the
  rest) are the Package API's, so the document has one shape. This is the
  "ticket e" seam for `render --sweep wooze.*|living.*`: the flags stay
  declared by `render` (the help is unchanged), and the sweep's cells are
  held and laid by the point.
- **`flint check`'s tables**: `flint_package::cli::TableChecks`
  (`refused`, and `check` over a `TableCheck`), so `[living]` / `[wooze]`'s
  refusals and the `[wooze]` / `[living.species]` checks are living's.
- **The viewer's field**: `flint_viewer::field_clocks::SceneFields` (the
  settle, the field, the clocks), a point living's CLI part registers. The
  viewer reads it from `flint_package::cli::points()` and depends on
  flint-package for it.
- **The Living Props panel goes in through debug-hud's point**:
  `flint_debug_ui::ScenePanels`, which the kernel inserts (behind
  `debug-hud`) and each package adds a `fn(&FlintWorld) -> Option<Box<dyn
  DebugPanel>>` to. The player makes them where it made living's panel, so
  the F3 order holds. The pose overlay is `DebugPanel::paint_world`, a
  default method, so the frame no longer downcasts to a panel type.
- **`gen --strip`** moved with living (`flint_living::cli::strip`, still
  procgen's `StripSheet`). The GIF writer it shares with `render` is
  `flint_package::cli::gif`, behind flint-package's `gif` feature, so the
  player compiles none of it.
- **What the kernel keeps of living, as it keeps music's**: the policy
  name `living` (so a state that names it registers in every build), and
  the "built without" line (`missing_features::unbuilt_living`, said only
  when the registry lacks the package). No `feature = "living"` is left in
  flint-player, flint-cli, flint-viewer or flint-debug-ui. Their tests that
  drive the living subsystem through a `PlayerApp` stay in flint-player
  (`player_app/living_tests/`, over a dev-dependency on flint-living), and
  pass over a launcher without the package.
- **Measured**: the frozen Woozy Grove and the three wooze and echo
  fixtures, rendered by a build from before the move and one after, are
  byte-identical: a still, `--time 2`, `--boil-time`, a list of moments, a
  wooze sweep, a preset sweep, an echoes sweep, `--scripts`, `--report json`,
  `check --json`, `validate`, and two player screenshots (one with
  `--compare ,calm --split`). Their logs match too, once clock times are
  taken out.

## As built: debug-hud moves into its package (#1559)

The last of the six moves. flint-player had 137 `feature = "debug-hud"`
lines on main (the epic counted 189 before living and rhythm took theirs);
now it has none.

- **Its crate is in its folder**: `packages/debug-hud/crates/flint-debug-ui`,
  moved whole. Its new `player` feature is the package's part,
  `flint_debug_ui::player::PACKAGE`, which the manifest names under `[rust]`
  and flint-player's build script lists. The panels and the widgets stay in
  every build of the crate, since flint-cli and flint-viewer draw the
  widgets and the viewer the Rendering & Effects panel. So the CLI and the
  viewer still link the package's crate, as they did; the player links it
  only with `debug-hud`, which `scripts/headless-check.sh` holds.
- **The panel vocabulary is the kernel's**: `flint_package::panels` holds
  `DebugPanel`, `PanelLayout`, `PanelCtx`, `PanelHost`, `DebugPanels`,
  `ScenePanel` and `ScenePanels`, moved from flint-debug-ui, which
  re-exports them at their old paths. Section 2 and the subsystem table had
  `DebugPanels` as debug-hud's type. It isn't, because two of the panels are
  the kernel's own: a game's (`ExtensionPanel`, ADR 0107) and the
  expression feed's (`expression_panel.rs`). Both stay in flint-player, and
  the panel the table said would move with debug-hud stays with the feed
  it reads, so no feed type is published as a resource. A package's panel
  (living's, rhythm's) needs no flint-debug-ui for the trait. The run has a
  `DebugPanels` only where it has a debug HUD, so `get_mut` is still `None`
  without the package, as section 2 says.
- **`DebugPanel::outlives_scene`** (default `false`) says which panels a
  transition keeps: F4's and a game's. It replaces the name compare and
  the downcast to the player's `ExtensionPanel`, which the package can't
  name.
- **The seam is `flint_package::hud::DebugHud`**, made once a run from
  `Registry::debug_hud(MakeHud)` (`Registry::hud()`; one a run, and a
  second warns). `PlayerApp::new` makes it and puts the run's `DebugPanels`
  in at the same time, so a capture, which builds no `PlayerApp`, has
  neither. Its hooks, and what each is lent, field by field:

  | Hook | When | Lent |
  |---|---|---|
  | `scene_panels(panels, HudScene)` | `enter_scene`, after the renderer's share | world, scene path, `GrassPanel` (the grass `bind_scene` bound: `GrassConfig`, entity, component), the authored styles, the `AssetScope`, the looks and the one in use, the scene file Commit writes, the schemas, and the packages' scene panels, made by the kernel from `ScenePanels` |
  | `look_changed(panels, authored, current)` | `fade_to_look` | the authored styles under the look, its name |
  | `sync(panels, PanelCtx)` | `render`, before `prepare` | the same `PanelCtx` as before; the kernel reads `PanelHost` back (freeze, look, particles) and the HUD handles a committed style itself |
  | `toggle(panels, Toggle)` | F3, F4 | which; `Some(open)` or `None` |
  | `world_lines(panels, WorldView, out)` | after the scripts' lines are drained | world, clock time, camera eye and forward |
  | `ui(HudUi)` | the HUD pass, after the subsystems' `ui` | egui, the panels, the view-projection, a drawer that draws a game's panel through its extension |
  | `cards(ctx, Cards)` | the HUD pass, last | `ScriptErrors`, the `StyleRegistry`, the inset |
  | `graph(ui, GraphView)` | inside F2's overlay | the frames and counters, the layout |

  flint-package gained flint-particles (the `PanelCtx` it already lent) and
  flint-terrain (`GrassConfig`). Both are kernel crates below it.
- **The issue's risk, the hitch and frame-history panels**: the history
  and the hitch detector stay the player's, and nothing of them is lent.
  F2's graph is still the player's: `stats_view::StatsGraph` turns the
  history into frames and counters as before. Those are
  `flint_package::hud::{GraphFrame, Band, HitchCounters, GraphLayout}` now,
  moved from the widget (flint-debug-ui re-exports them), and the HUD only
  draws them, keeping the widget's ceiling and mesh in its own state. No
  field had to become a kernel ticket.
- **F3 and F4 stay the player's keys**, offered only when the registry has
  a HUD (`debug_keys::player_keys()`: F2's text names the graph too). What
  they toggle is the HUD's (`Toggle::Scene` is every panel but F4's).
  `--list-keys` prints the same rows in the same order as before.
- **`ScenePanels` is inserted by the kernel** (`packages::ENGINE`), so living
  fills it in any order of registration, and the kernel makes the panels
  only where there is a HUD.
- **One change in behaviour**: closing F4 now catches the cursor only
  when no other panel is open (the rule F3 and a subsystem's keys follow).
  Before, it caught it even with F3's panels open.

## As built: the gate in CI (#1560)

Decision 5 was taken as recommended, with the gate restated: a package is
added by creating `packages/<name>/` and running `scripts/package-sync.py`.
CI holds both halves.

- **`scripts/package-sync.py` writes the generated blocks** of section 5,
  one in each launcher's `Cargo.toml`, between `# package-sync:features`
  (or `:dependencies`) and `# package-sync:end`. The blocks hold
  `packages`, every package feature the crate has, which both crates'
  `default` turns on. It is the `stock` of section 5, under the name
  flint-cli already had, so `--features packages` and `flint build`'s
  options read as before. They also hold, for a package whose feature
  isn't written by hand, its feature (flint-player: `dep:<crate>`,
  `<crate>?/player`; flint-cli: `flint-player/<name>` when it has a player
  part, `dep:<crate>`, `<crate>?/cli`; each after its `[requires]`' features)
  and an optional `path` dependency on `packages/<name>/crates/<crate>`.
  Not every package is generated. The six moved packages keep their
  hand-written features, which carry what no manifest states yet: device
  features (`flint-music?/device`), `[with.debug-hud]` lines on another
  package's feature, the ai flags' extra crates (#1572), and the CLI's
  debug-hud in every build (#1583). The script lists them in `packages`
  and refuses to generate a package that would need such a line, naming
  the hand edit. Moving the six into the block is #1568's remaining work.
  `--check` is in `scripts/ci.sh` and names the command.
- **No package name in the launchers' source** is
  `scripts/package-names-check.py`, in `scripts/ci.sh` (so CI's rustfmt job
  runs it). A package name means two things in code. One is a
  `feature = "<f>"` of any manifest's `[build] features`, anywhere in
  `crates/flint-player/src` or `crates/flint-cli/src`, test code included.
  The other is a path through a package's crate outside test code. Tests
  keep their dev-dependencies (#1558, #1559). A string such as
  `registry.has("living")` builds without the package, so it is not a
  name in this sense. The CLI's two leftovers are recorded exceptions,
  each failing once it matches nothing: the ai flags (#1572) and the
  debug-hud widgets (#1583). flint-player has none.
- **The toy package** (`tests/fixtures/packages/toy`, with a game in
  `toy-game/`) is shaped like `vehicle`. It has a component (`toy_spring`),
  a `fixed_update` that pulls the body before Rapier's step, a script
  module (`toy_steps()`, `toy_springs()`) and a command
  (`flint toy springs`). `scripts/package-toy.py`, in CI's `package
  done-when` job, copies it in, syncs, and holds every change outside the
  folder to the generated blocks and `Cargo.lock`. It then builds the stock
  binaries, asks the built flint (`package list --built`, `check`,
  `playtest`: 120 steps, bob swinging to x = -1.000 m, then `toy springs`),
  deletes the folder, syncs, builds again, and holds both `Cargo.toml`
  files and the lock to their bytes before. A toy calling a Package API
  method that isn't there failed the build. A toy with a `[with.*]` part
  failed the sync.

## As built: the widgets are the kernel's (#1583)

#1559 left flint-cli and flint-viewer linking flint-debug-ui, the debug-hud
package's crate, for two things: the egui widgets their windows draw, and
the viewer's Rendering & Effects panel (F4) with its style editor and
style cards. So `debug-hud = false` still put the package in a game's
`flint`, and the CLI turned on the player's `debug-hud` in its dependency
line. That is the last of the CLI debug-hud leftovers in the sections
above.

- **A kernel crate, `crates/flint-widgets`**, holds what both the tools and
  the HUD draw: the widgets (`section`, `mixed_slider`, the drag helpers,
  the curve and gradient editors, `FrameGraph`), `run_headless_frame`, and
  the renderer's controls: `RenderDebugPanel`, `StylesEditor`,
  `paint_style_errors` and `paint_style_drops`. The engine manifest lists
  it with the kernel. It depends on flint-package (the panel trait the
  panel implements, the graph's data), flint-render, flint-scene and
  flint-schema, nothing of a package. flint-debug-ui depends on it and
  re-exports it at the old paths (`pub use flint_widgets as widgets`, and
  each panel type at the crate root), so living's and rhythm's panels and
  every test built unchanged.
- **The viewer's F4 is the viewer's own, not a debug-hud part.** The issue
  asked which it is. The panel is the renderer's controls, which the
  engine manifest groups with the render features still inside
  flint-render, and the viewer keeps its post settings in the panel itself
  (`render_panel.pp` is what DoF-follow reads and writes), so a viewer
  without it would need a second home for that state. A viewer part
  behind a feature would also leave `flint edit` without its render
  controls in any game that drops the HUD, when what a game drops is the
  in-game overlay. The player's F4 is the same panel, which the debug HUD
  shows; with debug-hud out the player shows none, as before.
- **flint-cli's `debug-hud` is a hand-written package feature,
  `["flint-player/debug-hud"]`**, in `packages` and so default-on. The
  dependency line turns on only `frame-history`. `package-sync.py` lost
  `CLI_HAS_IN_EVERY_BUILD`, `package-names-check.py` its #1583 exception,
  and `absent::ALWAYS_ON` and the owners test their debug-hud rows.
  `flint build` for a project with `debug-hud = false` is one cargo run
  now: nothing the CLI forces on the player is a package's, so the split
  stays only as the general rule.
- **Held by `scripts/headless-check.sh`**: flint-debug-ui is in
  `CLI_OPTIONAL`, and the no-feature viewer is held to no flint-debug-ui
  beside flint-wooze and flint-procgen. `cargo tree -p flint-cli
  --no-default-features -e normal` has no flint-debug-ui.

## As built: the kernel names no package (#1585)

#1560's check held the player to no package name in the build's sense. It
still named living and music as strings, which build without them. Those
went, as decisions 4 and 8 have it:

- **The "built without" line is the manifest's** (decision 8).
  `missing_features::unbuilt` is one kernel builder, registered by
  `ENGINE`. For each package flint-player has a part of that the run's
  registry lacks, it counts the package's `[provides]` components among
  the scene's entities and its scene tables among the scene's tables. A
  scene that uses any of them hears `[<p>] this player is built without the
  `<p>` package: the scene's <parts> <does>` once. `<does>` is a new
  manifest table, `[without] does` (living: `stand still`; music: `plays
  silent, and music_beat() reads -1, as with no music`). `build.rs` writes
  the table (`packages::WITHOUT`) from the manifests, as it writes
  `PLAYER_PACKAGES`, so it works in the browser too. `unbuilt_living`,
  `unbuilt_music`, `living_uses` and `music_line` went. "Woozy grass"
  dropped out of the line: `grass.wooze` is a field of the kernel's
  `terrain` and `grass_ground`, not something the package provides, and
  woozy grass needs a `[wooze]` table, which the line names.
- **Policy names are the manifest's too.** `[without] policies =
  ["living"]`. `ENGINE` registers each one, so a game state naming `living`
  registers in every build (#727). Living still registers its own when
  it is there, and a name is registered once.
- **`--compare` and `--split` are living's `player_args`** (section 4's
  row). `Registry::player_args::<A: clap::Args + Clone + Send + Sync>()`
  records `A::augment_args` and a reader. `Registry::augment_player_args`
  adds every package's group to a command, and `read_player_args` reads
  them back into a `flint_package::PackageFlags`. `flint-player`
  (`PlayerCli::parse_launched`) and `flint play` (flint-cli's
  `with_play_flags` on the `play` subcommand) parse with them, and
  `prepare` puts each group in the run's resources by its own type. Living's
  group is `flint_living::player::CompareArgs` (under a "Living props" help
  heading, with clap checking `A,B` and `--split`'s `requires`), and
  `LivingStart::of` reads it. `flint_package::LaunchFlags` and
  `PlayerApp::launch_flags_mut` went. A build without living has no
  `--compare`: clap refuses it as any unknown flag, where the player used
  to warn and play on. `PlayerArgs` lost two flags (#990's count).
- **`scripts/package-names-check.py` holds strings too**: in
  `crates/flint-player/src`, outside test code, a string literal that is a
  package's name fails (rule 3). One exception is recorded by file: the
  `music` log target in `logging.rs`, which the music package logs under.
  The player's two "procgen" debug labels for an asset maker's collider
  mesh now read "made mesh". The CLI isn't held to it: its commands and
  help name packages as words a user types.

## As built: a package's flags on another's command (#1572)

- **A flag slot** (`flint_package::cli::flags`). A command that takes a
  package's flags declares where they go: a `PackageFlags<P>` field in its
  derived `Args`, `P` naming the command's words. Its `augment_args` adds
  each registered `Flags` group of that place (a derived `clap::Args` of
  the package's, through its `augment_args`), and its value is the
  command's `ArgMatches`, from which the package reads its own group
  (`flags::parsed`). The slot sits where the group's help heading always
  sat, so `--help` reads as it did: clap orders headings by their first
  argument, and a group added after the derive would have moved "Refine a
  look" below "Debug, lists, GPU and schemas". `render` is the one slot
  (`cli::render::RenderFlags`, the field `packages` in place of `refine`).
- **`CliExtension::flags()`** returns the groups (default none).
  `extension::Added` checks each group's command is in the tree and holds
  them; `Added::parser` builds the engine's tree with them in its slots,
  and `run_with` sets them for the run (`flags::set_run_flags`) so every
  later parse, a `--variant`'s words included, has them. A test builds
  with another list through `flags::with_flags`, a thread's scope.
- **A flag the owner declares and a package serves** stays declared by the
  owner, since its help heading is the owner's and its value runs at a
  point: `gen --ai-*`, `--cost-report`, `--style-guide` (procgen's
  `GenArgs`, served through procgen's `AiPoint`) and `asset resolve
  --strategy ai_generate|human_task|ai_then_human` (the kernel's, served
  through `cli::asset::BatchResolve`). The package says so with
  `Points::serve(line)` in its `register`, and the owner refuses a line
  nobody serves before anything else runs (`gen_args::refuse_unserved`,
  `asset::batch_of`).
- **`[provides] flags`** (`flint_core::package::Kind::Flag`, `FlagLine`):
  each flag as a line spells it, the command's words, the flag and at most
  one value (`render --ai-refine`, `asset resolve --strategy ai_generate`).
  It is how a binary without the package names it. clap's unknown argument
  on a command whose words and flag a manifest lists is
  `absent::missing_flag`'s `MissingPackage`; a served line nobody serves is
  `absent::missing_served(line)`. Both exit 2, `package` in the `--json`
  error, from the manifests `build.rs` compiles in. The ai crate's test
  holds its list to what its extensions add and serve; the completeness
  test holds every line to a flag of the stock tree.
- **The points the moved code asks**: `cli::look` (`LookRefines`, a
  `LookRefine` that reads its flags and runs; `RefineHost`, what the render
  lends: the look, the reference, a draw of a candidate block scored
  against it, `--sweep`'s check of an edit and its tune sweeps, the
  measures and the sheet) with the report types it logs (`DistanceReport`,
  `ImageMeasures`, `Metric`, moved from flint-cli's `look_measure`, which
  re-exports them, so the log is byte for byte as before);
  `cli::gen::ModelStill` (the kernel's model still, which the ai package's
  `gen --ai-refine` previewer draws with; flint-cli registers it in every
  build); `cli::asset::BatchResolve` (the scene's missing references,
  handed over as `MissingAsset`s).
- **Where the code went**: `gen_ai`, `gen_refine` and `look_refine` are
  `flint_asset_gen::cli`, with `resolve` (the batch half of `asset
  resolve`). The package's second extension (`Steps`) adds no command:
  its `register` fills procgen's `AiPoint`, `LookRefines` and
  `BatchResolve` and serves the gen and resolve lines, and its flags are
  `RefineArgs` on `render`. `RefineHost`'s implementation is
  `render_still.rs`'s `LookHost`, whose tests run the package's loop over
  it (the ai crate is a dev-dependency of flint-cli, as living's is).
- **What this leaves**: `git grep 'feature = "ai"' crates/flint-cli/src`
  finds nothing, and `scripts/package-names-check.py` lost its five #1572
  exceptions (#1583 had taken its own; the one left is #1585's `music`
  log target, a string). flint-cli's `ai` feature is
  `["procgen", "dep:flint-asset-gen", "flint-asset-gen?/cli"]`: it names
  no procgen-ai or ai-core crate. `gen`'s ai flags are still declared in
  `GenArgs`, interleaved with procgen's under its headings; moving them
  into the package's group would need slots between procgen's headings
  (#1592).
- Measured (debug, Windows): every `--help` page of the stock `flint`, 78
  of them, is byte-identical to main's. A mock `--ai-refine` (three rounds
  of three, `stylize_props` at 320x180, still and `--time 0.5`) writes the
  same sheet, refined look and JSON log as main's, paths aside; `gen
  --ai-create --ai-provider mock` the same GLB and spec. A build without
  ai has 133 lines of `render --help` against 149, and each of the
  package's flags exits 2 naming it.

## As built: the CLI names no package (#1595)

- **The rule holds both launchers.** `package-names-check.py`'s
  `STRING_LAUNCHERS` is `flint-player` and `flint-cli`. Holding the CLI's
  strings found five literals and no help text (a word inside a longer
  string is never a hit), each the kernel knowing a package: the owner
  `render --sweep wooze.*|living.*` was refused for, the `[living]` table a
  sweep cell wrote, the refine host's list of a scene's keys and the
  used-package scan's `procgen`.
- **A flag line may name values by a prefix.** `[provides] flags`' value
  `wooze.*` names every value past `wooze.` (`FlagLine::names_value`).
  Living lists `render --sweep wooze.*` and `render --sweep living.*`;
  `render` declares `--sweep` and parses every family's keys as before, and
  a render with no `StagePart` refuses a key a package draws
  (`SweepGrid::package_key`) through `absent::missing_value`, which names
  the package whose line's value names the key. The completeness test
  holds the line to `render`'s `--sweep`, as for any flag.
- **The cell's tables are the part's to write.** `StagePart::lay_tables`
  takes the cell's own copy of the scene's top-level tables and each table
  axis's key and value as the line wrote them (`SweepGrid::table_axes`:
  `("living.echoes.spread", "0.9")`); living's walks the key's path and
  types the value as a scene would (`count` an integer). The kernel's
  sweep keeps the keys' grammar (what `--sweep` takes is its help) and
  hands the rest over. The refine host asks `sweep::Family::of(key)` which
  keys a `[post_process]` block holds, the one place the prefixes are read.
- **A new `[provides]` kind, `specs`** (`Kind::Spec`): patterns of the spec
  files a package makes assets from, `*` then the ending. Procgen lists
  `*.procgen.toml`; `flint package used` credits a scene naming one (a
  path, or the name a spec states) to `Packages::spec_owner(file)`, and
  `seen_by_scan` counts a package with specs as one the scan can see used.
  A kind and not a `[without]`-style table, because a spec file is an item
  a scene uses, which is what `[provides]` lists. The completeness test
  holds each pattern to a file of the tree (the frozen scenes' specs).
- Measured: `flint package used` on the frozen Woozy Grove gives the same
  text and JSON as main's (procgen used by `grove_oak`, the same left-out
  line on a scene with no spec); a sweep `living.echoes.spread=0,0.9,1.8`
  draws the same sheet byte for byte.

## As built: every package's lines are generated (#1568)

#1560 left the six moved packages' features and dependencies written by
hand in the two launchers, because three things they carry were in no
manifest. Each is a manifest line now, and `scripts/package-sync.py`
writes every package's lines. Neither `Cargo.toml` holds a package's
feature outside its generated block, and the script refuses one that does.

- **Device features are `[rust.devices]`**, a table in `[rust]`: for each
  of the launchers' device features, the package's crates' features it
  turns on, as `crate/feature` (music: `audio = ["flint-music/device"]`;
  rhythm: `audio` and `gamepad`; procgen: `dialogs =
  ["flint-procgen-cli/dialogs"]`). `flint_core::package::Package::devices`
  holds each crate to `[provides] crates`. Cargo can't give one feature two
  lines, and a device feature such as `audio` also turns on kernel crates,
  so it stays hand-written, with its comment, and turns on a generated
  `packages-audio` (`packages-gamepad`, `packages-dialogs`) that holds the
  packages' share as weak lines (`flint-music?/device`). A device reaches a
  package's crate only where the package is built, as before. The script
  refuses a device name neither launcher has and a device feature that
  doesn't turn on its `packages-<device>`.
- **`[with.<other>] features` are generated** onto the other package's
  feature, weakly (`flint-living?/debug-hud` on `debug-hud`), so the part is
  on when the game lists both. Rhythm's Music Guide panels were a
  `flint-rhythm-game?/debug-hud` line with no manifest behind it, so
  rhythm's manifest has a `[with.debug-hud]` now too.
- **The ai flags' crates and the CLI's debug-hud needed nothing.** #1572
  made flint-asset-gen's `cli` part bring flint-procgen-ai and
  flint-ai-core, and #1583 made the CLI's `debug-hud` the plain forward
  (`["flint-player/debug-hud"]`), so both were already the generated rule.
  flint-cli's `procgen` turned on an optional flint-procgen that its source
  doesn't use (the player's `procgen` brings it), so that dependency went.
  flint-procgen-cli gained an empty `cli` feature, so every package's
  commands are turned on as `<crate>?/cli`.
- **A line flint-player already turns on isn't written again in
  flint-cli.** The CLI's package and device features forward to the
  player's, which link the same crate (one package in the graph), so the
  CLI's `packages-audio` is empty and its `packages-dialogs` holds only
  flint-procgen-cli's, a crate the player doesn't link.
- **A dependency line is `{ workspace = true, optional = true }`** when the
  root's `[workspace.dependencies]` declares the crate, so its
  `default-features = false` (flint-music's `default` is `device`) still
  decides, and the `path` form otherwise (a new package's crate).
- **Measured**: `cargo tree -e normal -f '{p} [{f}]'` gives the same crates
  with the same features as main's hand-written lines in thirteen
  configurations (both binaries by default, the CLI with no features, and
  device and package mixes such as `audio,music`, `gamepad,rhythm,debug-hud`
  and `dialogs,procgen`), apart from the new names, `packages-*` and
  flint-procgen-cli's `cli`, which turn on nothing else.
  `scripts/package-check.sh` and `scripts/headless-check.sh` pass
  unchanged.
- **`package-sync.py --self-test`** (in `scripts/ci.sh`) runs the generator
  over a scratch copy of the tree with a made-up package that has a device
  line, a CLI-only device and a `[with.debug-hud]` part, and holds each
  refusal. Before this, a toy with a `[with]` part failed the sync.

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
