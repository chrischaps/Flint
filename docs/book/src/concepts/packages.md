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

The list decides what the game's schemas and script API hold. A scene, prefab or script that uses a package the list leaves out is an `unlisted-package` error in `flint check`, `flint validate` and `flint script check`, and its hint is the line to add. That includes a field another package gives its meaning: a `music_session` with a `chart` in a game that lists `music` alone gets ``the field `music_session.chart` is the `rhythm` package's``, with the hint to add `rhythm = "*"`. A name no package has, such as `livng = "*"`, is a `bad-manifest` error that says `did you mean `living`?`. The player agrees with `flint check` even when it was built with the package: `flint play` words the scene's load summary the same way, and runs none of a left-out package. Its props stand still, its script modules are "not enabled", and one line at the scene's entry names `[packages]` and the line to add. With `procgen = false` no spec is made: a model or texture a spec names loads from its file, as in a build without procgen, and the line counts the names (`the scene's 1 asset named by a spec (rock) load from files, where there are any`). `flint render` follows the same list. [Packages a game uses](../guides/building-a-game-project.md#packages-a-game-uses) has the rules in full and the commands' output.

A web bundle follows the same list. `flint bundle` packs and lists only the schema folders of the packages the game has, so a game that doesn't list `living` ships no `living_prop` and no `[wooze]`. A packed scene that uses a left-out package is refused with the same `unlisted-package` error and hint `flint check` gives, and nothing is written. A game with no `[packages]` table bundles as it always did, with every package's schemas.

## Asking what is used

You don't have to work the list out by hand. `flint package used` reads the project's scenes, chunks, prefabs and scripts and names the package of each component, archetype, scene table and script call it finds, and of each procgen spec a scene names. It reads fields too. A package can give its meaning to a field of another package's component (its `[provides] fields`), so a `music_session` that sets `chart` names `rhythm` as well as `music`: rhythm is what judges the chart, and without it the scene plays as music alone. `--write` adds what it found to `[packages]`, and keeps every line already there:

```bash
flint package used
flint package used --write
```

A standard package is in unless set to `false`, so a list of what was found still gives the game procgen. When nothing the scan reads names a procgen spec, `flint package used` also prints `procgen = false   # nothing scanned uses it` as a line to add. `--write` leaves that line for you to write, because it changes what `flint build` gives the game. `debug-hud` is never suggested, since no file can show the F3/F4 panels in use.

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

## A package is a folder

An engine package lives in `packages/<name>/`: its `package.toml`, its crates under `crates/`, and its schemas under `schemas/`, laid out as the engine's are. Nothing outside that folder names it. The stock `flint-player` and `flint` read the manifests when they build. A manifest's `[rust]` table names the crate whose player part (`player::PACKAGE`) and commands (`cli::extensions()`) the launchers list, and the engine's schema folder brings every package's `schemas/` with it.

A player knows only the schemas of the packages it has. One built or launched without `living` reads no `packages/living/schemas`, whether the engine's tree on the disk holds it or a bundle packed it, and the web player carries only the schema text of the packages it was built with. Its load summary calls a `living_prop` an `unknown-component`.

What a player built without a package still says of it comes from the manifest too. A scene that uses the package's components or scene tables (its `[provides]`) hears one line when it's entered, with the manifest's `[without] does` and what to build: ``[living] this player is built without the `living` package: the scene's living_prop, [wooze] stand still; run `flint build` in the project for a player with it``. That line is the only thing said of those names. Such a player doesn't read the package's schemas, so the scene's load summary would otherwise call `living_prop` an `unknown-component`. It leaves them to the line instead, and `flint playtest` counts one warning for them. Where `[packages]` leaves the package out, the line gives the line to add before `flint build`, and the load still says `unlisted-package`. The `[without] policies` names are game-state names a game may give in every build, so a state that pauses `living` still registers where living isn't built. A package's own command-line flags are its own as well: living registers `--compare` and `--split` through the Package API (`Registry::player_args`), and a player built without it has neither.

```toml
[without]
does = "stand still"
policies = ["living"]
```

Cargo can't read a manifest, so one script writes the lines it needs. `scripts/package-sync.py` writes a generated block in `crates/flint-player/Cargo.toml` and `crates/flint-cli/Cargo.toml`: the `packages` feature (every package, which both binaries' defaults turn on), and for each package its feature and an optional dependency on its crate. Two more manifest tables become lines there. `[rust.devices]` names what the launchers' device features turn on in the package's crates, as `crate/feature`. The sound card, gamepads and the editors' file dialogs are the features `sound`, `gamepad` and `dialogs`, and a build without one has no such device library. (The sound card's was `audio` until the audio package took that name.) `[with.<other>]` names a part that needs another package listed too, and it's turned on only when both are:

