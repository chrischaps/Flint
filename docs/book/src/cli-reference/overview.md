# CLI Reference

Flint's CLI is the primary interface for all engine operations. Below is a reference of available commands.

## Commands

| Command | Description |
|---------|-------------|
| `flint init <name>` | Start a game project: manifest, lit scene, HUD script, input config, agent guide ([below](#the-init-command)) |
| `flint entity create` | Create an entity in a scene |
| `flint entity delete` | Delete an entity from a scene |
| `flint scene create` | Create a new scene file |
| `flint scene list` | List scene files |
| `flint scene info` | Show scene metadata and entity count |
| `flint query "<query>"` | Query entities with the Flint query language |
| `flint package list [--json]` | The engine's packages, from their manifests (`packages/<name>/package.toml`): each one's tier, version, requirements, build features, and the crates, components, archetypes, scene tables, script modules and commands it owns; `--built` lists only those this `flint` was compiled with ([The `package` Command](#the-package-command)) |
| `flint build [--release] [--dry-run]` | Build the project's `flint-player` and `flint` with the packages its `flint.toml` lists: the cargo command it runs and where the binaries land; `--dry-run` builds nothing ([The `build` Command](#the-build-command)) |
| `flint env [--json]` | Every `FLINT_*` environment switch, what it accepts and its value now; exit 1 on a value it can't take ([Environment Switches](environment.md)) |
| `flint schema <name>` | Inspect a component or archetype schema; an unknown name exits 1. `--list` names every one loaded (`--json`); `--format json` is the schemas as data; `--format json-schema [--output file]` is a JSON Schema for a `.scene.toml`, for editor completion ([Introspecting Schemas](../concepts/schemas.md#introspecting-schemas)) |
| `flint script api` | Every function a script can call, with its parameters and a one-line doc, and the callbacks; `--json` for the same as data, `--filter <text>` for names or modules containing it. A function a game's extension registers on the Rhai engine directly is listed with its parameter types only. `--book` writes the book's [Script API Reference](../reference/script-api.md) and the generated tables in its chapters; `--check-book` fails when they are stale or a chapter calls a function that doesn't exist (CI runs it) ([Scripting](../concepts/scripting.md#api-reference)) |
| `flint script check [dir\|scene\|file.rhai]` | Compile every script and the modules it imports, and check each call's name and argument count against the script API, each callback's name and parameters, and each import, reported as `file:line:col`. Exits 1 on an error; `--json`; `--allow` / `--api` for a game's own functions ([Checking your scripts](../concepts/scripting.md#checking-your-scripts)) |
| `flint check [dir\|scene]` | The pre-flight: everything wrong with a project, or with one scene, in one run. Schemas, entity references, every asset and file a scene names (with the paths tried), styles and looks, constraints, scripts. Exits 1 on any error; `--json` ([The `check` Command](#the-check-command)) |
| `flint frame show <capture>` | A frame capture's summary (scene, adapter, size, GPU time by pass, draws and triangles, memory), and `--passes`, `--draws [--pass P] [--entity E] [--sort tris|gpu] [--culled]`, `--targets`, `--hitch`; `flint frame diff <a> <b> [--out dir]` says what changed between two; `flint frame image <capture> <pass> -o x.png` copies a pass's image out to look at; `flint frame open <capture>` is the frame debugger's window ([The Frame Debugger](../guides/frame-debugger.md)); `flint frame replay <capture>` draws it again, a draw at a time or with an entity or pass left out. All but `replay` read only the capture folder; `--json` ([The `frame` Command](#the-frame-command)) |
| `flint profile explain <trace> [frame]` | Why a frame was slow, as a sentence first: each scope of a `--trace` file against its median over the trace's normal frames, the deepest scope that carries the excess named in the first line, then what ran only in that frame. `flint profile top <trace>` lists the scopes by total, self time, worst call or calls (`--thread`, `--frames a..b`); `flint profile frame <trace> <n\|worst\|hitch>` prints that frame's scope tree; `flint profile diff <a> <b>` compares two traces scope by scope and marks the deltas the frames' own spread explains. Reads only the file; `--json` ([The `profile` Command](#the-profile-command)) |
| `flint validate <scene>` | Check one scene against its schemas (every unknown archetype, component and field, every value of the wrong type, every entity reference to nothing and every missing asset, in one run: [Scene Diagnostics](../concepts/schemas.md#scene-diagnostics)), its constraints, and its `[wooze]` table. Exits 1 on any error; `--strict` makes an unknown field one; `--format text\|json`; `--fix` applies what a constraint can fix |
| `flint examples` | List the engine's example scenes by topic, each with its purpose (`--topic`, `--json`); `--check` fails when one is missing from [`flint-examples/index.toml`](https://github.com/chrischaps/flint-examples/blob/main/index.toml) or fails validation |
| `flint asset import` | Import a file into the asset store |
| `flint asset list` | List assets in the catalog |
| `flint asset info` | Show details for a specific asset |
| `flint asset resolve` | Check asset references in a scene |
| `flint asset where <kind> <name> [--scene x] [--json]` | Print the file the engine would load for an asset name and the paths it tried first; exits 1 with the list when there is none. See [Assets](../concepts/assets.md#where-the-engine-looks-for-a-file) |
| `flint asset generate` | Generate an asset using AI providers |
| `flint asset validate` | Validate a generated model against style constraints |
| `flint asset manifest` | Generate a build manifest of all generated assets |
| `flint asset regenerate` | Regenerate an existing asset with new parameters |
| `flint asset costs [--since DATE]` | What AI calls have cost, from the project's `.flint/ai_costs.jsonl`: assets, procgen specs and looks together |
| `flint asset job status` | Check status of a generation job (a Meshy model) |
| `flint asset job list` | List all generation jobs |
| `flint asset job resume <id>` | Finish a job a generate didn't see through: wait, download, register; nothing new is paid for |
| `flint edit <file>` | Unified interactive editor (auto-detects file type) |
| `flint play [scene]` | Play a scene with first-person controls and physics; with no scene, the project's default (`flint.toml`'s `[project] scene`) |
| `flint playtest <scene>` | Play a scene headless on a fixed clock, with an input script's `expect` / `wait` lines and the scripts' `test_*` calls; one JSON report, the same bytes every run, exit 1 on a failure ([Playtesting](../guides/playtesting.md)) |
| `flint capture <shots.toml>` | Capture a shot list as deterministic frame sequences, each take a stepped `flint play`; `--verify` runs each twice and names the frames that differ, `--encode mp4` ([Filming a Showcase](../guides/filming-a-showcase.md)) |
| `flint render <scene>` | Render a scene to PNG (headless) |
| `flint dxc status` | What DX12 compiles shaders with here, and why: DXC is linked into the binary ([DX12's shader compiler](#dx12s-shader-compiler)); `flint dxc notices` writes DXC's licence and notices for a Windows build you ship |
| `flint goldens <file>.goldens.toml` | Render a list of scenes and gate each against its golden PNG; `--bless` rewrites them ([Golden Suites](../guides/headless-rendering.md#golden-suites)) |
| `flint gen <spec>` | Run a procedural generation spec to produce meshes or textures |
| `flint prefab view <template>` | Preview a prefab template in the viewer |
| `flint bundle <game>` | Pack a game folder into `.flintpack` bundles for the web player, running `flint check`'s file and load checks over the packed files first. Given a `.scene.toml`, it packs that scene alone with only what it reaches ([One scene](../guides/publishing-to-the-web.md#one-scene-flint-bundle-scene)) |
| `flint export web <game>` | A folder any static server plays the game from: the web player, the bundles and `games.json` ([Publishing to the Web](../guides/publishing-to-the-web.md)) |
| `flint music validate <manifest>` | Validate a musical suite manifest and optional chart ([music commands](music.md)) |
| `flint music play <manifest>` | Play a suite's stems sample-locked, no judgment |
| `flint music calibrate <manifest>` | Tap-to-beat latency calibration, written to `logs/latency/` |
| `flint music chart play <manifest>` | Play a suite against its chart with live gamepad capture |
| `flint music chart replay <manifest>` | Replay a recorded or synthetic session through judgment, headless |
| `flint music render <manifest>` | Render a music `.suite.toml` to WAV, offline and deterministically |
| `flint goldens <file>.goldens.toml` | Render a list of scenes and gate each against its golden PNG ([Golden Suites](../guides/headless-rendering.md#golden-suites)) |

The music commands are documented on their own page: [Music Commands](music.md). Until #709 they were top-level (`validate-suite`, `play-suite`, `calibrate`, `play-chart`, `replay-chart`, `render-suite`); those names ran for one release and are gone since #1141, as is `spike-rumble`: the rumble spike is an example program now.

## Machine-readable output

Every command that doesn't open a window takes `--json`, a global flag that goes anywhere on the line. With it the command prints exactly one JSON document on stdout, and every other line it would have printed (progress, warnings, what it wrote) goes to stderr, so `flint <cmd> --json | jq` always has one value to read:

```bash
flint check --json                          # the report: valid, errors, diagnostics...
flint scene info scenes/main.scene.toml --json
flint gen specs/stone_wall.procgen.toml -o wall.png --json   # the variants it made, and their files
flint render scenes/main.scene.toml --json  # the render's report (--report json)
```

The exit status says whether the command succeeded: 0, or 1 on any failure. A command that answers with a report prints it whether or not it failed, so `flint check` of a broken project prints its `"valid": false` report and exits 1. Any other failure prints an error document instead, with the message and the errors that caused it:

```json
{ "error": { "message": "no component, scene table or archetype named `nope`", "causes": [] } }
```

A mistake on the command line itself (an unknown flag or subcommand) is an error document too, `{"error": {"message", "usage"}}`, with clap's exit status 2; `--help` and `--version` print as always. A command that opens a window (`play`, `edit` other than a model's or effect's `--render`, `prefab view`, `music play`, `music calibrate`, `music chart play`) refuses `--json` with an error document rather than opening one. `flint render --watch` refuses it too, since it never finishes.

Where a command already had its own form, `--json` is that form: `validate`'s `--format json`, `query`'s and `entity list/show`'s `--format json`, `schema`'s `--format json` (a `--format json-schema` stays a JSON Schema), `render`'s `--report json`. `flint gen --report json` still prints a JSON line a variant; with `--json` the variants are one document's `variants` list instead. Paths in the documents use `/` on every platform. `crates/flint-cli/tests/json_cli.rs` runs every command `--help` lists with `--json` and parses what it prints, so a new command has a JSON form or is named as a window.

## The `init` Command

```bash
flint init my_game                       # the walk template, naming this engine checkout by path
flint init my_game --template 2d         # walk | blank | 2d
flint init my_game --engine subtree      # engine/ inside the game: prints the git subtree add to run
flint init my_game --engine-path ../Flint
flint init . --force                     # into a directory with files; existing files are kept
```

It writes a game project from templates compiled into the binary: `flint.toml` (`[project] id`, `schemas`, `scene`, `input`), `scenes/main.scene.toml` (lit, with a `[camera]`), `scripts/hud.rhai` on a `hud_controller` entity, a pause menu (`ui/pause.ui.toml` and `ui/pause.style.toml`, opened by Escape or a pad's Start), `config/input.toml`, `bundle.toml`, `.gitignore`, an empty `schemas/` for the game's own components, `renders/`, and `CLAUDE.md` + `AGENTS.md` for coding agents. Then it prints the commands to run next, none of which needs `--schemas`.

| Flag | Description |
|------|-------------|
| `--template walk\|blank\|2d` | The first scene. `walk` (default): a first-person player facing a lit ring of stones. `blank`: a lit ground, one box, no player. `2d`: sprites under an orthographic camera, one steered by a script |
| `--engine path\|subtree\|none` | Where the engine's schemas come from. `path` (default): a Flint checkout on this machine, named by absolute path in `flint.toml`; the printed commands work at once. `subtree`: `engine/schemas`, the game-repo pattern; `flint init` runs no git, so it prints the `git subtree add`, and commands stop with `schema directory not found` until it has run. `none`: only the game's own `schemas/`; scenes validate and render, engine archetypes resolve to nothing, and `flint play` is not offered |
| `--engine-path <dir>` | With `--engine path`: the checkout to name (default: the one this `flint` was built from) |
| `--force` | Write into a directory that already holds files. A file already there is kept; nothing is overwritten or deleted |

No engine schema is copied into the game. One engine file is: with `--engine path` or `none` the project carries `scripts/lib/menu.rhai`, the engine's menu helpers, which the HUD imports, because a script's `import` searches the project and `engine/` but not a checkout named by path. With `--engine subtree` the engine's own is found and nothing is copied. See [Your First Project](../getting-started/first-project.md) and [Building a Game Project](../guides/building-a-game-project.md).

## The `package` Command

```bash
flint package list            # every package: tier, version, what it owns
flint package list --json     # the same as one JSON document
flint package list --dir d    # the manifests under another folder
flint package list --built    # only the packages this flint was compiled with
flint package used            # the packages the project's scenes and scripts use
flint package used --write    # ...and add them to [packages] in flint.toml
```

A package is a name a game will list in its `flint.toml`, and everything that name brings ([ADR 0136](https://github.com/chrischaps/Flint/blob/main/docs/adr/0136-the-package-model.md)). Each has a manifest, `packages/<name>/package.toml`, that says what it owns, so a tool or an agent can ask what exists without compiling anything. There are seven: `engine` (the kernel, holding for now the standard parts that have no switch yet: physics, audio, animation, particles, terrain, and the ocean, grass, stylize and panels features), the standard `procgen` and `debug-hud`, and the features `living`, `music`, `rhythm` and `ai`. A game lists the ones it uses under `[packages]` in its `flint.toml`, and `flint package used` says which those are ([Packages a game uses](../guides/building-a-game-project.md#packages-a-game-uses)). The [Packages](../concepts/packages.md) chapter is the overview.

```text
living     feature   0.1.0  Props that hold and glide, and the wooze field
           engine     ^0.1
           requires   procgen
           features   living
           crates     flint-living, flint-wooze (2)
           components living_prop, shallow_wooze (2)
           tables     [living], [wooze] (2)
           modules    living, wooze, debug (3)
```

`engine` is the engine API range a package builds against, `requires` the packages that come with it, and `features` the Cargo features of `flint-player` and `flint-cli` that build it. The folder read is the project's engine: `engine/packages` in a game that carries the engine as a subtree, `packages/` in an engine checkout, else the tree this `flint` was built from.

Under `--json` it is one document: `dir`, `valid`, `packages` (each with `name`, `tier`, `version`, `summary`, `engine`, `api`, `requires`, `features`, `file`, and the lists `crates`, `components`, `archetypes`, `scene_tables`, `script_modules`, `commands` under the manifest's own keys) and `problems`. A manifest with a mistake is left out of the list and named among the problems as a `bad-package` diagnostic with its file and key, and the command exits 1:

```text
living/package.toml: error[bad-package] field 'package.tier': `feture` is no tier (the tiers are kernel, standard, feature, game) (did you mean `feature`?)
```

So is an item two manifests provide, a requirement naming no package, and a package in a folder of another name.

A manifest:

```toml
[package]
name = "living"            # the key a game lists: [a-z][a-z0-9-]*
version = "0.1.0"          # semver
tier = "feature"           # kernel | standard | feature | game
summary = "Props that hold and glide, and the wooze field"
engine = "^0.1"            # the engine API range; the kernel states `api` instead

[requires]
procgen = "*"

[build]
features = ["living"]

[provides]
crates = ["flint-living", "flint-wooze"]
components = ["living_prop", "shallow_wooze"]
scene_tables = ["living", "wooze"]
script_modules = ["living", "wooze", "debug"]
commands = []              # paths below `flint`: "music chart", "asset generate"
```

Every crate, schema file, script module and command of the engine belongs to exactly one package. A test holds that: a new component, crate, script module or command fails it, naming the file or item, until one manifest lists it.

### Which packages a `flint` has

`flint-cli` has a Cargo feature for each package with a switch: `living`, `music`, `rhythm`, `procgen` and `ai` (and `packages`, all five). They are on by default; a build without one links none of that package's crates. Its commands stay in `flint --help`, so every build parses the same command lines, but running one exits 2 and names the package:

```text
$ flint music chart play x
Error: flint music chart play needs the `rhythm` package, and this flint was built without it: list `rhythm = "*"` under [packages] in flint.toml and run `flint build` (`flint package list --built` names the packages this one has)
```

The same goes for a flag that needs one: `gen --ai-create` and the other AI steps, `render --ai-refine` and `asset resolve --strategy ai_generate` need `ai`; `gen --strip` and a `render --sweep` of `wooze.*` or `living.echoes.*` need `living`; `music validate --chart` needs `rhythm`. Under `--json` the error document carries `package`. `flint package list --built` lists what this binary has, each package whose `[build] features` it was compiled with. A `flint-cli --no-default-features` build has only the kernel and the debug panels:

```text
$ flint package list --built
engine     kernel    0.1.0  The kernel, with the standard packages and render features that have no switch yet
debug-hud  standard  0.1.0  The F3/F4 debug panels and their shared widgets
2 of 7 packages built into this flint (features: debug-hud, frame-history)
```

Under `--json` it is the list's document with `built: true` and `features`, the Cargo features the binary has. A build without `living` still renders a scene with living props: everything else in it draws as before, and the props, whose models are `[living]` specs only that package grows, are left out, with one warning at the load. `flint check` still reads the scene's `[living]` table.

## The `build` Command

```bash
flint build                        # a debug flint-player and flint with the project's packages
flint build --release              # optimized
flint build --dry-run              # the plan, the cargo command and where the binaries would land
flint build --engine-path ../Flint # another engine workspace
```

It reads `[packages]` against the engine's manifests and runs `cargo build --no-default-features --features ...` in the engine workspace (`--engine-path`, else the project's `engine/`, else the engine its schemas name, else the tree this `flint` was built from), keeping each crate's build options (`audio`, `gamepad`, `frame-history`, `dialogs`). `--target <triple>` builds for another platform. A project that drops `debug-hud` builds in two cargo runs, since `flint-cli` turns that feature on in the player. On Windows each output binary that exists is renamed to `<name>.old.exe` first, so a running `flint.exe` or `flint-player.exe` doesn't stop the build. The first build of a game is `cargo run -p flint-cli -- build` from its `engine/` folder. The guide's [Building the game](../guides/building-a-game-project.md#building-the-game-flint-build) has the output, the JSON and what `flint play`, `flint render` and `flint check` say when a binary lacks a listed package.

## The `check` Command

```bash
flint check                          # the project in the working directory
flint check flint-examples/          # every scene under a folder
flint check scenes/dock.scene.toml   # one scene, and the files it names
flint check . --json                 # one JSON document on stdout
```

`flint check` is the gate to run before playing, rendering or bundling. It reads the project and reports **every** problem it finds in one run; it never stops at the first. Each problem is a [diagnostic](../concepts/schemas.md#scene-diagnostics): a severity, a code, the file, the entity and the field when there is one, a message, a hint, and for a missing file every path the engine tried.

```
$ flint check
scenes/dock.scene.toml: 4 errors, 1 warning
  error[unknown-component] entity 'buoy' component 'lihgt': no component named `lihgt`, so nothing checks it
      hint: a component a game adds is declared in its schema layer, as components/lihgt.toml
  error[unknown-archetype] entity 'crate' field 'archetype': no archetype named `porp`, so the entity gets no defaults
      hint: did you mean `prop`?
  error[unknown-entity] entity 'gate' joint.parent: no entity named `frame`
      hint: it names an entity of this scene (a prefab's are `<prefix>_<name>`); leave it "" when a script fills it at run time
  error[missing-asset] entity 'boat' model.asset: model `boat` is not found (also named by 'spare')
      hint: `flint asset where models boat --scene scenes/dock.scene.toml` shows the search; the asset catalog and a procgen spec named `boat` were asked first
      tried:
        scenes/models/boat.glb
        scenes/models/boat.gltf
        models/boat.glb
        models/boat.gltf
  warning[constraint] entity 'lantern': Lamp 'lantern' is too bright (constraint `lamps_are_not_blinding`)

schemas/constraints/rules.toml: 1 error
  error[bad-constraint] field 'query': constraint `lit_props_have_a_place`: `entities where archetype == 'prop' and lamp.brightness > 0` can't be evaluated: Parse error: ...
      hint: a query is one resource and at most one condition: `entities where archetype == 'door'`; the constraint is not checked until it parses

scenes/torn.scene.toml: 1 error
  error[toml-parse]: TOML parse error at line 4, column 15

checked 2 scenes and 3 files: 6 errors, 1 warning
```

| What it checks | Codes |
|---|---|
| Each scene reads, parses and expands its prefabs | `file-unreadable` `toml-parse` `bad-prefab` |
| Archetypes, components and fields against the schemas | `unknown-archetype` `unknown-component` `unknown-field` `bad-type` `bad-enum-value` `out-of-range` `missing-field` `no-schemas` |
| References: every `parent`, every field a schema types `entity` (`joint.parent`, `ik_two_bone.target` / `pole` / `tip`, a game's own) or `asset`; an `ik_two_bone.tip` must be a child of the entity naming it; no entity's parents may loop | `unknown-entity` `missing-asset` `not-a-child` `parent-cycle` |
| The scene builds | `scene-load` |
| Every inline `particle_emitter` against the emitter's own reader: its keys, and that each value parses and resolves | `bad-emitter` |
| The assets a scene names by kind, through the [one search](../concepts/assets.md): `model.asset`, `material.texture`, `sprite.texture`, `audio_source.file`, `script.source`, `terrain.heightmap` / `splat_map`, `grass_ground.mask`, `spline.file`, `particle_emitter.texture`, `[environment] skybox`, `[scene] input_config` | `missing-asset` |
| The procgen spec behind a model or texture parses | `bad-spec` |
| Every UI document (`.ui.toml`) and its style file, as `load_ui` reads them: types, anchors, keys, parents, classes, style properties and their values | `bad-ui` |
| That spec's params fit its generator's schema, at every depth (`flint_procgen::validate_params`, what `flint gen` checks before generating; also `flint validate`) | `spec-param` |
| The spline file behind a `spline.file` parses, its mistake placed by line and column | `bad-spline` |
| The input config (the scene's, else `[project] input`, else `config/input.toml`) parses | `bad-input-config` |
| Every other file a scene, prefab, particle effect, glyph set, chunk, user style or script names by path | `missing-file` |
| User styles load and compile; `[post_process.styles]`; the look | `bad-user-style` `bad-style-setting` `bad-look` |
| Every look file (`*.look.toml`) the project holds reads, named by a scene or not, and each of its tables for a style that isn't built in names a style some folder that can take the look has, with keys that style takes | `bad-look` `bad-style-setting` |
| `[wooze]` and `[living.species]` | `bad-wooze` `bad-species` |
| Each field a scene declares, `[field.<name>]`: its keys and its zones | `bad-field` |
| Constraints, and that each one can be evaluated | `constraint` `bad-constraint` |
| `flint.toml` reads, its schema directories exist, its start scene is there | `bad-manifest` `bad-schemas` `missing-asset` |
| A folder's `Cargo.toml` (a game with a Rust binary) names every `[patch.crates-io]` line of the engine it builds by path, since cargo applies only the root workspace's ([Upgrading the Engine](../guides/upgrading-the-engine.md#cargo)) | `missing-cargo-patch` |
| That workspace's `Cargo.lock` gives gpu-allocator the `windows` crate wgpu-hal has: a lock moved forward from an engine before wgpu 30 can keep an older one, and wgpu-hal doesn't compile on Windows ([Upgrading the Engine](../guides/upgrading-the-engine.md#cargo)) | `stale-cargo-lock` |
| Every script compiles, and its calls, callbacks and imports fit the script API ([`flint script check`](../concepts/scripting.md#checking-your-scripts)) | `script-syntax` `unknown-function` `wrong-arity` `bad-argument` `unknown-callback` `callback-arity` `missing-module` `deprecated-function` `mixed-numbers` `unknown-global` |

A model or texture name counts as found when the asset catalog holds it, when a procgen spec the scene can see is named that, or when the file search finds it, in that order, as at run time. A `model` with `runtime = true` has no file to find. A script is checked for the files its string literals name, and held to the script API as `flint script check` holds it; a script problem has a line and a column (`error[wrong-arity] 82:9: ...`).

For a folder, every `*.scene.toml` under it is checked, and every prefab, particle effect, glyph set, chunk, music suite, user style and script there is read for the files it names, and every glTF model for the textures and buffers it keeps beside it (a model missing one is skipped whole by the player). `target/`, `dist/`, dot-folders, `docs/` and an `engine/` subtree are skipped (the folders `flint bundle` never packs); what a game's `bundle.toml` excludes from the web build is still checked, since it is still part of the game on disk. For one scene, the scene and the scripts and prefabs it names are checked.

| Flag | Description |
|------|-------------|
| `--json` | Print one JSON document on stdout and nothing else: `valid`, `summary`, `errors`, `warnings`, `scenes`, `files` and `diagnostics`, each diagnostic with `severity`, `code`, `file`, `entity`, `component`, `field`, `message`, `hint` and `tried` |
| `--strict` | A field no schema declares is an error, not a warning (as `[project] strict = true` does for every run) |
| `--schemas <dir>` | Schema directories, repeatable; without it the project's `flint.toml` names them |
| `--allow <name>` | A script function the game's binary registers (an extension's): calls of it pass the script check. Repeatable. `flint validate` takes it too |
| `--api <file>` | The game binary's own script API listing, in `flint script api --json`'s shape: its functions pass at their argument counts. Repeatable. `flint validate` takes it too |

The exit code is 1 when there is an error and 0 otherwise. A warning does not fail it.

`flint bundle` runs the same code over the files it is about to pack, and `flint validate` over one scene, so the three never disagree about what a problem is. They differ in what they ask: `flint bundle` asks whether the bundle is whole (every scene loads, every file named is packed, styles and looks load), `flint validate` asks about one scene's schemas and constraints and can `--fix`, and `flint check` asks everything. `flint bundle` leaves the scripts' calls alone; `flint validate` checks the scene's scripts.

## The `frame` Command

```bash
flint render levels/dock.scene.toml --frame-capture dock.flintframe   # make one
flint frame show dock.flintframe                    # the summary
flint frame show dock.flintframe --passes           # every pass, ran or why not
flint frame show dock.flintframe --draws --entity "barrel_*" --sort tris
flint frame show dock.flintframe --targets          # every texture, its memory, who wrote it
flint frame show dock.flintframe --hitch            # the frame history around it, when it has one
flint frame diff before.flintframe after.flintframe --out diffs/
flint frame image dock.flintframe Scene -o scene.png              # HDR, tonemapped
flint frame image dock.flintframe Scene --target depth -o depth.png
flint frame image dock.flintframe --all -o dock-images/
flint frame open dock.flintframe                    # the frame debugger's window
flint frame open dock.flintframe --pass Scene --screenshot scene.png   # one frame of it, no window
flint frame replay dock.flintframe                  # draw it again: byte-identical, or how far off
flint frame replay dock.flintframe --draws 40 --hide barrel_02 --out step.flintframe
flint frame replay dock.flintframe --draw-times --overdraw --out cost.flintframe   # each draw's GPU time, the overdraw heat map
```

`flint frame` reads a frame capture: the folder `flint render --frame-capture` writes, with a `manifest.json` describing the frame and an image for each target each pass wrote ([A frame's images](../guides/profiling.md#a-frames-images)). Every view but `replay` reads only that folder: no GPU, no scene, no schemas. So a capture from another machine, or attached to an issue, reads the same as one made here. A `--frame-dump` JSON file reads too, with no images. `replay` draws the frame again from the capture's `replay/`, so it needs the scene and its assets and a GPU.

### `flint frame show`

With no view it prints the summary:

```text
Capture  tests/fixtures/frames/two_crates.flintframe  (flintframe v1, written by flint render at 2026-11-17T01:15:46Z, engine 0.1.0)
Scene    Crate and Lantern  (tests/fixtures/frames/two_crates.scene.toml)
Adapter  NVIDIA GeForce RTX 5070 Laptop GPU (DiscreteGpu, Vulkan) (headless)
Frame    64x36, scene 64x36 (scale 1.0), msaa 1, Rgba8UnormSrgb
Time     GPU 0.069 ms (holds the capture's copies)
Draws    3 draws, 36 triangles; shadows 9 draws, 108 triangles
Passes   8 of 17 ran, 5 timed
Memory   311.1 KB in 10 targets
Images   11 images, 12.2 KB on disk
Lights   1 drawn

Most expensive passes (GPU ms):
   #  pass       gpu ms  draws  triangles
  12  Bloom       0.024      0          0
  10  SSAO        0.021      0          0
  01  Shadows     0.012      9        108
  05  Scene       0.007      3         36
  13  Composite   0.006      0          0
```

`#` is the pass's place in the frame, the `NN` of its image files. `--top N` lists more or fewer passes. A pass's draws include its layers' (the shadow cascades under `Shadows`). A capture written since #1176 also has a `Camera` line after `Frame`: the camera the frame drew with, from the manifest's `camera` (a comic panel's is under its panel), such as `Camera   perspective, fov 50°, near 0.1 m, far 1000 m, at (3.4, 1.7, 9) looking at (0.7, 0.7, 4.2)`. The frozen fixture above was written before it and has none.

| View | What it lists |
|------|---------------|
| `--passes` | Every pass in the order the frame recorded it, a shadow map's layers indented under it: its GPU ms (`untimed` for a part the timer has no scope for), draws, triangles, and what it wrote, or `skipped:` and why |
| `--draws` | Every draw, named by its entity: the pass, kind, layer, pipeline, mesh, triangles and instances. `--pass <name>` keeps one pass's, `--entity <name>` one entity's (`*` matches any run: `tree_*`), `--sort tris` puts the heaviest first, `--sort gpu` the costliest on the GPU (a replay's `--draw-times`, which also adds a `GPU ms` column), `--culled` adds the draws a cull set aside with the reason (`frustum`, `lod`, `draw distance`) |
| `--targets` | Every texture the frame drew into or read: format, size, layers, samples, memory, its images and the passes that wrote it |
| `--hitch` | The player's frame history around the capture and the hitch that asked for it ([Hitches](../guides/profiling.md#hitches)), when the capture holds one. A `flint render` capture has none, and says so |

Views combine (`--passes --targets`). An entity that draws nothing, or a pass that doesn't exist, is an error with the nearest name (`did you mean `lantern`?`), and an entity a cull set aside says to add `--culled`.

### `flint frame diff`

`flint frame diff <a> <b>` says what changed between two captures, pairing passes by name (the second `Bloom` with the second), entities by name (never by id, which moves between runs) and targets by id:

```text
Frame
  draws        3 -> 2 (-1)
  triangles    36 -> 24 (-12)
  shadow draws 9 -> 6 (-3)

Draws by entity
  removed  lantern in Shadow cascade 0: 1 draw: shadow
  removed  lantern in Shadow cascade 1: 1 draw: shadow
  removed  lantern in Shadow cascade 2: 1 draw: shadow
  removed  lantern in Scene: 1 draw: pbr

Images (5 of 11 the same)
  image                                     differing     of     max  diff
  Scene / hdr_scene                         78 (3.4%)   2304  0.2678  diffs/scene_hdr_scene.diff.png
  ...
  Composite / surface                      172 (7.5%)   2304  0.3569  diffs/composite_surface.diff.png
```

That is the frozen pair in `tests/fixtures/frames/`, a crate and a lantern and then the crate alone, with the passes and targets sections (unchanged) left out.

- **Frame**: the GPU time, draws, triangles, culls and target memory, each a -> b.
- **Passes**: those added, removed or moved, those that switched between running and skipped, and each pass whose GPU time, draws or triangles changed.
- **Draws by entity**: each entity whose draws appeared or vanished in a pass, or changed pipeline or number.
- **Targets**: added, removed, or changed in format, size or memory.
- **Images**: for each pass image in both, how many pixels differ and by how much at most (in the image's own units, 8- and 16-bit steps normalized to 1). `--tolerance X` lets float values lie X apart and still be the same. `--out <dir>` writes a picture of each image that differs: the after image dimmed to grey, each changed pixel lit from amber to white by how far it moved.

The diff is a report, so it exits 0 whatever it finds.

### `flint frame image`

`flint frame image <capture> <pass> -o out.png` copies a pass's image out as a PNG you can look at. An agent can read that PNG; an EXR it can't. A capture keeps each target exactly as the GPU held it, so the viewable copy is made here:

| Stored | Written as |
|--------|------------|
| 8-bit PNG | the file, byte for byte |
| float colour (EXR half) | exposed at the frame's `exposure`, tonemapped with the composite's ACES curve, sRGB-encoded |
| the scene's depth (EXR `Z`), in a capture that names its camera | metres from the camera, through the manifest's `camera` near and far planes, on a log scale from the nearest drawn (white) to the farthest (dark); the clear value black. The `view` line names the range: `depth 2.67..86.0 m, log scale, near white` |
| depth in a capture that names no camera (written before #1176), and a shadow map's layer (a light's view, linear already) | grey stretched over the stored depths the frame drew, near white, the clear value black: `depth 0.96265..0.99894 stretched` |
| one or two float channels | grey, or red and green, as stored in 0..1; a measure past 1 (a distance in metres) on a log scale, so a far sky doesn't black out the rest; centred on mid-grey when a value is negative |
| integer ids (the style mask, a u32 target) | a colour each, spaced round the hue circle; id 0 black |

The pass is named as `--passes` lists it (`Scene`, `Bloom`, `Shadow cascade 1`, `Bloom (2)` for the second pass of that name), or by an image's file name (`05-hdr_scene.exr`). A pass that writes several images gives its first. `--target <id>` picks another (`--target depth`), and `--step <step>` picks one of bloom's `threshold`, `down` and `up`. `--raw` copies the stored file (an EXR stays an EXR). `--all -o <dir>` writes every image of the capture.

### `flint frame open`

`flint frame open <capture>` is the frame debugger: a window with the passes as a tree, the selected pass's image (zoom, pan, a texel readout, channel, exposure and depth range, an A/B wipe against the pass before or the finished frame), its draws as a table linked by entity across passes, and the frame's CPU phases, GPU passes and frame history along the bottom ([The Frame Debugger](../guides/frame-debugger.md)).

| Flag | What it does |
|------|--------------|
| `--pass <name>` | Open on this pass (`Scene`, `Bloom (2)`); else the finished frame. A name the capture lacks is refused before any window, naming the nearest |
| `--target <id>` | Open on this of the pass's images (`depth`) |
| `--entity <name>` | Select this entity (any part of its name): its draws are marked in every pass |
| `--wipe previous`, `--wipe final` | Open with an A/B wipe against the previous pass's image or the finished frame |
| `--screenshot <png>` | Draw one frame of the window into a PNG and exit, with no window; `--json` answers `{"written": [path]}`. Without it the command is a window and refuses `--json` |
| `--width`, `--height` | The window's size in points (1600 x 1000); a screenshot's in pixels |
| `--scale <x>` | A screenshot's pixels per point |
| `--replay` | Draw the frame again as it opens (R in the window): needs the capture's `replay/`, the scene and a GPU |
| `--draws <n>`, `--hide <name>`, `--skip <pass>`, `--outline <name>` | Replay it with a pass stopped after N draws (the `--pass` shown when it is a shadow layer, `Style mask` or `Style albedo`, else the scene pass), without an entity, without a pass, with an entity's pixels outlined (each implies `--replay`; `--hide` and `--skip` repeat). A screenshot waits for the replay |
| `--scene <file>`, `--schemas <dir>` | The replay's scene, when it isn't where the capture says, and its schema directories |

### `flint frame replay`

`flint frame replay <capture>` draws a captured frame again ([Replaying a frame](../guides/frame-debugger.md#replaying-a-frame)): the authored scene loaded as `flint render` loads it, the captured world laid over it by entity name, the renderer's state put back, no game step run. It writes the replayed frame as a capture, which every other `frame` view reads, and says whether its final image and its scene pass's image are byte-identical with the captured ones, or in how many pixels they differ and what the capture draws fresh. A style's history the capture holds (#1136) is handed back before the draw, and a `history:` line says, for each part, whether the style took it or drew fresh and why. Each asset is checked against its recorded hash; a missing or changed one is named and the frame still drawn. A capture with no `replay/` is refused, saying so. `--json` carries `identical`, `final` and `scene` (`pixels`, `differing`), `pass` (the stepped pass's own images, summed, when it isn't the scene's) and `pass_draws` (how many draws the capture's stepped pass has), `assets` (each `same`, `moved`, `changed` or `missing`), `notes`, `fresh_only`, `history` (those lines), `refused` and `skippable`; with `--draw-times`, `draw_times` (`timed`, `drawn`, `frames`, `costliest`, `by_kind`), and with `--overdraw`, `overdraw` (`pixels`, `empty`, `max`, `max_at`, `mean`, `over_4`, the two images' files).

| Flag | What it does |
|------|--------------|
| `--out <dir>` | Where the replay goes (default: beside the capture, `<name>.replay.flintframe`) |
| `--draws <n>` | Stop a pass after its first N draws, counted as `flint frame show --draws --pass <name>` lists them: the scene pass, or the one `--pass` names |
| `--pass <name>` | The pass `--draws` steps (#1204): `Scene` (the default), `Shadow cascade 0` to `2`, `Spot shadow`, `Style mask` or `Style albedo`. Every other pass draws whole; a shadow layer's grass isn't counted. Another name is refused before the scene loads, naming these |
| `--hide <name>` | Draw the frame without this entity in any pass (repeatable) |
| `--skip <pass>` | Draw the frame without this pass: `Shadows`, `SSAO`, `Volumetric`, `Bloom`, `FXAA`, `Fog veil`, or a style's (repeatable). The text answer lists what this capture takes |
| `--outline <name>` | Mark this entity's pixels with the selection outline |
| `--draw-times` | Time each scene pass draw on the GPU (#1203): a timestamp before and after it inside the pass, in frames drawn before the captured one (2,048 draws a frame). Each draw's `gpu_ms` is in the replay's manifest, and the answer lists the costliest and the cost by kind. Needs an adapter with timestamps inside passes; without them it says so in `refused` |
| `--overdraw` | Add the `Overdraw` pass after the scene's (#1203): how many draws shade each pixel (`overdraw_count`) and its heat map (`overdraw`; each colour twice the draws of the last, 1 to 31+). The answer gives the mean, the most and where, and the share over 4. Neither flag changes the frame's pixels |
| `--scene <file>` | The scene, when it isn't where the capture says (found as recorded, else under the capture folder's parents) |
| `--schemas <dir>` | Schema directories, as `flint render` takes them |

### Capture versions

A manifest carries an integer `version` (ADR 0126). `flint frame` reads its own version and every older one. A field it doesn't know is ignored, and one an older capture lacks is defaulted. A newer capture is refused, naming both versions: `this is a version 2 capture, and this flint reads version 1 and older: read it with a newer engine`.

## The `profile` Command

```bash
flint-player flint-examples/hitch_metronome.scene.toml --trace-on-hitch   # make one (traces/)
flint profile explain traces/hitch_metronome-hitch-t3.000.json      # why the worst frame was slow
flint profile explain out.json hitch                 # the hitch it was saved for, else the worst
flint profile frame out.json 412 --min-ms 0.2        # that frame's scope tree
flint profile top out.json --sort self --thread main --frames 400..500
flint profile diff before.json after.json            # what moved, and what the spread explains
```

`flint profile` reads a scope trace: the Chrome trace-event JSON that `flint-player --trace`, `--trace-on-hitch`, Shift+F3 and a frame capture's `trace.json` hold ([Recording a trace](../guides/profiling.md#recording-a-trace)). It reads only that file: no GPU, no scene, no schemas. A frame is named by the trace's own frame index, the `frame` each scope carries, which Perfetto shows in its args; `worst` is the longest frame, and `hitch` the hitch the file means: for a `--trace-on-hitch` file the one it was saved for (the hitch at the game time its `reason` names, else the last on the `hitches` track), for any other the worst the detector found. The profiler tab of `flint frame open` opens a trace on the same frame. Every view answers `--json` with one document carrying the numbers its text shows. A trace of a newer version than the reader knows is refused, naming both.

### `flint profile explain`

```text
frame 525 took 107.28 ms (median 4.57): stall.rhai:on_update 100.09 ms against its median 0.032, +100.06 of the frame's +102.71 ms excess
  inside tick > run_scripts > script::update > script::on_update > script::call (flint-examples/scripts/stall.rhai) on main
  the hitch detector: script stall.rhai:on_update 100.1 ms (judged 108.33 ms against 4.17)
```

The first lines are the answer. The frame (the worst, unless one is named) is held against the trace's **normal frames**: every frame but the look-back's first and last (whose scopes may be cut), the hitches the detector found, and any over twice the median frame. Each scope's time in the frame is compared with its median over those, a frame it didn't run in counting 0. The answer is found by walking down the frame thread's tree from the frame's excess: at each level the walk goes into the child with the largest excess while that child carries at least half its parent's, so it stops at the deepest scope that still explains the frame, not at `tick`, which every excess is inside.

- A scope that ran in no normal frame says so: `import_model (models/whale.glb) 30.00 ms ran only here`.
- When no outermost scope carries the excess, the time was outside any scope, and the phases say where: `no scope covers the +40.00 ms excess; the acquire phase took 40.50 ms against its median 0.50`, with `a wait, not work` for `acquire` and `present`.
- When the scope found spreads its excess over children too small to carry half each, the answer says so and names `flint profile frame` to see them.

Below the answer: the phases against their medians, a table of scopes by **self excess** (the time in a scope less its children's, over its median, so the culprit is the first row and its ancestors follow), with `ran in` counting the normal frames each ran in, the scopes that ran in no normal frame, and the frame's notes. A scope with text detail is counted by its detail too (`script::call (scripts/stall.rhai)`), so one scope at many files reads as many.

### `flint profile frame`

```text
main: 103.90 ms in its outermost scopes
  scope                                                   ms  share  self ms  calls
  tick                                                101.15  97.2%    0.082      1
    run_scripts                                       100.47  96.6%    0.056      1
      script::update                                  100.41  96.5%    0.006      1
        script::on_update                             100.18  96.3%    0.011      1
          script::call (flint-examples/scripts/stall.rhai)  100.07  96.2%    0.008      1
            stall.rhai:on_update                      100.06  96.2%   100.06      1
    (16 more under the threshold)                       0.34   0.3%
```

One frame's scopes, thread by thread, indented, each with its ms, its share of the frame and its self time. Siblings of one name under one parent are one row with their call count. `--min-ms` (default 0.05) leaves short scopes out, and each parent says how many it left out and their time. Then the frame's phases, its GPU passes and its notes. `--thread` keeps one thread.

### `flint profile top`

Every scope over the trace: calls, total, self, mean per frame (the total over every frame in range, so it reads against the frame's budget), the worst single call and the frame it was in. `--sort total|self|worst|calls` (default total), `--limit` (20), `--thread`, `--frames 400..500` (or `400..`, `..500`, one frame), and `--by-detail` to count a scope with text detail by its detail as well. A scope inside one of its own name counts once in a total.

### `flint profile diff`

```text
frame: mean 3.79 -> 16.43 ms (+12.64), p90 3.79 -> 41.29 (+37.50), over 14 and 14 frames
  the frame time's delta is inside its own spread (+-15.14 ms, 2 standard errors): run both again before believing it

ms a frame, by the size of the mean's delta:
  scope      a mean  b mean  delta  a p90  b p90  delta p90
  encode_to    0.40    0.90  +0.50   0.40   0.90      +0.50
  render       0.80    1.30  +0.50   0.80   1.30      +0.50

  10 scopes moved less than 2 standard errors of their frames' spread: --all lists them, marked ~
```

(The tests' two hand-made traces: `b` has three hitches in sixteen frames, which make its mean frame four times `a`'s and are still inside the spread, while `encode_to`'s steady half millisecond is not.)

Each scope's time a frame in two traces (every frame but each trace's first and last, a frame it didn't run in counting 0): its mean and p90 in each, and the deltas, largest first. Two runs of one build never match exactly, so a delta under twice the standard error of the two means (the frames' own spread) is left out and counted, and `--all` lists it marked `~`; a scope in one trace only says `only in a` or `only in b`. The frame time's own line says when its delta is inside its spread: `run both again before believing it`. A hitch in one trace is a large mean over few frames, and usually inside the spread for that reason. A note says when the two traces' scenes, builds, adapters or backends differ.

## The `play` Command

Launch a scene as an interactive first-person experience with physics:

```bash
flint play flint-examples/phase4_runtime.scene.toml
flint play levels/tavern.scene.toml --schemas schemas --fullscreen
# in a game whose flint.toml names `scene = "scenes/main.scene.toml"`: no argument
flint play
# a small companion window in a corner that stays up and leaves the keyboard alone
flint play scenes/body.scene.toml --width 480 --height 480 --position 1400,560   --borderless --always-on-top --no-activate
```

| Flag | Description |
|------|-------------|
| `--schemas <path>` | Schema directory (repeatable; later paths override earlier). Without it: the project's `flint.toml` `schemas`, else `schemas/` in the working directory, beside the scene and one level up ([Common Flags](#common-flags)) |
| `--fullscreen` | Launch in fullscreen mode |
| `--width <px>` / `--height <px>` | Initial window size in physical pixels (default `1280`x`720`; ignored with `--fullscreen`). A narrow or square window keeps the camera's aspect: it shows less to the sides, never a stretched scene |
| `--position <x,y>` | Put the window's outer top-left corner here, in physical desktop pixels. Negative values reach a monitor left of or above the main one; write them with `=` (`--position=-1920,0`). Default: wherever the system puts it |
| `--borderless` | No title bar or frame. Esc still exits (unless the game binds it) and Alt+F4 still closes it |
| `--always-on-top` | Keep the window above other windows |
| `--no-activate` | Open without taking the keyboard from the window that has it, for a companion window beside a terminal. Clicking the window still focuses it. Not with `--fullscreen`, whose path takes the foreground on Windows |
| `--input-config <path>` | Input config overlay path (highest priority, overrides all other layers) |
| `--save-dir <dir>` | Keep this run's save data in `<dir>`, as `<dir>/<slot>.toml`, and read no other save. Without it: `<config>/<game id>/saves/` under the OS user directory, or the project's `save/` for a game with no id (see [Where saves live](../concepts/scripting.md#where-saves-live)). Also on `flint-player` |
| `--music-volume <f32>` | Initial gain for the `music` mixer bus |
| `--sfx-volume <f32>` | Initial gain for the `sfx` mixer bus |
| `--gpu <name>` | GPU adapter to use (case-insensitive substring, e.g. `NVIDIA`). See [GPU Selection](#gpu-selection) |
| `--list-gpus` | Print available GPU adapters and exit |
| `--list-keys` | Print the debug keys after the scene's input layers (each key, action, owner, what it does, when it acts, and any the game's bindings take) and exit. F1 shows the same in a running game ([Debug Keys](../concepts/physics-and-runtime.md#debug-keys)) |
| `--json` | With `--list-keys`: the table as JSON, for agents. Under `flint play` this is flint's global `--json`, so it may come before `play` too, and without `--list-keys` it is refused as for any window command; `flint-player` takes its own, only with `--list-keys` |
| `--style <name[:k=v,...]>` | Turn a registered style on over the scene's post-processing, as `flint render --style` does: `ascii`, `ascii:cell=12,ramp=blocks`, `ascii:off` (off everywhere, entities' own included, #376). Repeatable; a later word wins. Written over every scene again after a transition, so it survives them (#328) |
| `--style-time <s>` | Hold the post time animated styles see (shared with the ocean, grain, the tear and the reality styles) at this many seconds |
| `--motion-hold <rate>` | Motion on twos for the whole frame (#405): every entity's drawn position and pose, and the post time styles read, change only this many times a second (12 = on twos at 24 fps). Over every scene's `[post_process] motion_hold`, kept through transitions. See [Motion on twos](../concepts/stylize-passes.md#motion-on-twos) |
| `--expression-timeline <file>` | Play expression states from a JSON timeline on the game clock, for scripts' `expression()` and `on_expression` (see [Expression API](../concepts/scripting.md#expression-api)) |
| `--expression-listen <ip:port>` | Take expression states as newline-delimited JSON over TCP at a loopback address, e.g. `127.0.0.1:7777`. Off unless given. It opens once the first frame is presented (until then a connect is refused) and says so in one `listen` line ([Expression API](../concepts/scripting.md#expression-api), #584) |
| `--log-format <text\|json>` | How the player writes what it says, to stderr: `text` (default) or `json`, one object a line with `timestamp`, `level`, `target`, `message` and the event's fields (#716) |
| `--quiet` | Say only warnings, errors and what a flag asked for (`--perf`'s lines, the `--exit-after` report, `--inspect-listen`'s and `--expression-listen`'s opening): no load summary, controls line or transitions |
| `--inspect-listen <ip:port>` | Answer questions about the live world at a loopback address, e.g. `127.0.0.1:7878`: `flint query --attach` and `flint entity show --attach` ask it. Off unless given ([Asking a running game](#asking-a-running-game)) |
| `--dump-world <file>` | Write the world as JSON at `--dump-world-at` times, each entity `--dump-query` picks with its components' values ([Asking a running game](#asking-a-running-game)) |
| `--dump-world-at <secs>` | Game times for `--dump-world`, repeatable or comma separated (`1,2.5`), each taken on the first frame at or after it. Default: `--exit-after`'s time, else the first frame |
| `--dump-query <query>` | Which entities `--dump-world` writes, in [query](../concepts/queries.md) grammar. Default `entities`, all of them |

`--music-volume 0` is the quickest way to audition a scene's sound design
without its score. See [Audio: Mixer Buses](../concepts/audio.md#mixer-buses).

### What the player says

Everything the player says goes through one log on stderr (#716); stdout is
left for what a flag asks it to answer (`--list-gpus`). Each line has a
target naming what it is about: `load`, `transition`, `perf` (the line
`--perf` prints every 10 s), `report` (the exit report), `script`, `input`,
`music`, `player` (debug keys, looks, bundles), `inspect` (`--inspect-listen`,
`--dump-world`) and `listen` (`--expression-listen`'s socket opening). A scene coming up says
two lines, one for what it loaded and one for the controls it binds:

```text
INFO load: Loaded showcase_game/levels/tavern_hub.scene.toml: 36 entities, 1 model, 3 procgen assets, 15 scripts, 3 skeletal clips, 10 sounds in 6346 ms; no warnings
INFO input: Controls: interact E, jump Space, move_backward S, move_forward W, move_left A, move_right D, pause Escape, sprint LShift | mouse look, F1 debug keys
```

The `2d` template (`flint init x --template 2d`) has no character controller
and moves with WASD from its script, so it says
`Controls: move_backward S, move_forward W, move_left A, move_right D, toggle_help H | Esc cursor/exit, F1 debug keys`.
The player's other keys are the debug-key table's, which the key named
there lists over the game (F1 unless `[debug_keys]` moves it, #717).
The summary counts the engine's own warnings and quotes the first three; a
library's (the GPU driver's, a gamepad mapping) is shown as it happens and
left out of the count.

The controls are the bound actions the scene uses: those the game's input
config (and `--input-config`) names, those a loaded script spells as a
string (`is_action_pressed("move_left")`, `action == "jump"` in `on_action`,
and `interact` for a script with `on_interact`), and the character
controller's walk, jump and sprint when the scene has one. A built-in action
nothing uses (`fire` in a racing game) is left out. It is said again after a
transition that changes it. Each
file a load reads is a `debug` line of target `load`, counted into the
summary: `RUST_LOG=load=debug` lists them all again. `RUST_LOG` replaces the
filter whole, under either format; the default is
`warn,load=info,transition=info,perf=info,report=info,script=info,input=info,music=info,player=info,inspect=info,listen=info`.
An agent reading a run takes `--log-format json` and reads one object a line:

```bash
flint-player scenes/race.scene.toml --exit-after 5 --log-format json 2> run.jsonl
```

A game binary that embeds `PlayerApp` gets the same with
`flint_player::logging::init(&LogArgs::default())`, or adds
`flint_player::logging::tally_layer()` to a subscriber of its own for the
summary's counts.

The standalone `flint-player` binary is the same player: both flatten one
set of flags (`flint_player::PlayerArgs`) and launch through one function
(`flint_player::launch`), so their `--help` differs only by `flint-player`'s
`--bundle <file>`, which plays a game bundle made by `flint bundle` (#705).
Every flag below, the capture and quality ones included, works on either, and
a scene plays the same under both. Among them: `--msaa <1|4>` (default `1`)
for multisample anti-aliasing of the scene passes, `--render-scale` and
`--quality`, `--vsync off|mailbox|fifo`, `--frame-latency 1..3` and
`--no-pacing` for how frames reach the display (#1149: each over its old
variable, `FLINT_VSYNC`, `FLINT_FRAME_LATENCY` and `FLINT_PACING`, which are
read only when the flag isn't given), and `--procgen-defer`, which shows placeholders for a scene's
procgen assets at once and generates them over the following frames instead
of during load (see [Specs in Scenes](../concepts/procgen.md#specs-in-scenes)).

### Player Screenshots (HUD included)

`flint render` is headless and never runs scripts, so it cannot show a
script-drawn HUD. The player (`flint play` or the standalone `flint-player`)
can capture the frame it actually presents — scene *and* egui overlay — and exit on its own, which
makes it the tool for verifying `on_draw_ui` output:

```bash
flint-player scenes/race.scene.toml   --width 1280 --height 720   --screenshot renders/hud.png --screenshot-at 6 --exit-after 6.5
```

With `--screenshot`, `--exit-after` or `--input-script`, the run is a capture
and ignores the real keyboard, mouse and pads, so a stray scroll over its
window can't change the shot. Only the input script drives it (see
[input gates](../concepts/input-gates.md#during-a-capture)).

A run that asks for a screenshot either writes it or fails (#639). A read-back
or save that fails is warned about and tried again on the next frames (up to
30), and `--exit-after` waits for those retries. If the file still isn't on
disk when the run ends, the player prints `Error: --screenshot <path> was
never written: <why>` and exits with status 1. The reason can be the failure,
or that the run ended before `--screenshot-at`. With `--screenshot-every`, a
lost frame fails the run the same way. A player that fails to start (no GPU
adapter, no window) also exits 1 now, from `flint play` too.

| Flag | Description |
|------|-------------|
| `--screenshot <path>` | Write a PNG of the presented frame (scene + HUD). Parent directories are created |
| `--screenshot-at <secs>` | Game time at which to capture: the first frame whose clock reaches this value (default `0`, the first frame) |
| `--screenshot-every <secs>` | Keep capturing every N seconds after `--screenshot-at` until exit; files are numbered `hud_0001.png`, `hud_0002.png`, ... next to `--screenshot` |
| `--exit-after <secs>` | Exit once game time reaches this value, printing the average frame time and the average draw calls, entity draws, culled draws and triangles per frame (both skip the first 2 s) |
| `--frame-step <secs>` | Advance the clock exactly this much per frame, whatever the wall time. With `--screenshot-every` the same build on the same machine writes byte-identical frames run after run (grass included since #341). egui's clock steps with it, so a menu opening animates the same on every run. It seeds scripts' `random()` too (below) |
| `--seed <n>` | Seed scripts' `random()` and `random_range()`: one sequence, shared by every script in the order they run, the same every run. `--frame-step` seeds it on its own with the seed `flint render --scripts` uses, so a stepped player run and a scripted render draw the same numbers; `--seed` names another. Without either the sequence starts from the clock, once, so runs differ but no two draws repeat (#580). A script can reseed it itself with `seed_random(n)`. The web page takes `?seed=` |
| `--pixels-per-point <f32>` | Draw point-sized things (ASCII cells, egui panels, the HUD) at this scale instead of the window's, so a capture is the same on any display. `2` at 3840x2160 lays the frame out as 1920x1080 points |
| `--look <name\|path>` | Play every scene under a look, merged over its `[post_process]` (and over the look the scene names) as `flint render --look` merges it: a built-in (`flint render --look help` lists them) or a `styles/<name>.look.toml` |
| `--input-script <file>` | Play timed keys, typed text, axes, gamepad buttons, the wheel and the mouse, one event per line (below). Scripted keys reach egui and the debug keys too, so `2.0 tap F2` opens the stats overlay |
| `--stats` | Open the F2 rendering stats overlay from the first frame. F2 still closes it |
| `--perf` | Print a perf line every 10 s: frame p50/p99/max, each phase's CPU time (mean/max), each pass's GPU time where the adapter can time, draws and triangles. The same line the web page's `?perf=1` copies, so a desktop and a browser can be set side by side |
| `--load-report` | Print a table for each load, the start-up, every scene transition and every chunk (#1092): each stage's ms, share of the load and count (scene read, window, GPU device, audio device, renderer, procgen, models, textures, pipelines, scripts, `on_init`, physics, audio, the first frame...), what no stage named, and the five slowest assets by name. `--perf` prints it too ([Load time](../guides/profiling.md#load-time)) |
| `--perf-report <file>` | With `--exit-after`: write what the `[perf]` lines print as JSON when the run ends (#1093), plus each phase's and script's mean and max over the run, the GPU's passes as spreads, the hitch counts and the machine (adapter, backend, present mode, size, build profile, engine commit). Records every frame, times scripts and turns GPU timing on. `scripts/perf-compare.py` compares two ([Comparing two runs](../guides/profiling.md#comparing-two-runs)) |
| `--no-hot-reload` | Don't watch the game's files: scripts, UI documents, user styles, glyph sets and palettes load once and never reload, and nothing polls the disk (#633) |
| `--capture-frame-at <secs>` | Capture the frame at this game time for the frame debugger (#1086): a `<scene>-t<secs>.flintframe/` folder under `--capture-dir` with every pass's image, the frame's passes, draws and GPU times, the frame history around it and what a replay needs. Repeatable. With `--frame-step` two runs write the same folder apart from its timings. The run exits 1 when a time is never reached or a capture can't be written ([Capturing from the running player](../guides/profiling.md#capturing-from-the-running-player)) |
| `--capture-on-hitch[=N]` | Capture the frame after each of the run's first N hitches past the start (default 1). Hitches in the first 2 s of game time are passed over, as the exit report and `--max-hitches` set them apart (#1207); the first one passed over is said on the console. A hitch is over by the time it is found, so the capture is the next frame, and the manifest's `cpu` holds the hitch, its cause and the frame history around it. Turns the frame history and script timing on, as `--hitch-log` does |
| `--capture-on-hitch-startup` | Let `--capture-on-hitch` take the hitches of the first 2 s too, when the load is what you're chasing. Needs `--capture-on-hitch` |
| `--capture-dir <dir>` | Where frame captures go: by default `captures/` at the project's root. Shift+F2 and an input script's `capture frame` write there too, and so does a `capture` line on `--expression-listen` ([Asking the window for its frame](../concepts/scripting.md#asking-the-window-for-its-frame)) |
| `--record <dir>` | Record the window's frames as shown, scene and HUD, while the game plays (#583): `frame_000001.png`, ... with `frames.jsonl` (each frame's file, game time, wall seconds and Unix milliseconds, for lining a recording up with a transcript) and, at exit, `frames.ffconcat`, which plays them at the times they were shown: `ffmpeg -f concat -i frames.ffconcat -pix_fmt yuv420p out.mp4`. The frame never waits for a read-back: it copies the surface in its own encoder and a writer thread encodes and writes, so the scripts' clock is the same recorded or not. If four frames are still on their way to the disk, the next due one is skipped and counted. The folder is made if it's missing and refused if it already holds a `frames.jsonl`. Desktop only |
| `--record-fps <fps>` | Frames a second for `--record` (default 8, at most 240), on the wall clock, or on game time under `--frame-step`, so a stepped run records the same frames every time |
| `--trace <file>` | Record the engine's scopes and write the last `--trace-seconds` of them to the file when the run ends (#1097): Chrome trace-event JSON with a track per thread, the frames, the phases, the GPU's passes, the hitches and the engine's notes, which [Perfetto](https://ui.perfetto.dev) opens. Needs the recorder in the build: every debug build, or a release build with `--features trace` ([Recording a trace](../guides/profiling.md#recording-a-trace)) |
| `--trace-seconds <secs>` | How far back the recorder holds (default 5); `0` keeps the whole session, which grows without bound. Starts the recorder on its own, for Shift+F3 |
| `--trace-on-hitch[=N]` | Write a trace eight frames after each of the run's first N hitches past the start (default 1), beside `--trace`'s file or in `traces/` at the project's root |
| `--strict-scripts` | Exit 1 when any script failed, to load, to compile or while running (#693). Without it script errors are still counted and `--exit-after` prints them, a line each with its count ([When a script fails while running](../concepts/scripting.md#when-a-script-fails-while-running)) |

A stepped run is repeatable as long as its scripts are. `--frame-step` (or `--seed`) takes care of `random()`; a script that reads `timestamp()` still sees the wall clock. Two stepped runs of [`flint-examples/seeded_stars.scene.toml`](https://github.com/chrischaps/flint-examples/blob/main/seeded_stars.scene.toml), which scatters a sky with `random()` every second, write byte-identical screenshots; before #635 every run drew its own sky:

```bash
flint-player flint-examples/seeded_stars.scene.toml --frame-step 0.0166667 \
  --screenshot out/stars.png --screenshot-at 2.5 --exit-after 2.6            # the same PNG every run
flint-player flint-examples/seeded_stars.scene.toml --frame-step 0.0166667 --seed 2 \
  --screenshot out/stars_2.png --screenshot-at 2.5 --exit-after 2.6          # another sky, also every run
```

An input script plays its events at set game times. `#` starts a comment:

```text
0.0   down    KeyW               # hold a key (names as in input configs)
2.5   up      KeyW
3.0   tap     KeyF 0.4           # down now, up 0.4 s later (default 0.06)
3.0   axis    LeftStickX -0.35   # stick value on the script pad
4.0   press   South              # gamepad button down
4.2   release South
4.5   pull    RightTrigger2 0.6  # a trigger pulled part-way (0 lets go)
5.0   wheel   1                  # one wheel line up this frame
5.5   wheel   -0.5 2             # half a line down, two lines right
6.0   mouse   640 360            # pointer to (640, 360) in window pixels
6.1   down    MouseLeft          # mouse buttons work with down, up and tap
6.2   mouse   900 500 0.5        # glide there over 0.5 s: a drag
6.8   up      MouseLeft
7.0   click   320 200            # pointer there, then tap MouseLeft
7.5   click   320 200 MouseRight
8.0   down    ShiftLeft          # held modifiers reach egui: a shift-drag
9.0   type    200 planks         # text into the focused egui field
9.5   tap     Enter
10.0  look    40 -10             # raw mouse motion, for mouse-look
10.5  look    300 0 1.5          # spread over 1.5 s
11.0  capture frame              # capture the next frame for the frame debugger (#1086)
11.5  screenshot shots/door.png  # the next frame drawn, scene and HUD, as a PNG (#1191)
12.0  expect  door door.open == true          # ask the game (flint-query's conditions)
12.0  wait    player transform.position.z > 9 within 5   # hold what follows until it holds
```

`expect` and `wait` ask the game rather than play into it: an `expect`
once at its time, a `wait` every frame until it holds, holding back the
lines after it (they then play that much later). A failed one is printed
with the exit report and makes the run exit 1. Their forms (an entity's
field, `entities where`, `persist`, `log`, `event`, and `frame` for what the
last frame drew) are in [Playtesting](../guides/playtesting.md), which is
where they are most use. `screenshot <file>` writes the next frame drawn to
the file (from the working directory, as `--screenshot`); one never written
fails the run.

`pull` sets an analog button's travel, as a real pad's trigger sends it: a
`gamepad_button` binding to `RightTrigger2` reads 0.6, not the 0 or 1 that
`press` gives. It sets the value alone, so add a `press` and `release` too
when a script wants the button's press edge. `axis RightTrigger2` writes an
axis of that name, which a button binding never reads.

The mouse buttons are `MouseLeft`, `MouseRight`, `MouseMiddle`, `MouseBack`
and `MouseForward`. Pointer positions are physical window pixels, the same
pixels a screenshot has, so a point read off a capture can be clicked in the
next one. A glide moves the pointer a little every frame, as a hand would.

The mouse takes the path a real one does ([ADR 0082](https://github.com/chrischaps/Flint/blob/main/docs/adr/0082-input-script-mouse.md)): egui first, then game
extensions, then `InputState` and scripts. So a scripted click can press an
egui button, drag between egui widgets or open a tooltip, and a click over a
panel stays out of the world. The wheel goes the same way: turned over a
panel, it scrolls the panel (`wheel -10` scrolls a list down) and the game's
`mouse_wheel_*()` reads zero, as with a real wheel. Two things wait a frame,
however coarse the frames: a button press or a wheel turn in the frame the
pointer jumped (egui decides who gets a click or a scroll from where the
pointer was the frame before), and any release in the frame of its press. A
`click` therefore takes three frames: move, press, release.

Keys take the path a real keyboard does too ([ADR 0097](https://github.com/chrischaps/Flint/blob/main/docs/adr/0097-input-script-keys-reach-egui.md)). egui gets each one
as egui-winit would have made it, the key and, on a press, the character it
types on a US layout (`KeyA` with Shift held types `A`; nothing while Ctrl is
held). Shift, Ctrl, Alt and Super also send a `ModifiersChanged`, so egui
sees a held modifier on the clicks and drags that follow, and so do
extensions. Then the same focus rule as a real key: while an egui field has
keyboard focus, the key stops there, and `InputState`, scripts and the debug
keys never see it. `type` sends the rest of its line as egui text, spaces and
all, without pressing any keys; click a field first to focus it. A key egui
passes on goes to game extensions' `on_key` next, as a real one does (ADR
0103), with the character it stands for on a US layout. Two differences from
a real keyboard: extensions' `on_window_event` never sees a scripted key
(winit's `KeyEvent` can't be built outside winit), so read keys in `on_key`
to script them, and a scripted Esc doesn't quit the player. The real cursor still moves the pointer if it crosses the window
during a capture, so keep it outside.

`look` feeds raw mouse motion, what `mouse_delta_x()` / `mouse_delta_y()` read, at once or spread over the seconds given. As a real mouse's motion, it reaches the game only while the cursor is captured and no egui field has focus ([Cursor Capture and Input Gates](../concepts/input-gates.md), [ADR 0110](https://github.com/chrischaps/Flint/blob/main/docs/adr/0110-capture-and-focus-gates.md)). The script's pad plays whether or not the window has focus. A real pad doesn't.

Times are `GameClock` seconds (what scripts see via `total_time()`), not
wall-clock seconds since launch, so a capture lands on the same gameplay
moment regardless of load time. The capture reads the swapchain back on the
GPU, so it requires a surface that advertises `COPY_SRC`; on adapters that do
not (some GL/Android surfaces) the player logs a warning and skips the
screenshot rather than failing. `flint play` takes every one of these flags
too (#705).

### Asking a running game

A screenshot shows what a run looks like; most gameplay bugs turn on what it
*is*: where the player stands, whether the door is locked. Two flags let a
run answer that (#713), both in the [query](../concepts/queries.md) grammar
`flint query` takes for a scene file, and both answered by the same code, so
a query means the same thing whichever world it is asked of.

**A dump at game times**, for scripted runs. `--dump-world <file>` writes the
entities `--dump-query` picks (all of them by default), each with its
components' values, on the first frame at or after each `--dump-world-at`
time:

```bash
flint-player flint-examples/phase4_runtime.scene.toml --frame-step 0.0166667 \
  --input-script flint-examples/walk_to_door.txt --exit-after 5 \
  --dump-world out/world.json --dump-world-at 0.5,2.5,5 \
  --dump-query "entities where archetype == 'player'"
```

prints `[dump] out/world.json at t=2.500s (frame 151): 1 entities` as each
snapshot is written, and leaves (the second snapshot, shortened):

```json
{
  "scene": "flint-examples/phase4_runtime.scene.toml",
  "query": "entities where archetype == 'player'",
  "snapshots": [
    {
      "at": 2.5,
      "time": 2.5,
      "frame": 151,
      "entities": [
        {
          "id": 9,
          "name": "player",
          "archetype": "player",
          "components": ["character_controller", "collider", "rigidbody", "script", "transform"],
          "data": {
            "transform": {"position": [-0.0073, 0.86, 4.2899], "rotation": [0, 0, 0], "scale": [1, 1, 1]},
            "character_controller": {"height": 1.8, "move_speed": 5.0, "...": "..."}
          }
        }
      ]
    }
  ]
}
```

That run is the reason the dump exists: the input script holds W to walk to
the front door at z = 10, and a screenshot at 5 s shows the door ahead
under a `[E] Talk to Patron` prompt. The dump says the player stopped at
z = 4.29 by 2.5 s and stayed there, 5.8 m short of the door. A dump of every
entity (leave out `--dump-query`) says why: `tavern_table_3` stands at z = 5
with a collider 0.8 m deep, so its near edge is at z = 4.6, and the player's
capsule is 0.3 m round. `patron_1`, 1.2 m away, is who the script's `E`
spoke to.

`time` is the frame's game time and `at` the time asked for; `frame` counts
the run's frames from 1. The world is read after the frame's scripts,
physics, animation and procgen, and before `--exit-after`'s check, so a dump
at the exit time is the run's last world. The file is written whole again
after each snapshot. A time the run never reaches fails it, as a screenshot
does: `Error: --dump-world out/world.json: the run ended at t=3.200s, before
--dump-world-at 5`, exit 1. With no `--dump-world-at`, the world is dumped
at `--exit-after`'s time, else on the first frame.

**A live question**, for a game you are playing or a long run.
`--inspect-listen 127.0.0.1:<port>` opens a loopback socket (only 127.0.0.1
or ::1 can listen; off unless given), and `--attach <port>` on `flint query`
and `flint entity show` asks it instead of a file. What they print is what
they print for a file, so a command line moves between the two by swapping
`--scene` for `--attach`:

```bash
flint play flint-examples/phase4_runtime.scene.toml --inspect-listen 127.0.0.1:7878

# in another terminal, while it runs
flint query "entities where door.locked == true" --attach 7878
flint entity show player --attach 7878
flint query "entities where archetype == 'character'" --attach 7878 --data
```

Each answer says on stderr when in the run it was read
(`live: flint-examples/phase4_runtime.scene.toml at t=12.483s (frame 749)`);
stdout is the answer alone. The socket speaks one line of JSON each way, so a
program can ask without `flint`: send `{"query": "entities", "data": true}`,
`{"entity": "player"}`, or a bare query line, and read back
`{"ok": true, "scene": ..., "time": ..., "frame": ..., "answer": ...}` or
`{"ok": false, ..., "error": "Entity 'x' not found"}`. The network is read
and written on threads of its own; the frame answers at most 16 questions,
at the end of its update, and a question no frame answers within 10 s (a
game stopped in a debugger) is answered with an error saying so.

### Player Controls (Defaults)

These are the built-in defaults. Games can override any binding via input config files (see [Physics and Runtime: Input System](../concepts/physics-and-runtime.md#input-system)).

| Input | Action |
|-------|--------|
| WASD | Move |
| Mouse | Look around |
| Left Click | Fire (weapon) |
| Space | Jump |
| Shift | Sprint |
| E | Interact with nearby object |
| R | Reload |
| 1 / 2 | Select weapon slot |
| Escape | Release cursor / Exit (the game's own key when its input config binds it, see [Escape](../concepts/physics-and-runtime.md#escape)) |
| F1 | Show the debug keys: every one, its action and whether it acts in this scene |
| F2 | Toggle the render stats overlay (FPS, frame time, draw stats) |
| F3 | Toggle the scene debug panels (see below); leaves the Rendering & Effects panel alone |
| F4 | Toggle the **Rendering & Effects** menu: every render and post-processing control, including debug shading mode, shadows, and the lighting levers |
| F11 | Toggle fullscreen |
| F5-F9, `` ` ``, `\` | Living props' and a charted music session's keys, only in a scene that has them; anywhere else they are the game's |

These are the debug keys' defaults. An input config's `[debug_keys]` moves, unbinds or turns them off, a key the game binds is the game's, and `--list-keys` prints the table ([Debug Keys](../concepts/physics-and-runtime.md#debug-keys); the full list is on [Debug Panels](../guides/debug-panels.md#keys)). The per-effect F-keys of earlier releases (F1 debug mode, F4 shadows, F5 bloom, F6 post-processing and so on) are gone; all of those toggles and their parameters live in the F4 menu (see [Post-Processing: The Rendering & Effects menu](../concepts/post-processing.md#the-rendering--effects-menu-f4)). F3, F4, F8, F9, `` ` `` and `\` exist only when the player is built with the default `debug-hud` feature.

Gamepad controllers are also supported when connected. Bindings for gamepad buttons and axes can be configured in input config TOML files.

### Debug Panels (F3)

`F3` opens live tuning panels for whichever systems the current scene actually
uses. Each panel is created **only when its driving component is present**, so a
scene with no ocean never sees an ocean panel, and in a scene with no panels at
all `F3` shows a short note at the top of the window (and prints it to the
console) instead. Panels are fold-open headers distributed across up to three
size-balanced columns.

Built-in panels:

| Panel | Component | Controls |
|-------|-----------|----------|
| Ocean Debug | `ocean` | Wave spectrum, colors, foam, contact foam, cel band edges, clarity/turbidity, grid, CPU/GPU parity probe |
| Camera | `camera_tuning` | Vertical FOV |
| Grass Debug | `grass` | Density, height, wind, LOD distances |
| (its schema's title) | any component whose schema puts a field on a panel (`panel = "status"`, `"tune"`, `"trigger"`) | A status line, trigger buttons and tune fields, from the schema alone ([A Panel From a Schema](../guides/debug-panels.md#a-panel-from-a-schema)) |
| Rendering & Effects | (always, when a renderer is active) | Post chain, SSAO, depth of field, fog, bloom, grade/grain/FXAA, Kuwahara, contour, shading boil, tear, dither/volumetric, shadows and resolution, lighting levers, camera FOV, debug shading mode. Owned by **F4**, not F3 |
| Music Guide | `music_session` | Upcoming pulse/press/flick windows with countdown, per-channel targets beside the live stick and trigger state (`` ` ``) |
| Manifest Map | `music_session` | Full-width bottom strip: sections, bar ruler, tempo/meter changes, re-entry points, playhead and this run's judged pulses and seams (`\`) |

The full roster and the `DebugPanel` trait are described in [Debug Panels](../guides/debug-panels.md).

Edits apply live through the world's components. **Commit to File** writes the
current values back into the scene TOML, so a tuning session ends as a diff
rather than as notes. Values a game script owns each frame (a day counter, a
published factor) are deliberately never committed.

Games get panels for their own components from those components' schemas,
with no Rust; a game binary can also add panels through its extension.

`Escape` releases the mouse so panels can be clicked; clicking the world
recaptures it.

For first-person play the scene needs an entity with a `character_controller` component (the `player` archetype bundles one with its collider and rigidbody). Physics colliders on other entities define the walkable geometry. A scene with no such entity plays through its `[camera]`.

### Game Project Pattern

Games that define their own schemas, scripts, and assets load several schema directories. Game projects typically live in their own repositories with the engine included as a git subtree at `engine/`. The engine schemas come first, then the game-specific schemas overlay on top. The game's `flint.toml` says so (`[project] schemas = ["engine/schemas", "schemas"]`), and then no command needs a flag:

```bash
# From anywhere in a game project (engine at engine/)
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/level_1.scene.toml

# Without a manifest, or to override it: the flags, in override order
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/level_1.scene.toml \
  --schemas engine/schemas \
  --schemas schemas
```

This loads the engine's built-in components (transform, material, rigidbody, etc.) from `engine/schemas/`, then adds game-specific components (health, weapon, enemy AI) from the game's own `schemas/`. See [Schemas: Game Project Schemas](../concepts/schemas.md#game-project-schemas) for directory structure details and [Building a Game Project](../guides/building-a-game-project.md) for the full workflow.

### Standalone Player Binary

The player is also available as a standalone binary for distribution:

```bash
cargo run --bin flint-player -- flint-examples/phase4_runtime.scene.toml

# From a game repo with an engine subtree: its flint.toml names the schemas
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/level_1.scene.toml
```

## The `playtest` Command

Play a scene with no window, GPU or sound device (or, with `--render`, a
GPU drawing every frame offscreen), on a fixed clock, and report whether
it passed: every `expect` and `wait` of the input script,
every `test_assert`, `test_fail` and `test_pass` of the scripts, every script
error with its file and line, and the engine's warnings, as one JSON document
that two runs write byte for byte. Exit 1 on any failure. The whole story is
[Playtesting](../guides/playtesting.md).

```bash
flint playtest flint-examples/phase4_runtime.scene.toml     --input-script flint-examples/playtests/phase4_runtime.playtest.txt --exit-after 8
flint playtest scenes/level1.scene.toml --timeout 30 --strict --dump player --json
flint playtest scenes/level1.scene.toml --test-script playtests/coins.rhai --timeout 30
flint playtest scenes/level1.scene.toml --render --screenshot shots/t2.png --screenshot-at 2
flint playtest scenes/menu.scene.toml --save-from saves/v1_halfway.toml --json
```

| Flag | Description |
|------|-------------|
| `--input-script <file>` | Keys, pads and the pointer, and `expect` / `wait` lines that check the game |
| `--test-script <file>` | A Rhai file of the playtest's own checks, attached to an entity the run spawns in every scene (`playtest`; repeatable, `playtest_2` ...): the whole script API and `test_*`, with nothing added to the scene |
| `--exit-after <s>` | End at this game time, passing if nothing failed (10 when neither this nor `--timeout` is given) |
| `--timeout <s>` | End at this game time as a failure: the run should have ended itself (`test_pass()`, `test_fail()`, `quit_game()`) |
| `--frame-step <s>` | Game seconds a frame (default 1/60) |
| `--seed <n>` | `random()`'s seed (default: every stepped run's) |
| `--strict` | A warning while the scene loads fails the run, and so does anything `flint check` finds in the scene |
| `--save-from <file>` | Start from this save file, loaded as the player loads one (its version and `on_persist_migrate`, a file that doesn't parse); the run writes nothing and the file is left as it is |
| `--dump <entity>` | Put the entity's components at the end in the report (repeatable) |
| `--report <file>` | Write the JSON report to this file as well |
| `--timings` | Wall-clock times in the report (two runs then differ) |
| `--width <w>`, `--height <h>` | The screen scripts see, in points (1280 x 720); with `--render`, the frame's pixels |
| `--render` | Draw every frame offscreen, scene and HUD, with no window: models load, and screenshots and the input script's `frame` lines see what was drawn |
| `--screenshot <file>` | Write the frame at `--screenshot-at` as a PNG (needs `--render`) |
| `--screenshot-at <s>` | The game time of that frame (default 0) |
| `--screenshot-every <s>` | Numbered frames every so many game seconds from `--screenshot-at` |
| `--schemas <dir>` | Schema directories (repeatable); without it, the project's |
## The `capture` Command

Captures a shot list (`[capture]` and its `[[shot]]`s: a scene, `seconds`,
optionally a look, a seed, an input script and several takes) as PNG frame
sequences, each take one run of `flint play` with the clock stepped exactly
one frame a frame, so a slow frame changes nothing on screen. The list's
keys and what a take is: [Filming a Showcase](../guides/filming-a-showcase.md).

```bash
flint capture showcase_game/trailer.shots.toml
flint capture showcase_game/trailer.shots.toml --verify --encode mp4
flint capture flint-examples/film/film.toml --shot 02_type --scale 0.5 --dry-run --json
```

### Capture Flags

| Flag | Description |
|------|-------------|
| `--shot <id>` | Only this shot (repeatable) |
| `--take <name>` | Only takes of this name (repeatable) |
| `--scale <f>` | A fraction of each shot's size, keeping the point canvas |
| `--width <w>`, `--height <h>` | One frame size for every shot |
| `--out <dir>` | Where takes go (default: the list's `out`, else `captures/<list name>/` beside it) |
| `--verify` | Capture each take twice and compare the runs; exit 1 when a frame differs |
| `--tolerance <levels>` | With `--verify`: the largest 8-bit difference that still counts as the same |
| `--encode mp4` | Also an MP4 a take (H.264, BT.709, no sound; needs ffmpeg on `PATH`) |
| `--dry-run` | Print each take's frames and `flint play` command line; capture nothing |
| `--no-disk-check` | Capture even when the estimate (8 MB a 4K frame) passes 90% of the free space under `--out`; without it such a run stops before any take |
| `--schemas <dir>` | Schema directories for the player (repeatable; default: the scene's project's) |

Words after `--` are added to every take's `flint play` command line.

## The `render` Command

Render a scene to a PNG image without opening a window:

```bash
flint render flint-examples/showcase_tavern.scene.toml --output hero.png
flint render scene.toml -o shot.png --distance 20 --pitch 30 --yaw 45 --target 0,1,0 --no-grid
```

| Flag | Default | Description |
|------|---------|-------------|
| `--output <path>` / `-o` | `render.png` | Output file path |
| `--width <px>` | `1920` | Image width |
| `--height <px>` | `1080` | Image height |
| `--gpu <name>` | (high-perf) | GPU adapter to use (substring match); `--list-gpus` prints the names |
| `--list-styles` | | Print every registered style, its stage, table, and each parameter's default and range, then exit; with a scene, its `styles/` user styles too ([Stylize Passes](../concepts/stylize-passes.md#listing-the-styles)) |
| `--list-effects` | | Print every built-in post effect and each of its `[post_process]` keys with its kind, default, the range it is tuned for and its help, then exit ([Post-Processing](../concepts/post-processing.md#listing-the-keys)) |
| `--json` | | The [global flag](#machine-readable-output): a render's report as one document, as `--report json` prints it; with `--list-styles`, `--list-effects` or `--list-gpus` the list; with `--variant` the files written |
| `--watch` | `false` | Keep running and write `--output` again after every save to the scene's folder, its style folders, its schemas or a file a flag names (#432). A still or a `--sweep` keeps its GPU, scene and renderer, so a user style's `.wgsl` save is drawn again on them; anything else (and a `.shader.toml` whose description changed) loads the scene again on the same GPU. One `watch: render <n> in <s>` line a render, then each style's error. Ctrl-C stops it ([Stylize Passes](../concepts/stylize-passes.md#user-authored-styles)) |
| `--distance <f32>` | (auto) | Camera distance from target |
| `--yaw <deg>` | (auto) | Horizontal camera angle |
| `--pitch <deg>` | (auto) | Vertical camera angle |
| `--target <x,y,z>` | (auto) | Camera look-at point |
| `--fov <deg>` | (auto) | Field of view |
| `--no-grid` | `false` | Disable ground grid |
| `--select <entity>` | (none) | Outline this entity (by name) and its descendants as `flint edit` does when it is picked |
| `--debug-mode <mode>` | (none) | `wireframe`, `wireframe-overlay`, `normals`, `depth`, `uv`, `unlit`, `metalrough`, `style`, `edges` |
| `--wireframe-overlay` | `false` | Wireframe edges on solid geometry |
| `--show-normals` | `false` | Normal direction arrows |
| `--no-tonemapping` | `false` | Raw linear output |
| `--no-shadows` | `false` | Disable shadow mapping |
| `--shadow-resolution <px>` | `2048` | Shadow map resolution per cascade. A real control since the per-resolution texel upload ([ADR 0049](https://github.com/chrischaps/Flint/blob/main/docs/adr/0049-upload-shadow-texel-size-per-resolution.md)); earlier builds silently filtered as if 2048 |
| `--msaa <n>` | `1` | MSAA sample count for the scene passes: `1` (off) or `4`. Default 1 keeps headless pixel-diff gates single-sample ([ADR 0058](https://github.com/chrischaps/Flint/blob/main/docs/adr/0058-opt-in-4x-msaa-for-scene-passes.md)) |
| `--render-scale <f32>` | `1` | Draw the scene at this fraction of the image on each side (0.25–1), upscaled in the composite, as `flint-player --render-scale` (#122) |
| `--gpu-time <frames>` | (none) | Render the frame this many times with GPU timing on and print each pass's smoothed time ([ADR 0108](https://github.com/chrischaps/Flint/blob/main/docs/adr/0108-gpu-timing-from-timestamp-queries.md)) before writing the image |
| `--frame-dump <file.json>` | (none) | Write the rendered frame's description as JSON (#1084, [ADR 0126](https://github.com/chrischaps/Flint/blob/main/docs/adr/0126-what-a-frame-capture-holds-and-how-it-replays.md)): every pass in the order drawn, named as `--gpu-time`'s rows are, with the targets it read and wrote, its draws by entity, what a cull set aside and why, why a skipped pass was skipped and its GPU time in that frame; then the frame's settings, targets, lights and stats. Of a `--time` span, the last frame; not with `--frames`, `--mesh` or `--sweep`. `--report json` names the file ([A frame's description](../guides/profiling.md#a-frames-description)) |
| `--frame-capture <dir>` | (none) | Capture the rendered frame into a folder (#1085): its description as `manifest.json`, written last, and under `targets/` every target each pass wrote, as that pass left it (`NN-<target>.<ext>`: 8-bit as PNG, float as OpenEXR half, depth as OpenEXR f32). The last `surface` image is the `-o` output byte for byte. Of a `--time` span, the last frame; not with `--frames`, `--mesh` or `--sweep`; a folder holding anything else is refused. `--report json` names it as `frame_capture` ([A frame's images](../guides/profiling.md#a-frames-images)) |
| `--load-report` | off | Time the load (#1092), from the scene read to the first capture: the GPU device, the renderer, every model, texture, pipeline and script. Prints the table `flint play --load-report` prints; `--report json` carries it as `load` ([Load time](../guides/profiling.md#load-time)) |
| `--frames <n>` | (none) | Draw a sequence of `n` frames on one renderer, written as `<stem>_0000.png`... beside `--output`; temporal styles see consecutive frames ([Frame sequences](../guides/headless-rendering.md#frame-sequences), #345) |
| `--frame-dt <s>` | `1/60` | With `--frames`: seconds between frames; post time, grass wind and particles advance by it |
| `--orbit <deg/s>` / `--camera-path <file>` | (still) | With `--frames`: orbit the camera about its target, or follow a TOML of `[[key]] time/position/target` keys |
| `--flip-stats` / `--flip-json <file>` | (off) | With `--frames`: count the ASCII style's glyph flips and A-B-A reversals from its own grid, printed per frame and in sum, or written as JSON |
| `--no-images` | `false` | With `--frames`: write no PNGs |
| `--last-frame` | `false` | With `--frames`: write only the last frame, to `--output` itself (not numbered): the sequence's end, a temporal style's history settled (#478) |
| `--mesh <entity>=<files>` | (none) | A frame per mesh file (one GLB, a folder, or `out/f*.glb`, natural order) shown on that entity, all on one renderer; repeatable, frame for frame. `-o` numbered PNGs, `%03d`, a `.gif` at `--fps`, or `--sheet` ([Mesh sequences](../guides/headless-rendering.md#mesh-sequences), #222) |
| `--sheet` | `false` | With `--mesh`: one labelled contact sheet, a cell a file |
| `--ascii-text <file>` | (none) | Also write the ASCII grid as UTF-8 text: a line per cell row, a character per cell, matching the PNG (a passed-through cell is a space; #317, [Reading a render as text](../guides/headless-rendering.md#reading-a-render-as-text)). Needs the style on; not with `--sweep` |
| `--ascii-text-ansi <file>` | (none) | The same grid with 24-bit ANSI colours, each glyph's ink over its ground |
| `--pixels-per-point <f32>` | `1` | A high-DPI window's scale factor: styles sized in points (ASCII cells) scale by it |
| `--no-postprocess` | `false` | Disable post-processing |
| `--boil-time <f64>` | `0` | Seconds of the wooze field to run, which picks the drawing |
| `--grain-time <s>` | `0.0` | Post time for grain and the styles' animation. Deterministic: two renders at the same value are identical |
| `--look <name\|path>` | (none) | A look merged over the scene's `[post_process]`, key by key: a built-in, a `styles/<name>.look.toml` beside the scene, or a path. `--look help` lists every look and what to tune |
| `--set <key=value>` | (none) | Set one key as the scene would write it (#708), repeatable, in the order given. `post.<key>=v` is a `[post_process]` key: `post.exposure=1.5`, `post.fog_color=0.7,0.75,0.82`, `post.fxaa=true`. `<style>.<key>=v` is a key of a style's table (`ascii.cell=12`), leaving the style on or off as it was. `--list-effects` and `--list-styles` list the keys |
| `--style <name[:k=v,...]>` | (none) | Turn on any registered style over the scene's post-processing: `ascii`, `ascii:cell=12,ramp=blocks`, `invert:strength=0.5`, `kuwahara:radius=6`, `ascii:off` (off everywhere, entities' own included, #376). Repeatable; a later word wins; `--style help` lists every style and its keys ([Stylize Passes](../concepts/stylize-passes.md#using-a-style)) |
| `--style-time <s>` | `0.0` | Post time animated styles read; the same clock as `--grain-time`, and it wins when both are given |
| `--particle-time <s>` | (none) | Simulate particle emitters and effects for this long at a fixed 1/60 s step before capturing. Deterministic; without it no particles are drawn |
| `--anim-time <s>` | (none) | Run the scene's animation (skeletal clips, layers, sequences, property tweens, node and sprite clips, then IK) to this time at a fixed 1/60 s step before capturing, as the player poses it there. Deterministic; with `--time` or `--frames` it runs on with the clock. Without it skinned meshes draw in their bind pose. See [Headless Rendering](../guides/headless-rendering.md#posed-characters) |
| `--time <s>` or `<a..b>` | (none) | Capture at this game time, with living props, the wooze field and the other clocks there, matching the player. A span (`0..4`) writes an animated `--output x.gif`, or a PNG a frame to `--output frames/f-%05d.png`; either runs in about one frame's memory however long. See [Headless Rendering](../guides/headless-rendering.md#a-chosen-moment) |
| `--frame-step <s>` | `1/60` | With `--time` or `--script-time`: how far each clock tick moves, as `flint-player --frame-step` |
| `--scripts` | `false` | Run the scene's scripts, a clock step at a time through the player's own frame (its game core), deterministically (seeded `random()`, no wall clock). Commands go through the player's dispatcher, so styles, tweens, style reads, looks and the camera apply as in the player; what a capture can't do (sound, the window, other scenes, physics) is named in a warning. See [Headless Rendering](../guides/headless-rendering.md#scripts-and-an-expression-timeline) |
| `--script-time <t>` | (none) | `--scripts --time <t>` in one flag, beside `--particle-time` and `--anim-time`: run the scene's scripts to game time `t` and capture there, so a look a script sets is in the picture. Takes what `--time` takes (a span, a list) |
| `--expression-timeline <file>` | (none) | Feed scripts' `expression()` and `on_expression` from a JSON timeline on the render's clock. Implies `--scripts` |
| `--strict-scripts` | `false` | Exit 1, after writing the picture, when any script failed to load, to compile or while running (#693). Implies `--scripts`. Either way a scripted render prints each error once with its count, and `--report json` lists them as `script_errors` |
| `--fps <n>` | `12` | With a `--time` span: GIF frames per second of game time, up to 50 |
| `--strip` | `false` | With a `--time` span or list: one PNG contact sheet of it instead of a GIF, a tile a moment, each labelled with its time; each tile is the pixels of `--time <t>` rendered alone at the tile's size (640 x 360 by default). See [A span as a contact sheet](../guides/headless-rendering.md#a-span-as-a-contact-sheet) (#574) |
| `--every <s>` | `1` | With `--strip` and a span: seconds of game time between tiles; the span's end is a tile when a step lands on it. At most 100 tiles |
| `--columns <n>` | about square | With `--strip`: tiles a row (alias: cols) |
| `--label <on\|off>` | `on` | With `--strip`: `off` draws only the tiles, 4 pixels apart, no title, times or borders |
| `--sweep <knob=a,b,...>` | (none) | The same moment once per value, as labelled columns: `wooze.glide=0,0.25,0.5,1`. Takes `wooze.amplitude`, `.step`, `.glide`, `.jitter`, `.preset`, and the scene's `[living] echoes` as `living.echoes.count`, `.spread`, `.alpha` (#531) |
| `--report json` | (none) | One JSON object on stdout: each living prop's lineage seed, pose indices, the wooze values it read, and every species value with the layer that set it, and `script_errors`, each script error of the run with its entity, callback, line and count. Everything else moves to stderr |
| `--oren-nayar <f32>` | `0` | Oren-Nayar diffuse blend (0 = Lambert, 1 = full); see [Lighting](../concepts/lighting.md) |
| `--sheen-strength <f32>` | `0` | Charlie-sheen rim strength (keep at or below about 0.3) |
| `--sheen-color <r,g,b>` | `1,1,1` | Charlie-sheen rim tint |
| `--schemas <path>` | manifest, else discovered | Schemas directory (repeatable; see [Common Flags](#common-flags)) |

The old render-mode flags (deprecated by #746) are hidden from the help but still work for one release, each mode drawn as the style that replaced it: 1 matrix, 2 blood, 3 drunk, 4 tron, 5 underwater, 6 the tear alone ([The old render modes](../concepts/post-processing.md#the-old-render-modes)).

When no camera flags are given, the render starts from the scene's `[camera]` block if it has one. The camera flags override the scene's authored `[camera]`.

**Post-processing and styles are keys, not flags.** Any `[post_process]` key is `--set post.<key>=<value>`, and a style is turned on with its keys by `--style <name>:<key>=<value>,...`. They apply over the scene's block and `--look`, `--set` in the order given and `--style` last, so the command line wins:

```bash
flint render scene.toml -o warm.png --set post.exposure=1.2 --set post.grade_gain=1.04,1,0.94 --set post.film_grain=0.03
flint render scene.toml -o fog.png --set post.fog_enabled=true --set post.fog_density=0.05
flint render scene.toml -o dof.png --set post.dof_strength=0.6 --set post.dof_focus_distance=8 --set post.dof_focus_range=3
flint render scene.toml -o painted.png --style kuwahara:radius=6 --style contour:width=1.5
flint render scene.toml -o glyphs.png --style ascii:ramp=blocks,palette=phosphor,color=palette --set ascii.cell=12
```

A key that turns an effect on is its own key: `post.fog_density` alone leaves fog off in a scene that has `fog_enabled = false`, and likewise bloom, dither, height fog and volumetric light. A colour in a `--style` word is `r/g/b` or `#rrggbb`, since commas separate its keys. `flint render --list-effects` lists every `[post_process]` key with its kind, default and tuned range, `--list-styles` every style's keys, `--look help` the looks, and `flint render --help` every flag by heading. The per-effect flags these replaced went in #1144: given one, `flint render` stops with an unknown-flag error naming its `--set` or `--style` spelling.

> **`flint render` runs no scripts unless you pass `--scripts` (or `--script-time <t>`),**
> and a render of a scene with scripts says so on stderr, naming them. Without it,
> anything your game drives from a script — a render mode, a time of day, a
> floating hull, an animated character — will not happen. With it, what scripts
> write to the world, Living Props and the wooze field is drawn, but physics,
> particles, render modes and the HUD still aren't (a warning names them). The
> render-mode flags exist so a stylized frame can be captured without a script.
> For animation, note that skinned meshes render at
> **bind pose** headlessly unless you pass `--anim-time 1.5`, which poses every
> animated entity as the player has it at that time. For one rigged model on its
> own, the model previewer does it too: `flint edit model.glb --render out.png
> --anim-time 1.5` (optionally with `--clip`, `--layer` or `--sequence`). Particles
> step the same way with `--particle-time 2`, and living props draw at time 0 or
> at `--time`.

## The `edit` Command

Unified interactive editor that auto-detects file type and opens the appropriate tool:

```bash
flint edit levels/demo.scene.toml              # Scene viewer (hot-reload)
flint edit levels/demo.scene.toml --spline     # Spline/track editor
flint edit models/character.glb                # Model previewer (orbit camera)
flint edit models/character.glb --watch        # Model previewer with file watching
flint edit specs/oak_tree.procgen.toml         # Procgen previewer (mesh/texture)
flint edit specs/stone_wall.procgen.toml       # Texture pipeline editor (if pipeline pattern)
flint edit terrain.terrain.toml                # Terrain editor
flint edit fx/fire.particles.toml              # Particle effect editor (created from --preset if missing)
```

### File Type Detection

| Extension | Tool | Description |
|-----------|------|-------------|
| `.scene.toml`, `.chunk.toml` | Scene viewer | Hot-reload, egui inspector, gizmos |
| `.procgen.toml` (pipeline pattern) | Texture pipeline editor | Node graph for texture specs |
| `.procgen.toml` (other) | Procgen previewer | Live preview of generated mesh/texture |
| `.terrain.toml` | Terrain editor | Heightmap terrain editing |
| `.particles.toml` | Particle effect editor | Emitters, curves, forces, bursts; scrub timeline; headless `--render` |
| `.glb`, `.gltf` | Model previewer | Orbit camera, animation playback |

### One host, the same flags

Every editor runs in one host (`flint_viewer::host`, #712): it opens the
window, the GPU and egui, feeds the orbit camera, watches the file and draws
the `--render` still. The orbit camera is one controller for all of them, the
scene viewer and track editor included (#1153), so a drag, a scroll, WASD/QE
and the O turntable turn every 3D view at the same speeds. `--watch` is the
host's too: the scene viewer asks it for the scene's folder and the schema
folders with everything under them. A flag means the same in every editor that takes it,
and one an editor doesn't take is an error that names the editors it applies
to, before anything opens:

| Flag | Editors |
|------|---------|
| `--width`, `--height`, `--watch`, `--render` | every editor |
| `--no-grid`, `--distance`, `--yaw`, `--pitch`, `--target`, `--fov` | the ones with a 3D view: scene, track (`--spline`), procgen mesh, terrain, model, particles |
| `--auto-orbit` | the same, but the track editor |
| `--seed` | procgen specs, texture pipelines, terrain |
| `--schemas` | scene, track |
| `--no-inspector` | scene |
| `--style`, `--style-time` | scene; a model's or effect's `--render` still |
| `--motion-hold` | scene, model |
| `--anim-time` | a model's or effect's `--render` still |
| `--clip`, `--layer`, `--sequence`, `--sequence-loop`, `--anim-speed`, `--no-animate`, `--pose-bone`, `--show-skeleton` | model |
| `--preset` | particles |

The camera flags lay themselves over the editor's own framing (the model
auto-fitted, the terrain from above, the scene's `[camera]`), so a flag left
out keeps that part of it.

**F1 lists the debug keys, in every editor** (#1170), as it does in the
player: the same card, the host's key for each of them. An editor's own
function keys are a table like the player's ([Debug Panels](../guides/debug-panels.md#keys)),
each with an owner and an action name, and the host looks a press up in it;
no editor matches an F-key of its own. The scene viewer's are F2 to F4 and
the procgen previewer's F2 to F5 (the tables below); the other editors have
none yet, so their card shows F1 alone. A held key doesn't repeat its
action. The editors read no input config, so `[debug_keys]` rebinds the
player's keys only.

`--render out.png` writes what the window would show, without the panels, at
`--width` x `--height`, for every kind of file:

```bash
flint edit flint-examples/specs/grove_oak.procgen.toml --render oak.png --yaw 60   # the mesh, auto-framed
flint edit flint-examples/specs/tavern_plank.procgen.toml --render plank.png       # albedo, normal, roughness side by side
flint edit flint-examples/rolling_meadow.terrain.toml --render meadow.png          # the terrain from above
flint edit flint-examples/stylize_props.scene.toml --render props.png --no-grid    # the scene viewer's frame
```

A texture generator's or a texture pipeline's still is its output maps side by
side, in the order the editor's tabs list them, and the command prints that
order. The track editor's still is the scene from the editor's camera with the
spline over it, as the window draws it: the curve, the thin polygon joining
its control points, and each point as a numbered white disc.

```bash
flint edit flint-examples/sweep_garden.scene.toml --spline --render track.png  # the track and its handles
```

### GPU Selection

Both windowed (`play`, `flint-player`, editors) and headless (`render`)
contexts choose their wgpu adapter through the same chain, first match wins:

1. `--gpu <name>` on `play`, `render`, or `flint-player`
2. `WGPU_ADAPTER_NAME=<name>` environment variable
3. `WGPU_POWER_PREF=high|low|none` environment variable
4. Default: high-performance adapter

The name is a case-insensitive substring of the adapter name; if nothing
matches, the engine warns with the available names and falls back to the
power preference. `--list-gpus` prints every adapter with its type and backend.
`flint render` prints the adapter it drew on as its `GPU:` line, and
`RUST_LOG=info` logs the pick with its driver:

```
GPU: NVIDIA GeForce RTX 4070 (DiscreteGpu, Vulkan)
```

The default matters on dual-GPU laptops: before this, `PowerPreference::None`
let the backend hand back whichever adapter enumerated first, usually the
integrated GPU.

#### DX12's shader compiler

On DX12, every shader is compiled by one of Microsoft's two compilers before it
draws. Flint uses **DXC** by default and keeps **FXC** only as the fallback.
FXC is old and slow. On Flint's ASCII style, a debug `flint render` on an RTX 4070
took 26-27 s on FXC and 5 s on DXC (1.1 s on Vulkan). FXC is also where Flint's
DX12-only miscompiles came from.

**DXC is linked into Flint's binaries.** No `dxcompiler.dll` or `dxil.dll` sits
beside them, and there is nothing to install. Where each compiler comes from:

- **FXC** is part of Windows (`d3dcompiler_47.dll`).
- **DXC** is Microsoft's open-source DirectX Shader Compiler, v1.9.2609. Microsoft
  publishes it only as DLLs, so Flint builds it from Microsoft's source as one
  static library, with a one-line change (`tools/dxc-static`). The `static DXC`
  workflow builds it and publishes it as a release asset of the Flint repository,
  and every Windows build links it (flint-render's default `static-dxc` feature,
  through wgpu's). Since DXC 1.8.2505 the compiler signs what it compiles itself,
  so `dxil.dll` isn't needed.

The first build on a machine downloads that archive once (80 MB) into
`%LOCALAPPDATA%\flint\static-dxc\`, and checks its SHA-256 against the pinned
value. Every target directory and worktree on the machine then reuses it. With no
network and nothing cached, the build stops and names the URL. Put the file in
that folder, or point `FLINT_STATIC_DXC_ARCHIVE` at a copy of it.
`FLINT_STATIC_DXC_CACHE` moves the folder. Linking it needs Visual Studio's C++
tools at least as new as the ones that built it; the build says so if yours are
older. Static DXC is linked on x86_64 Windows with MSVC only. On other Windows
targets, Flint loads a `dxcompiler.dll` it finds by name, the executable's folder
first, else uses FXC.

DX12 takes wgpu's own default, `Dx12Compiler::Auto`, which picks the linked DXC.
Every adapter line says which compiler DX12 uses, and why. These lines come from
`flint render`'s `GPU:` line, `flint goldens`' `adapter:` line, `flint dxc status`,
the player's F3 overlay and its `--perf` line (which shows only `DXC`, `FXC` or
`FXC fallback`):

```
GPU: NVIDIA GeForce RTX 4070 (DiscreteGpu, Dx12, shaders DXC (static, v1.9.2609))
GPU: NVIDIA GeForce RTX 4070 (DiscreteGpu, Dx12, shaders FXC (WGPU_DX12_COMPILER=fxc))
```

- **`WGPU_DX12_COMPILER`** overrides the choice. wgpu reads it: `fxc` forces FXC,
  to work around a driver or DXC bug; `dxc` loads `dxcompiler.dll` by name instead;
  `staticdxc` and `auto` give the default. If the DLL it asks for doesn't load,
  DX12 gets FXC and the line says `FXC (fallback: ...)`.
- **A shader that DXC rejects** is not retried with FXC. Its pipeline fails, as on
  any other backend.
- `flint dxc status` prints the linked DXC's version, the commit it was built from,
  how it was built, and the compiler DX12 would use.

##### Shipping DXC with your game

DXC comes under its own open-source licence, not Microsoft's binary terms: the
LLVM Release License (University of Illinois/NCSA), plus the notices for what it
incorporates. No `dxil.dll` is in Flint's build, so the DLL release's
`LICENSE-MS.txt` doesn't apply. That's Flint's reading, not legal advice. A
Windows build of your game that carries Flint ships DXC's notices beside its
executable:

```bash
flint dxc notices --to dist/win   # writes THIRD-PARTY-NOTICES-DXC.txt
flint dxc notices                 # or prints it
```

The file names the DXC release and the commit it was built from, then carries
DXC's `LICENSE.TXT` and `ThirdPartyNotices.txt`. `flint export` has no desktop
target yet, so nothing copies it into a build for you.

### Edit Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--schemas <path>` | manifest, else discovered | Schemas directory (repeatable; see [Common Flags](#common-flags)) |
| `--width <px>` | per editor (scene viewer and track editor `1600`) | Window width in physical pixels |
| `--height <px>` | per editor (scene viewer and track editor `900`) | Window height in physical pixels |
| `--no-grid` | `false` | Disable the ground grid, in every editor including the scene viewer and `--spline` |
| `--watch` | `false` | Watch for file changes |
| `--seed <u64>` | (auto) | Override seed (procgen) |
| `--no-inspector` | `false` | Hide egui inspector (scene) |
| `--spline` | `false` | Open the spline/track editor instead of the viewer (scene) |
| `--auto-orbit` | `false` | Start with the turntable auto-orbit on (scene/model/procgen); toggle with **O**, speed with `[` / `]` |
| `--motion-hold <rate>` | (the scene's) | Motion on twos for the whole frame (#871), as `--set post.motion_hold=<rate>` is on `flint render`: the scene viewer and the model previewer draw positions and poses on ticks, this many a second (0 = off). See [Motion on twos](../concepts/stylize-passes.md#motion-on-twos) |
| `--log-format <text\|json>` | `text` | How the editor writes what it says, to stderr, as the player does: `json` is one object a line with `timestamp`, `level`, `target`, `message` and the event's fields (#1164) |
| `--quiet` | `false` | Say only warnings and errors: no load summary or the window's notices |

The editors write what they say through the player's log (see
[What the player says](#what-the-player-says)): a scene, in the viewer or the
track editor, says its load in one line once its models are in, and again
after each hot reload,

```text
INFO load: Loaded tests/fixtures/scenes/stylize_props.scene.toml: 8 entities, 2 models, 2 particle effects in 140 ms; no warnings
```

and the window's own notices (saves, reloads, undo, the F4 toggles, the
auto-orbit speed) are info lines of target `viewer`. The default filter is
`warn,rfd=off,load=info,script=info,viewer=info`; `RUST_LOG=load=debug` lists each
file a load reads again. A `--render` still answers `Rendered ...` on stdout
as before.

### Model Previewer Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--distance <f32>` | (auto) | Camera orbit distance |
| `--yaw <deg>` | (auto) | Horizontal camera angle |
| `--pitch <deg>` | (auto) | Vertical camera angle |
| `--target <x,y,z>` | (auto) | Camera look-at point |
| `--fov <deg>` | (auto) | Field of view |
| `--no-animate` | `false` | Disable animation playback |
| `--clip <name>` | (none) | Start with a specific animation clip |
| `--anim-speed <f32>` | `1.0` | Animation playback speed multiplier |
| `--layer <clip[:weight[:mask[:mode]]]>` | (none) | Add an animation layer; repeatable. `mode` is `additive` or `override`, `mask` a root joint name |
| `--sequence <file.sequence.toml>` | (none) | Play a sequence of timestamped animator events |
| `--sequence-loop` | `false` | Loop the sequence regardless of its `loop` setting |
| `--anim-time <s>` | (none) | With `--render`: sample the animation (or replay the sequence) at this time |
| `--pose-bone <joint:x,y,z>` | (none) | Turn a joint in its own frame, in degrees; repeatable. Works without a clip |
| `--show-skeleton` | off | Draw the skeleton over the model, posed joints and everything past them in orange |
| `--render <path>` | (none) | Render to PNG instead of opening a window |
| `--style <name[:k=v,...]>` / `--style-time <s>` | (none) | With `--render`: a registered style over the snapshot, as on `flint render` (also for `.particles.toml`) |
| `--motion-hold <rate>` | off | Hold the pose on ticks of the clip's own time, playing and scrubbing (#871); the Animation panel's **Hold** switch and rate change it live |

### Particle Editor Flags

The camera flags above (`--distance`, `--yaw`, `--pitch`, `--target`, `--fov`), `--render` and `--anim-time` apply to `.particles.toml` files too. In addition:

| Flag | Default | Description |
|------|---------|-------------|
| `--preset <name>` | `sparks` | Preset written when the file does not exist yet: `fire`, `smoke`, `sparks`, `rain` |
| `--anim-time <s>` | `1.0` | With `--render`: simulation time of the snapshot (fixed 1/120 s steps; deterministic) |

### Particle Editor Controls

| Input | Action |
|-------|--------|
| Space / R | Play-pause / restart |
| Home / End, ← / → | Seek start / end, step (Shift: 0.1 s) |
| L, `[` / `]` | Loop, halve / double speed |
| O, G, X, B, H | Auto-orbit, grid, shape gizmos, backdrop, hide UI |
| Ctrl+S, Ctrl+Z / Ctrl+Y, Ctrl+D, Delete, Ctrl+R | Save, undo / redo, duplicate emitter, delete emitter, reload |
| Curve widgets | Drag keys; double-click adds; right-click removes; Shift-drag locks t |

See the [Particle Editor guide](../guides/particle-editor.md).

### Scene Viewer Controls

| Input | Action |
|-------|--------|
| Left-click | Select entity / pick gizmo axis (a press that moves more than a few pixels is a drag, and picks nothing) |
| Left-drag | Orbit camera (or drag gizmo if axis selected; a press on a gizmo handle never orbits) |
| Right-drag | Pan camera |
| Scroll | Zoom |
| W / A / S / D | Orbit camera (when no entity is selected) |
| Q / E | Zoom out / in (when no entity is selected) |
| W / E / R | Switch gizmo mode: translate / rotate / scale (while an entity is selected) |
| Space | Return to the scene's authored `[camera]` framing (viewer default if the scene has none) |
| O | Toggle auto-orbit |
| `[` / `]` | Slow down / speed up auto-orbit |
| Ctrl+S | Save scene to disk: only the fields that differ from the file are written (a prefab's entity into its instance's overrides), and each is a line in `.flint/viewer-edits.jsonl` ([What you changed](../getting-started/viewing.md#what-you-changed)); with nothing changed the file isn't touched |
| Ctrl+Z | Undo the last edit |
| Ctrl+Shift+Z / Ctrl+Y | Redo it |
| Escape | Cancel gizmo drag / exit |
| F1 | Show or hide the list of debug keys (`keys`, the host's) |
| F2 | Toggle the render stats overlay (`stats`) |
| F3 | Toggle normal arrows (`normal_arrows`) |
| F4 | Toggle the **Rendering & Effects** menu (all render and post toggles and parameters, debug shading mode, shadows, an authored-vs-viewer-default post switch, DoF follow, and the live Particles controls; `render_panel`) |

The viewer applies the scene's `[post_process]` block on load; the F4 menu's "authored" switch flips between those values and the viewer defaults. The earlier F1 (debug mode), F2 (wireframe overlay) and F4 (shadows) keys were folded into the F4 menu.

When an entity is selected, a **translate gizmo** appears with colored axis arrows (red = X, green = Y, blue = Z) and plane handles. Click and drag an axis or plane to move the entity. Position changes can be undone/redone and saved back to the scene file.

### Procgen Previewer Controls

The previewer of a mesh or texture spec (`flint edit specs/oak_tree.procgen.toml`):

| Input | Action |
|-------|--------|
| Left-drag, right-drag, scroll, W/A/S/D, Q/E | Orbit, pan, zoom (Q/E zoom a texture too) |
| O, `[` / `]` | Toggle auto-orbit, slower / faster |
| Space | Fit the camera to the mesh; a texture back to 1x, centred |
| R | A new random seed |
| T | Tile the texture |
| Tab | Hide or show the panels |
| Ctrl+S / Ctrl+Shift+S | Save the spec / save it as (only the changed params are written) |
| Ctrl+E | Export the output |
| F1 | Show or hide the list of debug keys (`keys`, the host's) |
| F2 | Wireframe over the mesh, or back to shaded (`wireframe`) |
| F3 | Normal arrows (`normal_arrows`) |
| F4 | The skeleton overlay, for a mesh with a skeleton (`skeleton`) |
| F5 | The next debug shading mode: PBR, wireframe, normals, depth, UV, ... (`debug_mode`) |
| Escape | Exit |

F2 to F5 act on a mesh; with a texture spec the card shows them dimmed. F5
was F1 until #1170 gave F1 one meaning in every tool.

### Spline Editor Controls (`--spline`)

| Input | Action |
|-------|--------|
| Left-click | Select control point |
| Left-drag | Move control point on constraint plane |
| Alt + drag | Move control point vertically (Y axis) |
| Middle-drag | Orbit camera |
| Right-drag | Pan camera |
| Scroll | Zoom |
| O, `[` / `]` | Toggle auto-orbit, slower / faster |
| F1 | Show or hide the list of debug keys; the scene viewer's F2 to F4 work here too |
| Tab / Shift+Tab | Cycle through control points |
| I | Insert a new control point after selected |
| Delete | Remove selected control point |
| Ctrl+S | Save spline to disk |
| Ctrl+Z | Undo |

> **Legacy aliases:** `flint serve`, `flint preview`, `flint gen-preview`, `flint tex-edit`, `flint terrain-edit`, and `flint spline-edit` still work (`serve` has no code of its own: it opens the scene viewer `flint edit <scene>` does). They are hidden subcommands that open the same tools `flint edit` dispatches to; they are omitted from `flint --help`.

## The `asset generate` Command

Generate assets using AI providers:

```bash
flint asset generate texture -d "rough stone wall" --style medieval_tavern
flint asset generate model -d "wooden chair" --provider meshy --seed 42
flint asset generate audio -d "tavern ambient noise" --duration 10.0
```

| Flag | Description |
|------|-------------|
| `-d`, `--description` | Generation prompt (required) |
| `--name` | Asset name (derived from description if omitted) |
| `--provider` | Provider to use: `flux`, `meshy`, `elevenlabs`, `mock` |
| `--style` | Style guide name (e.g., `medieval_tavern`) |
| `--width`, `--height` | Image dimensions for textures (default: 1024x1024) |
| `--seed` | Random seed for reproducibility |
| `--tags` | Comma-separated tags |
| `--output` | Output directory (default: `.flint/generated`) |
| `--duration` | Audio duration in seconds (default: 3.0) |

Generated assets are automatically stored in content-addressed storage and registered in the asset catalog with a `.asset.toml` sidecar. Models are validated against style constraints after generation.

## The `gen` Command

Run a procedural generation spec to produce meshes (GLB) or textures (PNG):

```bash
flint gen specs/oak_tree.procgen.toml -o tree.glb
flint gen specs/stone_wall.procgen.toml -o wall.png
flint gen specs/oak_tree.procgen.toml --dry-run
flint gen specs/oak_tree.procgen.toml --seed 42 -o tree.glb
flint gen specs/oak_tree.procgen.toml --batch 10 --seed-start 0
flint gen --list-generators                 # every generator a spec may name
flint gen --describe tree_v1                # its parameters: type, range, default, what each does
flint gen --describe texture_v1 --json      # the schema itself, and every pipeline op with its ports
```

Before anything is generated, the spec's params are checked against its generator's schema at every depth (`ops[2].columns`, `materials.shell.roughness`): a value the generator can't use (wrong type, out of range, not a choice, a required key missing) stops the run naming the key and what it allows, and a key it would ignore is a warning.

`flint gen` also previews a glyph set: `flint gen fonts/terminal.glyphs.toml -o sheet.png` writes the atlas, each ramp's ink and a specimen as one sheet, and `-o out/` writes the pieces as well. See [Glyph Atlas](../concepts/glyph-atlas.md#looking-at-a-set).

It also bakes a terrain spec, with no window: a `.terrain.toml` is a `terrain_v1` spec (#1168), so `flint gen flint-examples/rolling_meadow.terrain.toml -o terrain/meadow.png` writes the 16-bit heightmap as `terrain/meadow_height.png` and the splat map as `terrain/meadow_splat.png`, the bytes the terrain editor's Ctrl+E export writes. Every flag above works on one: `--register`, `--batch`, `--validate`, `--strict`, `--print-spec` (the same recipe as a `*.procgen.toml`) and the AI steps (`--ai-generator terrain_v1`). See [Terrain](../concepts/terrain.md#generating-terrain-from-a-spec).

| Flag | Default | Description |
|------|---------|-------------|
| `-o, --output <path>` | (derived from spec) | Output file or directory |
| `--seed <u64>` | (from spec) | Override the spec's seed |
| `--dry-run` | `false` | Print estimated cost without generating |
| `--format <fmt>` | (auto) | Force output format: `glb` or `png` |
| `--batch <N>` | (none) | Generate N (at least 1) variants with sequential seeds, named `<stem>_<seed>.<ext>` |
| `--seed-start <u64>` | `0` | First seed of a batch (requires `--batch`) |
| `--register` | `false` | Store output in the content store with a provenance sidecar; reuse a stored result for the same spec + seed |
| `--force` | `false` | With `--register`, regenerate even if this spec + seed is stored |
| `--validate` | `false` | Validate output after generation |
| `--strict` | `false` | Treat warnings as failures |
| `--list-generators` | `false` | List every generator: output, revision, parameter count, whether it can live (no spec) |
| `--describe <generator>` | (none) | Print every parameter a generator reads, grouped by the mode that reads it, a nested table's keys under the key that holds it (`bones[].name`); `texture_v1`'s ops too (no spec). With the global `--json`, either catalog is one document: `--describe`'s is the schema itself |
| `--markdown` | `false` | With `--describe`: Markdown tables, as the flint-procgen README's catalog |
| `--style-guide <path>` | (none) | Style guide TOML for validation constraints |
| `--report <fmt>` | `human` | Validation report format: `human`, or `json` (one object per variant on stdout, with each check's `actual` and `limit`; human output moves to stderr). Requires `--validate` |

AI assistance runs before generation; the agent's feedback goes to stderr:

```bash
ANTHROPIC_API_KEY=... flint gen --ai-create "twisted dead tree" -o tree.glb
flint gen specs/oak_tree.procgen.toml --ai-assist --max-cost 0.50 -o tree.glb
flint gen specs/stone_wall.procgen.toml --ai-refine --reference photo.png -o wall.png
flint gen --cost-report --since 2026-10-01
```

| Flag | Default | Description |
|------|---------|-------------|
| `--ai-create <text>` | (none) | Create a spec from a description, then generate it (no spec file) |
| `--ai-generator <name>` | (guessed) | With `--ai-create`: `tree_v1`, `texture_v1` or `creature_v1` |
| `--ai-assist` | `false` | Interpret the spec, then refine it |
| `--ai-refine` | `false` | Refine against `--reference <image>` |
| `--refine-rounds <N>` | `--ai-iterations` | With `--reference`: most measured refine rounds (1-8). Each round's result is rendered (meshes headlessly) and measured against the reference; the refine asks again while the distance falls and keeps the closest spec |
| `--ai-provider <name>` | `anthropic` | `anthropic` or `mock` (offline, free) |
| `--ai-model <id>` | `claude-opus-5-5` | Model; `.flint/procgen_ai.toml` can set the default |
| `--ai-iterations <N>` | `3` | Rounds per AI step (1-20) |
| `--print-spec` | `false` | Print the final spec as TOML on stdout (the summary moves to stderr) |
| `--save-spec <path>` | (none) | Write the final spec to a file |
| `--max-cost <USD>` | (none) | Refuse any AI call that could take the run past this amount |
| `--cost-report` | `false` | Summarize the project's `.flint/ai_costs.jsonl` by provider, model and spec or asset (the same report as `flint asset costs`) |
| `--since <date>` | (none) | With `--cost-report`: only calls from `YYYY-MM-DD` on |

## The `prefab view` Command

Preview a prefab template in the interactive viewer:

```bash
flint prefab view prefabs/kart.prefab.toml
```

| Flag | Default | Description |
|------|---------|-------------|
| `--prefix <string>` | `"preview"` | Prefix for `${PREFIX}` substitution |
| `--schemas <path>` | manifest, else discovered | Schemas directory (repeatable; see [Common Flags](#common-flags)) |

This command loads the `.prefab.toml` template, performs variable substitution, builds a synthetic scene from the expanded entities, and launches the viewer for visual inspection. Useful for verifying prefab structure and appearance without creating a full scene.

## Common Flags

| Flag | Description |
|------|-------------|
| `--scene <path>` | Path to scene file |
| `--schemas <path>` | Schema directory, repeatable on every command that loads schemas (`play`, `render`, `validate`, `query`, `entity`, `schema`, `edit`, `prefab view`, `flint-player`); later paths override earlier. When given, the flags are the whole list. Without it: the `schemas` list of the project's `flint.toml`; else `schemas/` in the working directory, beside the scene and one level up. A directory named by a flag or the manifest that doesn't exist is an error naming it |
| `--project <dir>` | The project root, instead of the one found by walking up; its `flint.toml` is the manifest read |
| `--format <fmt>` | Output format: `json`, `toml`, or `text` |
| `--fix` | Apply auto-fixes (with `validate`). The exit code is 1 if an error is left afterwards |
| `--strict` | With `validate`: a field no schema declares is an error, not a warning (`[project] strict = true` in `flint.toml` does it for every run) |
| `--dry-run` | Preview changes without applying |

## Usage

```bash
# Get help
flint --help
flint <command> --help

# Examples
flint init my-game
flint edit levels/tavern.scene.toml              # Interactive scene viewer
flint edit models/character.glb --watch           # Model previewer
flint play levels/tavern.scene.toml
flint render levels/tavern.scene.toml -o shot.png
flint gen specs/oak_tree.procgen.toml -o tree.glb
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml
```
