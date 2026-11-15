# ADR 0119: A game registers its procgen generators through its player extension

Status: accepted, 2026-11-11 (#195)

## Context

A procgen generator is a Rust type implementing `flint_procgen::Generator`.
Until now there was one way to make one usable: add it to
`register_built_in_generators` in the engine. `ProcGenResolver::new()` built
its registry from the built-ins, and so did `flint gen` and `flint edit`. A
game that carries the engine as a subtree had no way to give the player a
generator of its own, a `rock_v1` for a cave game, without editing the
engine, which a subtree pull then fights.

The player already lets a game binary bring Rust in: `PlayerExtension`
(ADRs [0071](0071-player-extensions-in-a-game-binary.md)-[0075](0075-screen-to-ground-picking.md)), installed with `PlayerApp::add_extension`, with
`register_script_api` as the precedent for a one-time registration.

## Decision

- `PlayerExtension::register_generators(&mut GeneratorRegistry)`, a no-op by
  default, is called once from `add_extension`, right after
  `register_script_api`, with the registry of the player's own
  `ProcGenResolver`. That registry already holds the built-ins. A generator
  registered under a built-in's type name replaces it, as
  `GeneratorRegistry::register` always has.
- `ProcGenResolver` gains `registry()`, `registry_mut()` and
  `with_registry(config, registry)`. `with_config` is now `with_registry`
  over the built-ins plus `mock`.
- The player rebuilds its resolver once, when it opens the disk tier (#181),
  after extensions are added. It now moves the registry across
  (`std::mem::take(registry_mut())` into `with_registry`), so a game's
  generators survive it. Without this the hook would have worked only with
  the disk cache off.
- Nothing else changes in resolution: a game's generator goes through the
  same `check_generator`, queue, in-memory and disk caches (keyed by type
  name and revision) and catalog > procgen > file order as a built-in.

## Alternatives considered

- **A registration on `PlayerApp` (`player.register_generator(..)`)**
  instead of a hook. It would work, but a game's Rust already arrives as an
  extension, and an extension is what a game hands to other hosts later (a
  game's own CLI, the web entry). One place to look is simpler.
- **A process-wide registry** that `register_built_in_generators` also reads.
  It would reach `flint gen` and `flint render` for free, but a global makes
  tests share state and makes "which generators does this resolver have"
  depend on what ran before. Rejected.
- **Tool time now.** `flint gen` / `flint edit` / `flint render` taking a
  game's generators needs the game to build its own CLI around flint-cli's
  commands. That is a larger change with no user yet; the issue allowed
  player-only first.

## Consequences

- A game's specs resolve in the game's own binary. The stock `flint-player`
  shows a placeholder for them, with a warning naming the generator and the
  ones it knows; `flint gen`, `flint edit` and `flint render` still know
  only the built-ins.
- A generator is registered at `add_extension`. An extension added after the
  first scene has loaded is too late for that scene's specs, which have
  already fallen through to a file or a placeholder; the next scene load
  sees it.
- A game's generator should be `Send + Sync`, stateless and build for
  `wasm32` like a built-in (`crates/flint-procgen/CONTRIBUTING.md`).
- Example: `crates/flint-player/examples/cave_game/` (a game binary with
  `rock_v1`). Book: guides/player-extensions.md, "Procgen generators".