```toml
[rust]
player = "flint-rhythm-game"
cli = "flint-rhythm-game"

[rust.devices]
sound = ["flint-rhythm-game/device"]
gamepad = ["flint-rhythm-game/gamepad"]

[with.debug-hud]
features = ["flint-rhythm-game/debug-hud"]
```

Adding a package is two steps:

```bash
# 1. create packages/<name>/ (package.toml, crates/, schemas/)
# 2. write the generated lines; the next cargo command adds the crate to Cargo.lock
python3 scripts/package-sync.py
```

Until the script runs, the launchers have no feature for the new package, and both binaries name the script. A `flint` built from the tree reads the folder's manifest, so a command the package provides exits 2: the package's folder is in `packages/`, neither launcher has a feature for it yet, and the step is `python3 scripts/package-sync.py` from the engine's root, then a build. `flint build` for a game that lists the package is refused the same way: neither flint-player nor flint-cli has its feature, so run the script in the engine it names, then `flint build` again. A game that lists the package hears the same from the checks that hold a list to the binary: `flint check` reports `unbuilt-package`, and `flint play`, `flint-player` and `flint render` refuse to run, each naming the script and the engine first and `flint build` after. A package the launchers already have, which the game left out, still gets the advice above: list it under `[packages]` and run `flint build`.

Removing one is deleting the folder and running the script again. Nothing else is edited by hand: no kernel crate's source names a package, the player's and the CLI's included (`scripts/package-names-check.py` holds every crate the engine's manifest lists, with a short list of exceptions, each waiting on the issue that removes it), and `scripts/ci.sh` fails on a stale block with `package-sync.py --check`, naming the command. The six packages above are written the same way: no package's feature is written by hand in either `Cargo.toml`, and the script refuses one that is.

CI holds this on every pull request with a toy package (`tests/fixtures/packages/toy`). It has one component, a spring pulled in each fixed physics step, its own script module and a command, which is what a vehicle package needs. `scripts/package-toy.py` copies it into `packages/toy/`, runs the script, builds the stock binaries and plays a scene that uses it. Then it deletes the folder, builds again, and checks that the checkout is back byte for byte. A package that would need an edit anywhere else fails that build.

## Where the commands are

| Command | What it answers |
|---|---|
| `flint package list` | Every package of the engine: tier, version, requirements, build features, and what it owns. `--built` lists only those this `flint` was compiled with ([The `package` Command](../cli-reference/overview.md#the-package-command)) |
| `flint package used` | The packages the project's files use, by file and item; `--write` lists them in `flint.toml` |
| `flint build` | Builds the project's `flint-player` and `flint` with the listed packages; `--dry-run` prints the plan ([The `build` Command](../cli-reference/overview.md#the-build-command)) |
| `flint script api` | The script functions the game has; in a project it ends naming the modules its list leaves out |
| `flint check` | `unlisted-package` for a use of a left-out package, `unbuilt-package` for a binary that lacks a listed one, `bad-manifest` for a line that names no package |
