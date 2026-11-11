# Assets

Flint uses a content-addressed asset system with SHA-256 hashing. Every imported file is identified by its content hash, which means identical files are automatically deduplicated and any change to a file produces a new, distinct hash.

## Content Addressing

When you import a file, Flint computes its SHA-256 hash and stores it under a content-addressed path:

```
.flint/assets/<first-2-hex>/<full-hash>.<ext>
```

This means:
- **Deduplication** --- importing the same file twice stores it only once
- **Change detection** --- if a source file changes, its hash changes, and the new version is stored separately
- **Integrity** --- the hash verifies the file hasn't been corrupted

## The Project Root

The content store (`.flint/assets`), the procgen provenance index (`.flint/procgen`), the job store (`.flint/jobs`), the project config (`.flint/config.toml`) and the sidecar catalog (`assets/`) all live at the **project root**, whichever folder of the project a command runs from. Flint finds the root the way cargo finds a workspace: it walks up from the working directory to the nearest directory holding

1. a `flint.toml` file (the [project manifest](../guides/building-a-game-project.md#the-project-manifest)),
2. a `.flint/` directory (not the one in your home directory, which holds the global config), or
3. `schemas/` beside `scenes/` or `levels/` --- the layout `flint init` makes and the game repos keep.

A Flint engine checkout is passed through when a project sits above it, so `flint gen ... --register` run inside a game's `engine/` subtree registers into the game's store. With no marker anywhere, the working directory is the root. `--project <dir>` (a global flag on every command) names the root outright (reading that folder's `flint.toml` if it has one), and `flint gen --register` prints the store it uses:

```
Store:      .flint\assets (project ., by schemas/ + scenes/ or levels/)
```

`flint asset resolve` uses the project of the scene it is given.

## Where the Engine Looks for a File

A scene names most of its assets by a short name: `effect = "campfire"`, not a path. The engine turns a name into a file with **one search rule, the same for every kind of asset** that has moved onto it (the table below; the rest still have searches of their own and move over kind by kind, [ADR 0114](https://github.com/chrischaps/Flint/blob/main/docs/adr/0114-project-manifest-and-one-resolver.md)). It looks, in this order, and takes the first file it finds:

1. **beside the scene**: the kind's folder in the scene's own directory;
2. **beside each directory above the scene**, one at a time, up to and including the [project root](#the-project-root);
3. **in the mounted engine**: the kind's folder under `<root>/engine/`, where a game keeps the engine as a subtree.

So a file near a scene overrides one of the same name further up, a scene any number of folders deep finds what the project keeps at its root, and the engine's own assets are a last resort a game can always override.

```
my_game/                       flint.toml here: the project root
├── particles/                 3rd   the project's effects
├── engine/particles/          4th   the engine's, last
└── scenes/
    ├── particles/             2nd
    └── act1/
        ├── particles/         1st   beside the scene: overrides everything
        └── cave.scene.toml
```

At each of those directories `D`, the file tried is `D/<folder>/<name><extension>`:

| Kind | Folder | Extension | A scene writes |
|------|--------|-----------|----------------|
| `particles` | `particles/` | `.particles.toml` | `particle_effect.effect = "campfire"`, `play_effect("campfire", ...)` |

A name that already ends in the kind's extension is taken as written (`campfire.particles.toml`), and a name may carry a subfolder (`fx/campfire` is `particles/fx/campfire.particles.toml`). Two details at the edges, the same for every kind:

- **No project marker.** A scene in a folder with no `flint.toml`, no `.flint/` and no `schemas/` + `scenes/` layout above it is its own project root. The engine then also looks one level above the scene, as every search did before there were projects.
- **A scene outside the root** (`--project` naming another folder) is searched beside itself, then at the root, then in the engine.

Inside an engine checkout the root already is the engine, so there is no third place. In a web bundle the rule is the same over the bundle's files, where `bundle.toml`'s `[mount]` puts the engine's files under `engine/`.

### `flint asset where`

Ask the engine which file it would load, and where it looked first:

```
$ flint asset where particles campfire --scene scenes/act1/cave/deep.scene.toml
particle effect 'campfire'
  project  C:/games/ember-game (flint.toml)
  scene    scenes/act1/cave
  loads    particles/campfire.particles.toml
  tried before it:
    scenes/act1/cave/particles/campfire.particles.toml
    scenes/act1/particles/campfire.particles.toml
    scenes/particles/campfire.particles.toml
```

When there is no such file it lists every path it tried and exits with status 1. A loader that can't find an asset prints the same list in its warning:

```
$ flint asset where particles ice --scene scenes/act1/cave/deep.scene.toml
particle effect 'ice'
  project  C:/games/ember-game (flint.toml)
  scene    scenes/act1/cave
  loads    nothing: no such file. Tried:
    scenes/act1/cave/particles/ice.particles.toml
    scenes/act1/particles/ice.particles.toml
    scenes/particles/ice.particles.toml
    particles/ice.particles.toml
    engine/particles/ice.particles.toml
Error: particle effect 'ice' not found (5 places tried)
```

| Argument | Meaning |
|----------|---------|
| `<kind>` | A kind from the table above. `flint asset where --help` lists them |
| `<name>` | The name a scene would use |
| `--scene <file>` | The scene that names it. Without one, a scene in the working directory is assumed |
| `--json` | One JSON document on stdout: `kind`, `name`, `scene_dir`, `project { root, found }`, `found` (a path or `null`), `tried` (the paths missed, in order) and `order_change`. Paths use forward slashes on every platform. The exit status is still 1 when nothing was found |

For one release, where a kind's old search would have picked a **different file** than the one rule does, the engine says so: a `note` line here (and `order_change` in the JSON), and a warning on stderr when a scene loads, once for each such file.

## Asset Catalog

The asset catalog is a searchable index of all imported assets. Each entry tracks:

- **Name** --- a human-friendly identifier (e.g., `tavern_chair`)
- **Hash** --- the SHA-256 content hash
- **Type** --- asset type (`mesh`, `texture`, `material`, etc.)
- **Tags** --- arbitrary labels for organization and filtering
- **Source path** --- where the file was originally imported from

## Importing Assets

Use the CLI to import files into the asset store:

```bash
# Import a glTF model with name and tags
flint asset import models/chair.glb --name tavern_chair --tags furniture,medieval

# Browse the catalog
flint asset list --type mesh

# Check asset references in a scene
flint asset resolve levels/tavern.scene.toml --strategy strict
```

## glTF/GLB Import

The `flint-import` crate provides full glTF/GLB support, extracting:

- **Meshes** --- vertex positions, normals, texture coordinates, and indices
- **Materials** --- PBR properties (base color, roughness, metallic, emissive)
- **Textures** --- embedded or referenced image files

Imported meshes are rendered by `flint-render` with full PBR shading.

## Resolution Strategies

When a scene references assets, Flint can resolve them using different strategies:

| Strategy | Behavior |
|----------|----------|
| `strict` | All referenced assets must exist in the catalog. Missing assets are errors. |
| `placeholder` | Missing assets are replaced with placeholder geometry. Useful during development. |
| `ai_generate` | Missing assets are generated via AI providers (Flux, Meshy, ElevenLabs) and stored. |
| `human_task` | Missing assets produce task files for manual creation by an artist. |
| `ai_then_human` | Generate with AI first, then produce review tasks for human approval. |

The `ai_generate`, `human_task`, and `ai_then_human` strategies are part of the [AI Asset Generation](ai-generation.md) pipeline.

## Asset Sidecar Files

Each asset in the catalog has a `.asset.toml` sidecar file storing metadata:

```toml
[asset]
name = "tavern_chair"
type = "mesh"
hash = "sha256:a1b2c3..."
source_path = "models/chair.glb"
tags = ["furniture", "medieval"]
```

## Runtime Catalog Resolution

The player can load the asset catalog at startup for name-based asset resolution. When an entity references an asset by name, the resolution chain is:

1. Look up the name in the `AssetCatalog`
2. If found, resolve the content hash
3. Load from the `ContentStore` path (`.flint/assets/<hash>`)
4. Fall back to file-based loading if not in the catalog

This allows scenes to reference both pre-imported and AI-generated assets by name without hardcoding file paths.

## Further Reading

- [Importing Assets](../guides/importing-assets.md) --- step-by-step import guide
- [AI Asset Generation](ai-generation.md) --- AI-powered asset creation pipeline
- [Schemas](schemas.md) --- the `material` component schema for PBR properties
- [File Formats](../formats/overview.md) --- asset sidecar TOML format reference
