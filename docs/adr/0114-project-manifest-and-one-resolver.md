# ADR 0114: A project manifest and one resolver for schemas, roots and assets

Status: accepted, 2026-11-10 (#706; audit epic #672)

## Context

The engine has no single idea of "a project". Each command and each loader
works out for itself where the project is and where to look, and the answers
differ (line numbers at `54f1f6ba`):

- **Schemas, five behaviours.** `flint query` hard-codes `schemas` in the
  working directory. `flint entity` takes one directory. `flint edit <scene>`
  takes only the first `--schemas` (#704). `flint schema` takes several and
  errors if none exist. `render`, `play`, `validate` and the spline editor
  copy-paste a block that adds `discover_schema_dirs`, which looks beside the
  scene and one level up and never finds `engine/schemas`
  (`crates/flint-schema/src/registry.rs:13-27`). So every game command needs
  `--schemas engine/schemas --schemas schemas`, and forgetting it gives a
  silently different world.
- **Project root, three guesses.** `player_app/persist.rs:33-50` (the scene
  directory's parent, always), `flint-script/src/api.rs:5177-5193` (a copy),
  and `player_app/init.rs:1057-1073` (the parent only if it holds `schemas/`
  or `specs/`).
- **Assets, about fifteen searches.** Models, textures, fonts, particles,
  scripts, specs, sprites, looks, palettes, glyph sets, paper images, prefabs,
  animations and audio each have a hand-written "scene directory, then its
  parent" with its own precedence. Specs look in the parent first; scripts
  take the first `scripts/` directory that exists, so one beside the scene
  hides the project's. A not-found message never says where the engine looked.
- **A marker nobody reads.** `flint_asset::Project::discover`
  (`crates/flint-asset/src/project.rs`) already walks up to a `flint.toml`,
  a `.flint/` directory or the `schemas/` + `scenes/` layout, and passes
  through an `engine/` subtree to the game above it. It is tooling only, on
  `std::fs`; the player never calls it, and its header says of `flint.toml`
  "nothing reads it yet".
- `bundle.toml` (#106) already holds project facts: `name`, the start
  `scene`, `[web]` and `[mount]` (`"engine/schemas" = "../schemas"`). The
  input config already carries a `game_id`, which the web uses to key saves.

Open bugs that are symptoms: #312 (a transition keeps the first scene's
directory), #426 (which same-named user style wins), saves that move with the
current scene (#726).

## Decision

1. **`flint.toml` at the project root is the manifest.** Minimum content:

   ```toml
   [project]
   id = "float"                       # saves, web bundle id, user files
   schemas = ["engine/schemas", "schemas"]   # in override order
   scene = "scenes/main.scene.toml"   # what `flint play` opens with no argument
   input = "config/input.toml"
   ```

   Every key is optional. With no manifest the engine behaves as it does now
   (the `Layout` and `Fallback` cases of `Project::discover`).

2. **One `Project` type in `flint-core`, over the vfs.** It moves down from
   `flint-asset` so the player and the web build can use it:
   `Project { root, id, schemas, found }`, made by
   `Project::discover(scene_or_dir)` once per run, and again on a scene
   transition into a different root. `flint-asset` keeps its `.flint/` store
   paths as an extension of it. On the web the manifest is packed in the
   bundle and read through the vfs like everything else.

3. **One schema resolver.** `project.schema_registry(extra_flags)`: `--schemas`
   flags if given, else the manifest's list, else today's discovery. Every
   command and the viewer call it. `--schemas` stays, as an override.

4. **One asset resolver.**

   ```rust
   project.resolve(AssetKind::Particles, "fire", &scene_dir)
       -> Result<PathBuf, NotFound { kind, name, tried: Vec<PathBuf> }>
   ```

   with a single precedence for every kind: the scene's own directory, then
   each directory from the scene up to the project root, then the mounted
   engine directories. Each kind names its folder (`models/`, `particles/`,
   `scripts/`, ...) and its extensions; nothing else differs per kind. A
   not-found warning prints `tried`. `flint asset where <kind> <name>
   [--scene x]` prints the same answer for a person or an agent.

5. **Saves and user files key on `project.id`**, under the OS user directory
   (`user_files`), with a one-time read of the old in-tree path (#726).

## Steps

One agent owns #706 and carries out steps 1 to 3 under its one claim, a PR a
step (or several, where a step says so): `Part of #706` on each PR but the
last, `Closes #706` on the last. The ticket's note has the details. A step
that names another ticket is that ticket's work, and waits its turn through
the tickets' blocked-by links. The steps get no sub-issues.

1. `Project` in `flint-core` with `discover` and the manifest parse; the CLI's
   `--project` override; `flint-asset` re-exports it. No behaviour change.
2. The schema resolver, replacing the five (closes #704's cause).
3. `resolve` for one kind (particles), with the `tried` list; then the other
   kinds, one PR per two or three kinds, each with a nested-scene test.
4. `flint init` writes the manifest (#707). Saves move (#726). `flint check
   <dir>` takes a project (#687).

## Alternatives considered

- **Make `bundle.toml` the manifest.** It is nearly one already. But its name
  says "packing rules", games that never ship to the web would need it, and
  its `exclude` list is not project identity. Keeping two files costs a few
  duplicated keys (`name`/`id`, `scene`); see the first decision below.
- **Keep per-kind searches, but share one helper.** Removes the copy-paste
  and not the disagreement: each caller would still choose its own order.
- **Root-first precedence** (project, then scene). Simpler to explain, but a
  scene could no longer carry a private override beside itself, which the
  demos and the film sets do.

## Consequences

- An agent in a game repo runs `flint render scenes/x.scene.toml` with no
  schema flags, and `flint query` can see engine schemas at all.
- Search order changes for kinds that differ from the single rule today:
  specs (parent first) and scripts (first directory wins). A scene that
  relied on the old order resolves differently. The resolver should log, for
  one release, when the old rule would have picked another file.
- A scene nested two folders deep finds project assets. Today it doesn't.
- The CLI gains one concept to document and loses a flag from every example.

## Decisions

Chris accepted the recommendations on 2026-11-10:

1. **Two files.** `bundle.toml` stays as the packing rules. `flint bundle` reads
   `id` and `scene` from `flint.toml` when `bundle.toml` omits them.
2. **Precedence** is the scene's directory first, then upward to the project
   root, then the mounted engine directories, for every kind.
3. **The change of order is logged for one release**: where the old per-kind
   rule would have picked a different file, the resolver says so. Then the
   log goes, with a changelog entry (#765).
