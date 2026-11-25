# CLAUDE.md

This file provides guidance to Claude Code when working with this repository. Every session reads all of it, so it holds only what every session needs (#992).

## Project Overview

Flint is a CLI-first, AI-agent-optimized 3D game engine written in Rust. The primary interface is CLI and code; visual tools validate results rather than create them. Phases 1-5 complete (ECS, constraints, PBR rendering, scripting/physics/audio/animation, AI asset generation). Game projects live in their own repositories with the engine as a git subtree.

## Build & Development Commands

```bash
cargo build                    # Debug build (--release for release)
cargo build -p flint-cli --no-default-features  # Headless CLI: no devices or packages (#221, #1333)
cargo run --bin flint -- <cmd> # Run CLI
cargo run --bin flint-player -- flint-examples/phase4_runtime.scene.toml  # Standalone player
cargo test                     # Run all tests (-p flint-core for one crate)
cargo clippy                   # Lint
cargo fmt --all                # Format
bash scripts/ci.sh             # What a PR must pass, in seconds; `issue.sh land` runs it
bash scripts/ci.sh --full      #   ...plus what CI's clippy and wasm jobs run (two builds)
```

**Build output:** keep `target/` per worktree; never point several at one `CARGO_TARGET_DIR` (#762). Disk runs out: a release or `--profile profiling` build adds GBs, so delete your worktree's `target/release` when done. What `ci.sh` checks, disk guards, the dev profile: `scripts/CLAUDE.md`.

## Development Workflow

Flint uses an **edit -> validate -> play** loop (examples and flags: `crates/flint-cli/CLAUDE.md`, each `--help`):

1. **`flint render`** -- Headless scene-to-PNG snapshot. Primary validation tool for AI agents.
   ```bash
   flint render levels/demo.scene.toml --output test.png --distance 20 --pitch 30 --yaw 45 --no-grid
   ```
2. **`flint edit <file>`** -- Interactive editor for a scene (hot-reload), a `.glb` or a `.procgen.toml`.
3. **`flint gen <spec>`** -- A procgen spec to a GLB or PNG.
4. **`flint play <scene>`** -- Full game runtime with physics, scripting, audio, and input (`cargo run --bin flint -- play <scene>`).
5. **`flint capture <shots.toml>`** -- A shot list as deterministic frame sequences.

**For AI agents:** before playing, rendering or bundling, `flint check <dir|scene> [--json]` is the pre-flight (#687): every problem of a project in one run. Every command that opens no window takes the global `--json` (#710): one JSON document on stdout; read answers from it. After a gameplay change, `flint playtest <scene> --input-script <file>` (#714) plays it headless and fails on what didn't happen. After visual changes, always run `flint render` to verify before moving on.

## Architecture

33-crate Cargo workspace (plus `tools/arch-analyzer`, which feeds the book's crate table). Every member but `flint-android` is a default member, so plain `cargo test` runs every desktop crate's tests (#758):

```
flint-cli           CLI binary (clap). Commands: init, entity, scene, query, schema, script, edit, play, playtest, capture, check, frame, profile, env, validate,
  │                 examples, asset, render, style, gen, prefab, bundle, export, music, goldens, dxc, package, build
  ├── flint-asset-gen AI asset generation: providers (Flux, Meshy, ElevenLabs, Mock)
  ├── flint-procgen   Procedural generation: Generator trait, registry, specs, GLB export
  ├── flint-procgen-ai AI-assisted procgen specs
  └── flint-ai-core   What every paid AI call goes through: retries, prices, cost ledger
flint-android       Android entry point (NativeActivity)
flint-player        Standalone player (game loop, physics, audio, animation, scripting, egui HUD)
  ├── flint-rhythm   Rhythm layer: charts, judgment, coherence, replay, haptics
  ├── flint-music    Music: suite manifests, buses, tempo maps, conductor, offline render
  ├── flint-input-capture 1 kHz gamepad capture stamped against the audio clock
  ├── flint-debug-ui F3/F4 debug panels and shared widgets
  ├── flint-script   Rhai scripting with hot-reload
  ├── flint-living   Living props' runtime: pose rings, clocks, knobs
  ├── flint-particles Particle sim: effects, curves, forces, bursts, sub-emitters
  ├── flint-animation Property tween + skeletal animation
  ├── flint-audio    Kira spatial audio
  ├── flint-audio-worklet Kira mixer for the browser's AudioWorklet
  ├── flint-terrain  Heightmap terrain with splat-map blending
  ├── flint-physics  Rapier 3D (character controller, bodies, joints)
  ├── flint-runtime  GameClock, InputState, EventBus, GameStateMachine, PersistentStore, pads
  ├── flint-viewer   egui inspector: gizmos, undo/redo, TOML write-back
  ├── flint-import   glTF model importer
  ├── flint-asset    Content-addressed asset catalog
  ├── flint-constraint Constraint evaluation + auto-fix
  ├── flint-query    PEG query language (pest parser)
  ├── flint-scene    TOML scene serialization/deserialization
  ├── flint-render   wgpu PBR renderer, post-processing, styles
  ├── flint-pixels   Texel loops on load, optimized in debug builds
  ├── flint-ecs      The world: stable EntityIds, dynamic components, hierarchy
  ├── flint-schema   Component/archetype schema system
  ├── flint-wooze    The wooze field and the boil clock
  ├── flint-geometry Ocean spectrum, splines, profile sweeps, mesh rays
  └── flint-core     EntityId, Transform, Vec3, Color, FlintError, vfs
```

## Where the detail lives

Feature notes go in the nearest scoped CLAUDE.md, not in this file. When a ticket says 'update CLAUDE.md', it means the scoped file for the code you changed; add to this root file only what every session needs.

| Scoped file | Covers |
|---|---|
| `scripts/CLAUDE.md` | `ci.sh`, CI, build output, disk, clippy rules, size budget, changelog |
| `schemas/CLAUDE.md` | schema layering, the examples' layer, readers |
| `crates/flint-ecs/CLAUDE.md` | entity ids, `ComponentsMut`, `merge_component` |
| `crates/flint-cli/CLAUDE.md` | the loop's examples, `render` flags, `edit`, `gen`, goldens, fixtures, export |
| `crates/flint-render/CLAUDE.md` | frame and GPU rules, wgpu, post-processing, quality, caches, lights |
| `crates/flint-render/src/stylize/CLAUDE.md` | styles, style mask, per-entity and user styles |
| `crates/flint-procgen/CLAUDE.md` | living props, wooze field, Woozy Grove |
| `crates/flint-player/CLAUDE.md` | extensions, UI, input, touch, capture, perf, web |
| `crates/flint-script/CLAUDE.md` | Rhai detail, callbacks, script UI |
| `crates/flint-core/CLAUDE.md` | vfs, hot reload, shear, project manifest, env switches |
| `crates/flint-audio/CLAUDE.md` | audio clock, decoding, device features |
| `crates/flint-{physics,animation,particles,terrain}/CLAUDE.md` | joints, IK, particles, terrain |
| `flint-examples/film/CLAUDE.md` | the film: shots, score, editor (flint-examples, #1195) |
| `showcase_game/CLAUDE.md` | the example game |
| `docs/CLAUDE.md` | the book, its checks, design notes, ADRs |

A bullet below that names a feature in a line or two has its full text in one of these. **This file has a budget** (`scripts/claude-md-check.py`, in `scripts/ci.sh`, #992): a PR taking it over fails, as does a scoped file missing from this table. Make room by moving detail out, not by raising the number.

## Key Design Principles

- **Dynamic components** stored as `toml::Value` -- archetypes defined at runtime in schema TOML files, not Rust types
- **Stable EntityIds**, never recycled, the same every load (#688); a seed meant to survive edits comes from the entity's **name**
- **Scene format is TOML** -- `[scene]` metadata + `[entities.<name>]` sections; reload is full re-parse
- **Schemas in `schemas/`** -- directories layer (game over engine): `--schemas` flags (the whole list), else `[project] schemas` in `flint.toml`, else `schemas/` nearby (ADR 0114)
- **Scripting via Rhai** -- `script` component with `.rhai` files in `scripts/`; `ScriptCommand` for deferred effects; `flint script api` lists every function
- **All game UI is script-driven** -- no hardcoded HUD: scripts draw it, and a menu is a UI document (`.ui.toml` + `.style.toml`, `load_ui`)
- **Game project pattern** -- the engine as a git subtree at `engine/`; `schemas = ["engine/schemas", "schemas"]` under `[project]` in the game's `flint.toml`; a game binary embeds `PlayerApp` with `PlayerExtension`s
- **Every runtime read goes through `flint_core::vfs`** (#97) -- never `std::fs` or `Path::exists` in a runtime crate (tests and the CLI keep `std::fs`); `cargo clippy` denies it, and a call that means it says why, `#[allow(clippy::disallowed_methods)] // <reason>`
- **`FLINT_*` environment switches are one table** (#715, `flint_core::env`): read one only through its getter; a new switch is a row there
- **The engine is packages** (ADR 0136): every crate, component, archetype, scene table, script module and command is in one `packages/<name>/package.toml`'s `[provides]`, which a test holds
- **A game lists its packages** under `[packages]` in `flint.toml`; `flint build` builds its player and CLI from it

## Critical Gotchas

- A package's code is behind its Cargo feature; `bash scripts/package-check.sh` builds each alone (#1336)
- Read a component through `world.view(id, comp::COLLIDER)` (ADR 0115), other TOML through `flint_core::toml_util`; never `toml::Value::as_float`: it reads `8` as nothing
- `world.get_components_mut(id)` is a `ComponentsMut` guard (bind it `mut`); `despawn` takes the whole subtree
- Runtime crates build for wasm (`bash scripts/wasm-check.sh` before pushing): time through `web-time`, never `Instant::now`
- `AudioManager::new()` / `Gilrs::new()` may fail in headless/CI -- wrap in `Option`
- `flint-android` is the one crate outside `default-members` (a new crate goes in both lists); `cargo test --workspace` fails on `ndk-sys` on Windows: use plain `cargo test`
- On Windows `flint` and `flint-player` link an 8 MB stack from `build.rs` (#990): keep a clap `Args` struct to a dozen or so flags
- Dependency API notes live in their crates' files: wgpu 30, winit 0.30 (render), Rapier 0.22 (physics), Kira 0.11 (audio), Rhai 1.24 (script)
- Never allocate GPU buffers or bind groups per entity per frame; a frame is one submission (#139)

## Documentation Pipeline

A change a game can notice (a scene or schema key, a script function, a CLI flag, a format, a look) gets a changelog fragment, `bash scripts/changelog.sh new <issue> <slug>`, never an edit to `CHANGELOG.md` (#788). A change to what the book (`docs/book`) describes updates the book in the same PR, checked against the code.

## Project Structure

- `schemas/` -- engine schemas, `schemas/examples/` the examples' room-builder layer; `styles/` style guides, looks; `packages/` manifests
- `flint-examples/` -- the examples, their own repository (#984): clone it here (gitignored). `tests/fixtures/scenes/` -- frozen scenes the goldens load
- `showcase_game/` -- the example game; `docs/book/` the book, `docs/design/` notes, `docs/adr/` decisions
- **Every byte here is in every game** (ADR 0122): `scripts/size-check.py` caps the tree. The root holds only manifests and docs

<!-- claims:begin (from chrischaps/Dashboard skills/setup-claims; re-run its install.sh to update) -->

## Claiming issues

Agents on several machines work this repo, all as the same GitHub user, so an issue is claimed before anyone works on it. A claim is the branch `claim/issue-<n>` on origin, holding one empty commit that names the agent and host. Git accepts only the first push of a new branch, so exactly one claimant wins. `scripts/issue.sh` does the whole cycle (bash; on Windows it runs under Git Bash):

```bash
scripts/issue.sh list                  # who holds what, on every machine, and for how long
scripts/issue.sh claim <n>             # claim and make the worktree ../Flint-<n>; exit 2: taken, pick another
scripts/issue.sh claim <n> --here      # claim for the worktree you're already in (a desktop-app worktree)
scripts/issue.sh claim <n> --base <b>  # branch from, and land on, the feature branch <b>; adds to either form above
scripts/issue.sh renew <n>             # at the start of each session on long work
scripts/issue.sh land <n>              # verify, push the branch and open a PR to main; the merge closes the issue and that releases the claim
scripts/issue.sh release <n>           # giving up: push your branch and comment where it stands first
scripts/issue.sh reap [--dry-run]      # delete the claims whose issue is closed; the SessionStart hook runs it
```

A claim ends when its issue closes: `.github/workflows/release-claim.yml` reaps it within a minute of the merge (or of any close), whichever machine held it, so nobody runs `release` after a merge.

- **A hook enforces it.** `.claude/settings.json` runs `scripts/issue-guard.sh` before every `git commit` and `git push`. It refuses issue work (a branch named `issue-<n>`, or a commit saying `Closes #<n>`) unless this session holds the claim, from the worktree it was claimed for. A chore with no issue (a typo, a dependency bump) needs no claim: keep it off `issue-<n>` branches and out of `Closes` lines. A branch built on a sibling's carries the sibling's `Closes` line; once that issue is closed (the sibling merged), the push passes without its claim. A SessionStart hook reaps the claims on closed issues, then lists the rest.
- **Only the session that claimed an issue renews, releases or lands it,** and only from its worktree. An existing worktree for an issue you didn't claim is another session's: leave it alone. Never delete another agent's `claim/*` branch, with one exception: a claim on a closed issue guards nothing, and `scripts/issue.sh reap` is the one way to clear it.
- **Work on a feature branch is claimed with `--base <branch>`,** never by changing `scripts/issue.conf`, which would ride into the default branch with the merge. The claim commit records the base, and `renew`, `--steal`, `land` and the hook all follow it. GitHub closes issues only for merges to the default branch, so merging the PR into the claim's base closes the issue, with a comment naming the PR and the branch, and releases the claim. Cut the feature branch from a default branch that already holds the release-claim workflow.
- **Claims go stale after 24 hours.** `claim <n> --steal` takes over a stale one; check the issue's comments and `origin/issue-<n>` for work in progress first, and continue from it. Taking a younger claim is Chris's call only: `claim <n> --steal --override "<his reason>"`.
- Settings for this repo are in `scripts/issue.conf`. The tool is vendored from `skills/setup-claims` in chrischaps/Dashboard: change it there, not here. `bash scripts/issue-test.sh` tests it against a throwaway remote.

<!-- claims:end -->
