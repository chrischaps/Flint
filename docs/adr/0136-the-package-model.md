# ADR 0136: The package model: four tiers, `package.toml`, `[packages]`, two versions and the subtree

Status: proposed, 2026-11-24 (#1329). Accepted when Chris accepts it; the
"Decisions needed" at the end are his. Epic #1328 (Modularity, Phase 1).
Builds on [ADR 0114](0114-project-manifest-and-one-resolver.md) (the
manifest and one resolver), [ADR 0117](0117-player-subsystems-and-one-game-step.md)
(subsystems and their features),
[ADR 0122](0122-what-a-games-engine-carries.md) (what `engine/` carries) and
[ADR 0133](0133-a-game-binary-adds-cli-commands.md) (a game's own `flint`
commands). From the Flint Modularity Plan, 2026-11-19.

## Context

Flint is one unit. Its 33 crates share one version (0.1.0), and every game
builds the stock `flint-player` and `flint` from its `engine/` subtree with
default features: Float, FlintKart and Kandinski each run `cargo build
--release --bin flint-player --bin flint` in `engine/`. Every game links the
rhythm layer, music, living props and the wooze field, and every game's
`flint schema` lists `living_prop`, `shallow_wooze` and `music_session`.
None of the three games uses any of them (a survey of their scenes on
2026-11-24: 201 scenes, no `living_prop`, `[living]`, `[wooze]`,
`shallow_wooze` or `music_session`).

The audit epics (#673, #675) built most of the seams a modular engine needs:

- `flint-player` features `music`, `rhythm`, `procgen`, `living`,
  `debug-hud` (plus the build options `audio`, `gamepad`, `frame-history`,
  `profile`, `trace`), each gating a subsystem (ADR 0117) and the script
  modules it owns (#698).
- `flint-living` is its own crate (#1024); the wooze field is `flint-wooze`.
- Schema directories layer (ADR 0114); a game adds CLI commands through
  `CliExtension` (ADR 0133) and player hooks through `PlayerExtension`.

Nothing lets a game use them. `flint.toml` has no key that selects features,
no command builds a matching player, `flint-cli` links `flint-rhythm`,
`flint-music`, `flint-input-capture`, `flint-asset-gen`, `flint-procgen-ai`
and `flint-ai-core` unconditionally, and `schemas/` is one flat folder.
Phase 1 of the plan makes leaving a package out real. Each of its tickets
(#1330-#1338) needs one text that says what a package is, what a game's list
means and how versions will work. This is that text. It changes no code.

Rust links statically. Unity compiles a package's C# when it is imported;
in Flint, adding a package means building a new player. So the build is
part of resolving packages, and on Windows a build has to replace a binary
the OS may be running.

## Decision

### 1. A package is everything one feature ships

A package is a name a game lists, and everything that name brings: its
crates, components, archetypes, scene tables, script modules, CLI commands,
debug panels, generators, styles, docs, examples and goldens. A manifest,
`package.toml`, says what it owns. Every item in the engine has exactly one
owning package (table in section 7); a test holds that from #1331 on.

### 2. Cargo features first, a composed binary second

Phase 1 selects packages with Cargo features on `flint-player` and
`flint-cli`, one feature per package with the package's name (`living`,
`music`, `rhythm`, `procgen`, `debug-hud`, `ai`). That works inside the
subtree, needs no new public Rust API and can be undone. From Phase 2 a small
launcher crate lists the packages and hands each to the kernel through a
public `Package` API, and `flint-player` names no package. Features cannot
carry per-package versions or a package from outside the engine repo; a
launcher can.

### 3. Four tiers

| Tier | Rule | Listing |
|---|---|---|
| **kernel** | Every game links it. Kept small | Never listed; always there |
| **standard** | On unless a game drops it | `name = false` drops it |
| **feature** | Off unless a game lists it | `name = "*"` (a version requirement) adds it |
| **game** | Lives in the game's repo, in the same package form | Listed like a feature package (Phase 2) |

A package's requirements come with it: `living` brings `procgen`, `rhythm`
brings `music`. `flint build` and `flint package list` say what came in
that way. A requirement the game has dropped by name (`procgen = false`
beside `living = "*"`) is an error naming both.

Some Cargo features are **build options**, not packages: `audio` (the sound
card, not the audio package), `gamepad` (gilrs, the pad backend of
`flint-runtime`'s input), `dialogs` (file dialogs), `frame-history`,
`profile` and `trace`. They own no component, scene table, script module or
command, so a game does not list them. `flint build` keeps their defaults,
and #1335 decides whether it exposes them as flags. (The plan put `gamepad`
in the standard tier; it has nothing to own, so it is a build option.)

### 4. One repo, per-package versions

Packages stay in this repo. Phase 1 puts each manifest at
`packages/<name>/package.toml` while the crates stay under `crates/`; Phase 2
moves each package's crate, schemas, docs, examples and tests beside its
manifest. A package moves to its own repo only when someone else owns it or
it serves one game.

### 5. Two version numbers

- **The engine API version** is the kernel's (`api` in its manifest),
  starting at `0.1.0`. It is what packages build against.
- **Each package has its own semver** and states the engine range it builds
  against (`engine = "^0.1"`).

Until Phase 5 every package's version is the workspace version, so the
numbers move together and a requirement is checked only for syntax.
`flint build` refuses a combination the ranges do not allow once they can
differ. Everything stays below 1.0 until Phase 5.

### 6. The subtree stays until Phase 5

A subtree pins the whole engine at one commit, so per-package versions mean
nothing while it is the only way in. Phases 1 to 4 change nothing about how a
game carries the engine (ADR 0122). Phase 5 adds a second source (a tagged
git dependency or a registry) beside the subtree.

### 7. Every crate, component, scene table, script module and command has a tier and a package

The tree on 2026-11-24 (`main` at `56af89d`): 33 crates, 46 components and 5
archetypes in `schemas/`, 7 scene tables (the ticket counted 6; `[field]`
arrived with #1014), 44 script modules, 26 top-level commands. "Optional
from" is the phase in which a game can first leave the package out: the
standard packages other than `procgen` and `debug-hud` have no Cargo feature
until Phase 3, and the render features live inside `flint-render` until
Phase 4. Until then the kernel carries them.

| Package | Tier | Optional from | Crates | Components, archetypes | Scene tables | Script modules | Commands |
|---|---|---|---|---|---|---|---|
| kernel | kernel | never | `flint-core`, `flint-ecs`, `flint-schema`, `flint-scene`, `flint-query`, `flint-asset`, `flint-import`, `flint-pixels`, `flint-geometry`, `flint-runtime`, `flint-render` (its core; four packages' passes leave it in Phase 4), `flint-script` (its core; Phase 3 moves the others' modules out), `flint-constraint`, `flint-player` (the shell), `flint-android`, `flint-viewer`, `flint-cli` | `transform`, `visibility`, `bounds`, `model`, `material`, `material_overrides`, `light`, `sky`, `lod`, `asset_def`, `script`, `interactable`, `camera_tuning`, `screen_anchor`, `ui_fill`, `ui_text`, `sprite`, `sprite_animator`, `parallax`, `spline`, `spline_mesh`, `motion_hold`; archetypes `sprite`, `animated_sprite`, `character` | `[camera]`, `[environment]`, `[post_process]`, `[field]` | `entity`, `event`, `input`, `log`, `math`, `time`, `scene`, `state`, `data`, `persistence`, `camera`, `camera_2d`, `camera_rig`, `draw`, `draw_world`, `ui`, `screen_ui`, `touch`, `sprite`, `view`, `post_process`, `quality`, `test`, `expression`, `chunk`, `spline`, `field` | `init`, `entity`, `scene`, `query`, `schema`, `script`, `edit`, `validate`, `check`, `frame`, `profile`, `examples`, `env`, `bundle`, `export`, `goldens`, `prefab`, `asset` (`import`, `list`, `info`, `resolve`, `where`), `play`, `playtest`, `capture`, `render`, `dxc`, `style`, `help`; from #1331 `package` |
| physics | standard | Phase 3 | `flint-physics` | `collider`, `rigidbody`, `character_controller`, `joint`; archetype `player` | | `physics`, `physics_2d`, `nav` | |
| audio | standard | Phase 3 | `flint-audio`, `flint-audio-worklet` | `audio_source`, `audio_trigger` | | `audio` | |
| animation | standard | Phase 3 | `flint-animation` | `animator`, `skeleton`, `ik_two_bone`, `bone_probe` | | `animation`, `tween` | |
| particles | standard | Phase 3 | `flint-particles` | `particle_emitter`, `particle_effect` | | `particle` | |
| terrain | standard | Phase 3 | `flint-terrain` | `terrain`; archetype `terrain` | | `terrain` | |
| procgen | standard | Phase 1 | `flint-procgen` | (none: a `model.asset` that names a `*.procgen.toml`) | | | `gen`; `edit` on a `.procgen.toml` |
| debug-hud | standard | Phase 1 | `flint-debug-ui` | | | | |
| living | feature | Phase 1 | `flint-living`, `flint-wooze` | `living_prop`, `shallow_wooze` | `[living]`, `[wooze]` | `living`, `wooze`, `debug` (also needs `debug-hud`) | `render --sweep wooze.*` and `living.*` keys |
| music | feature | Phase 1 | `flint-music` | `music_session` | | `music` (until Phase 3 declared in `flint-script`, answering `-1` without the package) | `music validate`, `music play`, `music render` |
| rhythm | feature | Phase 1 | `flint-rhythm`, `flint-input-capture` | none: it judges a music_session's chart | | `conducted` | `music calibrate`, `music chart` (`play`, `replay`) |
| ai | feature | Phase 1 | `flint-asset-gen`, `flint-procgen-ai`, `flint-ai-core` | | | | `asset generate`, `asset regenerate`, `asset validate`, `asset manifest`, `asset costs`, `asset job`; `gen --ai-*`, `--max-cost`, `--cost-report`; `render --ai-refine` |
| ocean | feature | Phase 4 | none of its own until Phase 4: its passes are in flint-render, its spectrum in flint-geometry | `ocean`, `ocean_contact`, `water_body` | | `ocean` | |
| grass | feature | Phase 4 | none of its own until Phase 4: its passes are in flint-render | `grass_ground` | | | |
| stylize | feature | Phase 4 | none of its own until Phase 4: flint-render's stylize module | `ascii_style`, `pencil_style`, `watercolor_style`, `style_mask` | | `style`, `style_field` | |
| panels | feature | Phase 4 | none of its own until Phase 4: its pass is in flint-render | | `[panels]` | `panels` | |
| examples | game | (not the engine's) | | `schemas/examples/`: `door`, `film`, `hologram_style`, `orbit`, `story`, `tour`; archetypes `ceiling`, `door`, `floor`, `furniture`, `pillar`, `room`, `wall`; constraints `basics` | | | |

`tools/arch-analyzer` is repo tooling that no game links, so it belongs to no
package. The `examples` row is the examples' project (ADR 0131), which lists
the engine's packages like any game.

### 8. `package.toml`

Tools read it without compiling anything, so `flint check`, `flint package
list` and agents stay fast. `flint_core::package` parses it through the vfs
(#1331), and a mistake is a diagnostic naming the file and the key, as
`flint.toml`'s are.

```toml
[package]
name = "living"            # required: [a-z][a-z0-9-]*, unique; the key a game lists
version = "0.1.0"          # required: semver; the workspace version until Phase 5
tier = "feature"           # required: kernel | standard | feature | game
summary = "Props that hold and glide, and the wooze field"   # required, one line
engine = "^0.1"            # required except on the kernel: the engine API range
# api = "0.1.0"            # the kernel's manifest only: the engine API version

[requires]                 # other packages it needs: name = version requirement
procgen = "*"

[build]                    # how flint build turns it on (Phase 1)
features = ["living"]      # features of flint-player and flint-cli, same names

[provides]                 # what it owns; every item has exactly one owner
crates = ["flint-living", "flint-wooze"]
components = ["living_prop", "shallow_wooze"]
archetypes = []
scene_tables = ["living", "wooze"]
script_modules = ["living", "wooze", "debug"]
commands = []              # paths below `flint`: "music chart", "asset generate"
```

- A package's schemas are the `schemas/` folder beside its manifest, laid
  out as the engine's are (`components/`, `archetypes/`, `scene/`). #1332
  moves them there.
- An unknown key is an error naming the keys there are. `[provides]` lists
  only what the package itself declares; a `[provides]` entry another
  manifest also lists is an error naming both.
- Phase 1 has seven manifests: the six optional packages and `engine`, which
  holds the kernel and the five standard packages that have no feature yet,
  undivided until Phase 3 splits them out.

### 9. `[packages]` in `flint.toml`

A top-level table beside `[project]`:

```toml
[packages]
living = "*"               # a feature package, any version
debug-hud = false          # drop a standard package
```

- **No `[packages]` key: every package**, kernel, standard and feature, as
  today. This is the compatibility rule: a project that never writes the key
  keeps working, and so does a game that writes it and then pulls an older
  engine, since `flint.toml`'s reader leaves unknown top-level tables alone.
- **The key present:** the kernel, every standard package not set to
  `false`, every feature package listed, and what those require. An empty
  table means kernel and standard only.
- **A value** is a version requirement string (`"*"`, `"^0.1"`), or `false`
  for a standard package. Phase 1 checks a requirement's syntax and stores
  it. A table (`{ version, path }`) is reserved for game packages (Phase 2)
  and package sources (Phase 5), and refused until then with the phase that
  brings it.
- An unknown name is an error with the nearest real name. `false` on a
  feature package, or on a standard package that cannot be dropped yet (one
  with "Phase 3" in the table above), is an error that says so.
- The list decides what loads: the listed packages' schemas and script
  modules, what `flint check` accepts, and the features `flint build` builds.
  A component, scene table or script function from an unlisted package is an
  error naming the package and the line to add (#1334).

### 10. `flint build` replaces a running binary by renaming it aside

`flint build` turns the list into `cargo build -p flint-player -p flint-cli
--no-default-features --features ...` in the engine workspace (#1335). The
binary it writes may be running: the `flint.exe` doing the build, a
`flint-player.exe` the game is playing in another window, or an editor.
Windows refuses to overwrite or delete a running image, and Cargo stops:

```text
error: failed to remove file `...\target\debug\lockt.exe`
Caused by:
  Access is denied. (os error 5)
```

Windows does let a running image be renamed in its own folder. So before it
runs Cargo, `flint build` renames each output binary that exists to
`<name>.old.exe` beside it. The running processes carry on from the renamed
file and Cargo writes the new one. If Cargo fails, `flint build` renames
them back, so a failed build leaves the old binaries in place. The renamed
file can't be deleted while it runs, so `flint` and `flint-player` delete
any `*.old.exe` beside themselves when they start, ignoring a refusal.
Off Windows nothing is renamed: replacing a running file is allowed.

## Alternatives considered

- **Dynamic libraries** (hot-loaded packages, as `cdylib`s). They fail on
  the web and Android targets, which link one static image, and Rust has no
  stable ABI, so a package would have to be built by the same compiler as
  the player that loads it, with no check that it was.
- **A general ECS scheduler with plug-in systems.** Rejected in ADR 0117
  for reasons that still hold: about ten stages in a fixed, reasoned order,
  and components that are dynamic TOML, not typed storage a scheduler could
  borrow-check. A package joins named stages instead (Phase 3).
- **Packages selected at run time** from one all-in binary. A game would
  still carry every crate, and the size and the dependencies are the point.
- **Per-package versions now.** Meaningless while the subtree pins every
  package at one commit (decision 6).
- **For the running binary: build only the player, and the CLI on request.**
  The CLI has package features too (#1333), so it would go stale against
  the list, and `flint check` would accept what the player can't run.
- **For the running binary: run the build from a copy** (copy `flint.exe` to
  a temporary folder and re-run from it). It works, but the first run of a
  copied binary (a debug `flint.exe` is 144 MB) takes 10 to 20 seconds on this machine, and it
  does nothing for a running `flint-player.exe`.
- **For the running binary: a separate output folder per build.** Cargo's
  `--artifact-dir` is unstable, and a target folder per game is gigabytes a
  build (the disk filled during worktree builds already).
- **The word "module" or "plugin".** "Module" is taken twice, by Rust
  modules and by script modules (`ScriptModule`); "plugin" suggests the
  run-time loading rejected above. "Package" is also Unity's word.

## Consequences

- Every Phase 1 ticket cites this record: #1331 writes the manifests and
  the completeness test over the table in decision 7, #1332 moves schemas,
  #1333 gives the CLI its features, #1334 reads `[packages]`, #1335 builds,
  #1336 holds the feature matrix in CI.
- A game that writes nothing changes nothing.
- `procgen` and `debug-hud` can be dropped in Phase 1; the other standard
  packages wait for Phase 3, and ocean, grass, stylize and panels for Phase
  4. Until then their components stay in every game's vocabulary.
- A new crate, component, scene table, script module or command has to name
  its package. From #1331 a test fails until it does.
- `flint build` leaves a `*.old.exe` beside a binary that was running until
  that binary's next start.

## Measured

- **The running-binary lock** (a scratch crate, Windows 11, 2026-11-24):
  with the crate's own `.exe` running, `cargo build` fails with `Access is
  denied. (os error 5)` on removing it. From the running `.exe`,
  `std::fs::rename` to `lockt.old.exe` succeeds while a second copy also
  runs from it, `cargo build` then exits 0, and the new binary runs and
  prints the new version. `std::fs::remove_file` on the renamed file fails
  with os error 5 while it runs.
- **What the games use** (their scenes outside `engine/`, 2026-11-24):
  Float 124 scenes and 80 procgen specs, FlintKart 44 scenes and 27 specs,
  Kandinski 33 scenes and none. Kandinski's styles are in 30 scenes. None
  of the 201 scenes uses living props, the wooze field or a music session.

## Decisions needed

Each has a recommendation; none is settled until Chris says so.

1. **The word "package"**, or "module" or "plugin". Recommended: package
   (see Alternatives).
2. **Are procgen and the debug HUD standard or feature?** Recommended: both
   standard. Two of the three games generate models from specs (Float 80,
   FlintKart 27), and a game that forgot to list procgen would load a scene
   whose models silently aren't made. The debug HUD is what a developer
   wants on every build and a shipped game drops with `debug-hud = false`.
3. **Does a missing `[packages]` key mean "everything" forever, or only
   until Phase 2?** Recommended: forever. It costs one branch, an older
   engine ignores the key anyway, and from Phase 2 `flint check` can suggest
   `flint package used --write` instead of failing.
4. **Does the rhythm layer leave the repo in Phase 5, and for which repo?**
   Recommended: yes, to the repo of the game that uses it then, as the first
   package the engine does not carry; decided at Phase 5's ADR.
5. **Does the Play site build one player per game?** Recommended: one
   player with every package until Phase 3 gives `flint export web` the
   project's own list, then one per game.
6. **The subtree at Phase 5:** games stay on it, move to tagged
   dependencies, or either. Recommended: either, with the subtree the
   default until one game has shipped on a tag.
7. **Does `vehicle` start right after Phase 2,** ahead of Phases 3 to 5?
   Recommended: yes. It tests the public `Package` API on code written for
   it, and FlintKart is waiting for it.
8. **(New) `gamepad` as a build option, not a standard package**, and the
   CLI-side `flint-viewer` and `flint-constraint` in the kernel. Recommended
   as written above.
