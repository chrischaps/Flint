# Packages

> **Package:** `engine`, the kernel: every game has it. `flint package` and `flint build` are kernel commands, in every build.

A game should carry only the engine it uses. A kart racer has no reason to link a rhythm judge, and a puzzle game has none to load the schemas of living props. So the engine comes in **packages**: a package is a name a game lists in its `flint.toml`, and everything that name brings with it. That means the crates linked into the player and the CLI, the components, archetypes and scene tables in the game's schemas, the script modules its scripts can call, and the commands that work on them. A package the game leaves out isn't there at all. Its components don't validate, its functions aren't in the script API, and its code isn't in the binary.

Each package has a manifest, `packages/<name>/package.toml`, that says what it owns. So a tool or an agent can ask what exists without compiling anything, and a test in the engine holds every crate, schema, script module and command of the tree to exactly one manifest. The design is [ADR 0136](https://github.com/chrischaps/Flint/blob/main/docs/adr/0136-the-package-model.md).

## The packages

| Package | Tier | What it brings | Comes with |
|---|---|---|---|
| `engine` | kernel | The kernel: the ECS, scenes, schemas, the renderer, scripting, the player and the CLI. It also holds, for now, the parts that have no switch yet (physics, audio, animation, particles, terrain, and the ocean, grass, stylize and panels render features) | |
| `procgen` | standard | Models and textures made from procgen specs, at load and with `flint gen` | |
| `debug-hud` | standard | The player's F3/F4 debug panels | |
| `living` | feature | Living props and the wooze field: `living_prop`, `shallow_wooze`, `[living]`, `[wooze]`, and the `living`, `wooze` and `debug` script modules | `procgen` |
| `music` | feature | Music sessions: `music_session`, stems on buses, the conductor and the beat, the `music` script module | |
| `rhythm` | feature | Charts judged against the music, with 1 kHz pad capture, and the `conducted` script module | `music` |
| `ai` | feature | AI asset generation, AI-written procgen specs and the cost ledger | `procgen` |

The **tier** says who has a package without asking:

- **kernel**: every game. It is never listed and can't be left out.
- **standard**: every game unless its `[packages]` sets it to `false`.
- **feature**: only a game whose `[packages]` lists it.

`flint package list` prints this table from the manifests, with every crate, component, archetype, scene table, script module and command each one owns. Each concepts chapter of this book states its package on its first line, and the [Script API Reference](../reference/script-api.md) groups its modules by package.

## A game lists what it uses

`[packages]` in `flint.toml` lists the packages the game uses. `flint init` writes it empty, with a comment, so a new game is the kernel and the standard packages:

```toml
[packages]
living = "*"        # a feature package: in only when listed
debug-hud = false   # a standard package: in unless set to false
```

- **With the table**, the game has the kernel, every standard package not set to `false`, every feature package listed, and what those come with (`living` brings `procgen`).
- **Without the table**, the game has every package, as before packages existed. A project that never writes it changes nothing.
- A value is a version requirement (`"*"` or `"^0.1"`). It is checked for its syntax and kept, but nothing holds a game to it until packages have versions of their own.

The list decides what the game's schemas and script API hold. A scene, prefab or script that uses a package the list leaves out is an `unlisted-package` error in `flint check`, `flint validate` and `flint script check`, and its hint is the line to add. A name no package has, such as `livng = "*"`, is a `bad-manifest` error that says `did you mean `living`?`. [Packages a game uses](../guides/building-a-game-project.md#packages-a-game-uses) has the rules in full and the commands' output.

## Asking what is used

You don't have to work the list out by hand. `flint package used` reads the project's scenes, chunks, prefabs and scripts and names the package of each component, archetype, scene table and script call it finds, and of each procgen spec a scene names. `--write` adds what it found to `[packages]`, and keeps every line already there:

```bash
flint package used
flint package used --write
```

## Building the game

The list also decides what the binaries link. A `flint` or `flint-player` links what it was compiled with, whatever the list says, so after changing the list you build the game's own pair:

```bash
flint build --dry-run
```

`--dry-run` prints the plan: the packages in and out and why, the Cargo features of each binary, the exact `cargo build` command, and where the two binaries will land. `flint build` runs it (`--release` for optimized binaries), in the engine the game carries in `engine/`, or the checkout its schemas name. A binary that lacks a package the list names refuses to play or render that game, and `flint check` reports an `unbuilt-package` error whose hint is `flint build`. [Building the game](../guides/building-a-game-project.md#building-the-game-flint-build) has the output and the details.

## Adding a package, start to finish

Say a game wants living props. Its scene gains a `living_prop` entity, and then:

1. **`flint check` says what's missing.** The new entity is an `unlisted-package` error: `the component `living_prop` is the `living` package's, and [packages] doesn't list it`, with the hint `add `living = "*"` under [packages] in flint.toml`.
2. **List it.** Add `living = "*"` under `[packages]` in `flint.toml`, or run `flint package used --write`, which adds every package the project reaches. `procgen` comes along, since `living` needs it.
3. **Rebuild.** `flint check` now passes the scene and, while the old `flint` has no `living`, reports `unbuilt-package` with the hint to run `flint build`. Run it (`flint build --dry-run` first to see the plan). The first time, before the game has a `flint` of its own, run the engine's from the project root: `cargo run --manifest-path engine/Cargo.toml -p flint-cli -- build`.
4. **Play.** `flint check` is clean, and `flint play` and `flint render` draw the props.

Leaving a package out is the same in reverse. Remove its line, or set a standard package to `false`. `flint check` names every scene and script that still uses it, and `flint build` makes binaries without it.

## Where the commands are

| Command | What it answers |
|---|---|
| `flint package list` | Every package of the engine: tier, version, requirements, build features, and what it owns. `--built` lists only those this `flint` was compiled with ([The `package` Command](../cli-reference/overview.md#the-package-command)) |
| `flint package used` | The packages the project's files use, by file and item; `--write` lists them in `flint.toml` |
| `flint build` | Builds the project's `flint-player` and `flint` with the listed packages; `--dry-run` prints the plan ([The `build` Command](../cli-reference/overview.md#the-build-command)) |
| `flint script api` | The script functions the game has; in a project it ends naming the modules its list leaves out |
| `flint check` | `unlisted-package` for a use of a left-out package, `unbuilt-package` for a binary that lacks a listed one, `bad-manifest` for a line that names no package |
