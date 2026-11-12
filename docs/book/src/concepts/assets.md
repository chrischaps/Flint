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

A project is a folder: a game's repository, with its scenes, schemas and assets under it. Its top is the **project root**, and everything on this page is stated from it. The asset search below ends there. The schema directories a `flint.toml` names are paths from it. The content store (`.flint/assets`), the procgen provenance index (`.flint/procgen`), the job store (`.flint/jobs`), the project config (`.flint/config.toml`) and the sidecar catalog (`assets/`) live there, whichever folder of the project a command runs from.

Flint finds the root the way cargo finds a workspace: it walks up **from the scene** a command or the player is given (from the working directory when there is no scene) to the nearest directory holding

1. a `flint.toml` file (the [project manifest](../guides/building-a-game-project.md#the-project-manifest)),
2. a `.flint/` directory (not the one in your home directory, which holds the global config), or
3. `schemas/` beside `scenes/` or `levels/` --- the layout `flint init` makes and the game repos keep.

A Flint engine checkout is passed through when a project sits above it, so `flint gen ... --register` run inside a game's `engine/` subtree registers into the game's store. With no marker anywhere, the working directory is the root. `--project <dir>` (a global flag on every command) names the root outright (reading that folder's `flint.toml` if it has one), and `flint gen --register` prints the store it uses:

```
Store:      .flint\assets (project ., by schemas/ + scenes/ or levels/)
```

`flint asset resolve` uses the project of the scene it is given.

The player asks once a scene, and again after a transition, since the next scene may belong to another project. A web bundle is searched the same way: `flint.toml` is packed with the game, and the player reads it out of the bundle.

## Where the Engine Looks for a File

A scene names most of its assets by a short name: `effect = "campfire"`, not a path. The engine turns a name into a file with **one search rule, the same for every kind of asset** ([ADR 0114](https://github.com/chrischaps/Flint/blob/main/docs/adr/0114-project-manifest-and-one-resolver.md); the table below is every kind). It looks, in this order, and takes the first file it finds:

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

At each of those directories `D`, the file tried is `D/<folder>/<name><extension>`, for each of the kind's folders in turn and then each of its extensions. A kind is only that: its folders and its extensions.

| Kind | Folder | Extension | A scene writes | Built in |
|------|--------|-----------|----------------|----------|
| `particles` | `particles/` | `.particles.toml` | `particle_effect.effect = "campfire"`, `play_effect("campfire", ...)` | |
| `models` | `models/` | `.glb`, then `.gltf` | `model.asset = "barrel"` | |
| `textures` | `D` itself, then `textures/` | a file name as written; a name with no extension is tried bare, then `.png`, then `_color.png` | `material.texture = "wall.png"` or `"wall"`, `sprite.texture = "hero.png"` | |
| `sprites` | `sprites/`, then `D` itself | a file name as written | `draw_sprite(x, y, w, h, "heart.png")` | |
| `animations` | `animations/` | `.anim.toml` | every clip is loaded; `animator.clip = "door_swing"` names one by the `name` inside its file | |
| `sprite-animations` | `animations/` | `.sprite.toml` | every file is loaded; a `sprite_animator` names a clip inside one | |
| `sequences` | `animations/` | `.sequence.toml` | every file is loaded; `animator.sequence = "intro"`, `play_sequence` | |
| `audio` | `D` itself, then `audio/` | a file name as written (`.ogg`, `.wav`, `.mp3`, `.flac`); a name with no extension tries those four | `audio_source.file = "audio/fire.ogg"`; every file in `audio/` is preloaded for `play_sound` | |
| `scripts` | `scripts/` | a file name as written (`.rhai`) | `script.source = "hud.rhai"` or `"ui/menu.rhai"`; `import "ui_kit"` is the script `lib/ui_kit.rhai` | |
| `specs` | `specs/`, then `models/`, each with its subfolders | `.procgen.toml`; found by the `[meta] name` inside, not the file's name | every spec the scene can see is indexed; `model.asset = "oak_tree"`, `material.texture = "stone_wall"` ask for one | |
| `prefabs` | `prefabs/` | `.prefab.toml` | `[prefabs.player] template = "kart"` | |
| `wooze` | `wooze/` | `.wooze.toml` | `[wooze] preset = "misty"`, `load_living_preset("misty")`, the Living Props panel | |
| `files` | `D` itself | a path as written | `terrain.heightmap`, `terrain.splat_map`, `grass_ground.mask`, `spline.file`, `[environment] skybox`, `[scene] input_config` (and `config/input.toml` when a scene names none) | |
| `particle-textures` | `particles/`, then `D` itself | a file name as written | an effect's or a `particle_emitter`'s `texture = "spark.png"` | |
| `scenes` | `D` itself | a path as written | `load_scene("levels/two.scene.toml")`: the target of a transition, from the current scene | |
| `palettes` | `palettes/`, then `D` itself | `.palette.toml` | `[post_process.styles.ascii] palette = "dusk"` (or a path, `"art/dusk.palette.toml"`), `--ascii-palette`, a per-entity style's `palette` | `cga`, `cga-warm`, `cga16`, `phosphor`, `amber`, `mono`, `paper`; a file of the name wins |
| `glyph-sets` | `fonts/`, then `D` itself | `.glyphs.toml` | a style's `font = "terminal"`, `--ascii-font`, `ui_text.font = "terminal"`, a `.font.toml`'s `glyphs` | `builtin`, which is never searched for |
| `fonts` | `D` itself, then `fonts/` | `.font.toml` | `ui_text.font = "fonts/ui.font.toml"` | |
| `font-faces` | `fonts/` | a file name as written (`.ttf`, `.otf`) | every one is loaded for the script HUD, under its file stem; a `fonts/fonts.toml` alias names one by `file` | |
| `looks` | `styles/`, then `D` itself | `.look.toml` | `[post_process] look = "dusk"`, `--look dusk`, `set_look("dusk")`, or a path, `--look out/tuned.look.toml` | `ascii_noir`, `matrix`, `ink_wash`, `graphite`, `watercolor`, `coloured_pencil`, `blueprint`, `charcoal`, `rotoscope`; a file of the name wins |
| `papers` | `papers/`, then `D` itself | `.png`, then `.jpg`, `.jpeg` | `paper_image = "cold_press"` or `"scans/linen.jpg"` (pencil, watercolor) | |
| `strokes` | `strokes/`, then `D` itself | `.png`, then `.jpg`, `.jpeg` | `stroke_image = "graphite"` (pencil) | |
| `styles` | `styles/` | `.shader.toml` | every user style the scene can see is registered; `[post_process.styles.glow]` names one | |

`flint asset where --help` prints this table from the engine's own, and the first column is the word `flint asset where <kind>` takes. This is the only statement of the rule in the book: every other page that mentions where a file is found points here.

### Schemas: the rule beside it

Schemas are not searched for by name; a run loads whole directories of them, a later one overriding an earlier. Which directories is decided once, the same way by `flint render`, `edit`, `play`, `validate`, `query`, `entity`, `schema`, the viewer and `flint-player`:

| Asked | The directories | A directory that isn't there |
|-------|-----------------|------------------------------|
| `--schemas <dir>` flags, if any | exactly those, in the order given; nothing is added | an error naming it |
| else `flint.toml`'s `[project] schemas` | each one under the project root, in the order listed | an error naming it and the manifest |
| else discovery | `schemas/` in the working directory, beside the scene, and one level above the scene, whichever exist | not an error: the list may be empty |

So a game whose manifest says `schemas = ["engine/schemas", "schemas"]` needs no flag on any command. [Schemas: Which directories load](schemas.md#which-directories-load) has the details.

### Paths from the project root

A few things are one path under the root and no search:

| What | Where |
|------|-------|
| `load_ui("ui/hud.ui.toml")`, a style sheet's `tokens = "ui/theme.toml"` | `<root>/ui/hud.ui.toml` |
| `load_data("data/karts.toml")` | `<root>/data/karts.toml` |
| a `music_session`'s `manifest`, `chart` and configs | `<root>/<path>` |
| the save file of a game with no `id` | `<root>/save/persist.toml` (a game with an id saves under the OS user directory: [Where saves live](scripting.md#where-saves-live)) |
| the procgen disk cache | `<root>/.flint/procgen-cache/` |
| the project-local input override | `<root>/.flint/input.user.toml` |
| `flint.toml`'s `scene`, `input` and `schemas` | `<root>/<path>` |

The root is the same for every scene of the project, so a scene two folders deep reads the same `ui/` as one in `scenes/`. The save file is settled once, when the game starts, and no transition moves it.

**No project marker.** A scene with no `flint.toml`, no `.flint/` and no `schemas/` + `scenes/` layout above it takes its folder's parent for these paths (`loose/levels/a.scene.toml` reads `loose/ui/`), which is what the engine did for every scene before there were projects.

**For one release.** Before the project was asked, the root for these paths was guessed as the scene folder's parent, for every scene. The two are the same folder for a scene directly in `scenes/` or `levels/`. Where they differ (a scene two folders deep, or a `.flint/` folder beside the scenes) a file that exists only at the old place is still read from there, with a warning that says where it belongs: UI documents, data files, a music session's manifest, and the save file, which is written where saves live now from then on. The project-local input override is likewise still read from a `.flint/` beside the scene.

**Names.** A name that already ends in one of the kind's extensions is taken as written (`campfire.particles.toml`, `barrel.gltf`), and a name may carry a subfolder (`fx/campfire` is `particles/fx/campfire.particles.toml`). Textures, sprites and sounds are named by **file name**: a name with a dot in its last part is that file, whatever follows the dot (`sky.hdr`, `wall.jpg`). Extensions are matched as they are spelled, for every kind: `hit.OGG` is not found by the preload, and `barrel.GLB` in `model.asset` becomes `barrel.GLB.glb`.

**A folder of "`D` itself".** Textures, sprites and sounds may sit loose beside a scene, or be named by a path from it (`audio/fire.ogg`, `ui/heart.png`), so the directory of the walk is one of their folders. That is why `audio_source.file = "audio/fire.ogg"` finds `audio/fire.ogg` beside the scene and beside every directory above it.

**As written, first.** A name that is a path of its own is tried as written before any of the above, with the kind's extensions:

- starting with `./` or `../`, from the scene's directory: `texture = "../shared/wall.png"`, `asset = "./local/barrel"`. If that misses, the name goes through the search like any other.
- absolute (`/art/wall.png`, `C:/art/wall.png`): that one file, and nowhere else.

No path is tried twice, and the list a miss prints is in the order tried, the as-written paths first.

**Kinds loaded as a whole.** Animation clips, sequences, sprite animations, the `audio/` preload, procgen specs, user styles and the HUD's font files load every file the scene can see, from every one of those folders, a nearer file hiding a farther one of the same name. Only a kind's named folders are listed, never the scene's own directory, and not their subfolders, except for scripts and specs, whose folders are listed with their subfolders (a folder's own files before its subfolders').

**Named inside the file.** A procgen spec is asked for by the `[meta] name` it states, whatever its file is called: `specs/trees/big_oak.procgen.toml` with `name = "oak_tree"` is the spec `oak_tree`. The places and their order are the same as for every kind; the nearest file that states a name is that name's spec, a file that states none is not a spec, and of two in one folder stating the same name the first by file name wins. A miss lists the folders read, not files. `flint asset where specs oak_tree` answers by that name.

**Scripts.** Each script is found on its own, so a scene with a `scripts/` folder beside it still finds every script and every `import` in the project's `scripts/`. A script a chunk brings in is searched for from the chunk's folder first, then from the scene's.

**Files named by a path.** A heightmap, a splat map, a grass mask, a spline file, a skybox, a scene's `input_config` and a transition's target are written as paths (`terrain/height.png`), and the path is tried from the scene's directory, then from each directory above it. A transition's target that isn't found that way is still opened as written, which is a path from the working directory.

**Saving a wooze preset.** The Living Props panel and `save_living_preset(name)` write `<name>.wooze.toml` into the nearest `wooze/` folder that exists between the scene and the project root (never the mounted engine's), and into a new `wooze/` beside the scene when there is none. A preset saved there is one the scene finds.

**Built in, after the search.** Three kinds have names the engine carries itself: the palettes `cga`, `cga-warm`, `cga16`, `phosphor`, `amber`, `mono` and `paper`; the looks `ascii_noir`, `matrix`, `ink_wash`, `graphite`, `watercolor`, `coloured_pencil`, `blueprint`, `charcoal` and `rotoscope`; and the glyph set `builtin`. For palettes and looks the search runs first and **a file it finds wins over a built-in of the same name**, so a game's `palettes/amber.palette.toml` replaces the built-in amber for every scene that can see it; the built-in is what is used when no file is found. The glyph set `builtin` is never searched for: it always means the engine's own, and a file named `builtin.glyphs.toml` is not reachable by that name. Paper and stroke images have no built-ins (a style with no image draws its procedural paper and strokes). `flint asset where palettes amber` says when the answer is a built-in. The lists the F4 style editor and `flint render --list-styles` offer for a `palette`, `font`, `paper_image` or `stroke_image` are the built-ins and then exactly the files this search sees from the scene.

**Looks, palettes, glyph sets and images named by a path.** Each of these may be written as a path ending in the kind's extension (`--look out/tuned.look.toml`, `palette = "art/dusk.palette.toml"`, `paper_image = "scans/linen.jpg"`). The path is tried from the scene's directory and then from each directory above it, like any name, which is why "`D` itself" is one of their folders. It is **not** tried from the working directory: a path typed after `--look` or `--ascii-palette` is found when it leads from the scene's folder or from a folder above it (the project root among them, which is where commands are usually run from), and not otherwise. A look path ending `.toml` but not `.look.toml` is searched the same way, as a file named by its path.

**Files a description names beside itself.** A glyph set's `[font] file`, a user style's WGSL and its `[images]`, and (after the texture search misses) a `.font.toml`'s `texture` are read from the folder of the file that names them. That is not a search: wherever the description was found, its own files are beside it.

**User styles of one name.** Two `styles/` folders may each hold a `glow.shader.toml`. A scene gets the one nearest to it, as with every asset; inside one folder a style is its file's name, so there is only one. The registry of styles is one for the whole run, so when the next scene's nearest `glow` is another file, that file takes the name over (as an edit to the file would), and a style that only an earlier scene's folders had stays registered.

**Models and textures have two sources ahead of the files.** A name in the [asset catalog](#asset-catalog) resolves to its stored file first, and the player then asks for a [procgen spec](procgen.md) of the same name; the search above is for the file, and is what runs when neither has it. A miss says so under the paths: `and before the files: the asset catalog, a procgen spec of the same name`.

Two details at the edges, the same for every kind:

- **No project marker.** A scene in a folder with no `flint.toml`, no `.flint/` and no `schemas/` + `scenes/` layout above it is its own project root. The engine then also looks one level above the scene, as every search did before there were projects.
- **A scene outside the root** (`--project` naming another folder) is searched beside itself, then at the root, then in the engine.

**The mounted engine.** A game keeps the engine as a subtree at `<root>/engine/`, and that folder is the search's last place: `engine/models/`, `engine/scripts/`, and so on. It is a fixed place, not a setting. Inside an engine checkout the root already is the engine, so there is no third place. In a web bundle the rule is the same over the bundle's files, where `bundle.toml`'s `[mount]` puts the engine's files under `engine/`.

### `flint asset where`

Ask the engine which file it would load, and where it looked first:

```
$ flint asset where models barrel --scene scenes/act1/cave/deep.scene.toml
model 'barrel'
  project  . (flint.toml)
  scene    scenes/act1/cave
  loads    models/barrel.glb
  tried before it:
    scenes/act1/cave/models/barrel.glb
    scenes/act1/cave/models/barrel.gltf
    scenes/act1/models/barrel.glb
    scenes/act1/models/barrel.gltf
    scenes/models/barrel.glb
    scenes/models/barrel.gltf

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

A palette, look or glyph set the engine has built in is not a failure: with no file of the name, the answer is the built-in, with the places a file would have been found first.

```
$ flint asset where palettes amber --scene scenes/act1/cave/deep.scene.toml
palette 'amber'
  project  . (flint.toml)
  scene    scenes/act1/cave
  loads    the built-in palette 'amber': no file shadows it. Tried:
    scenes/act1/cave/palettes/amber.palette.toml
    scenes/act1/cave/amber.palette.toml
    scenes/act1/palettes/amber.palette.toml
    scenes/act1/amber.palette.toml
    scenes/palettes/amber.palette.toml
    scenes/amber.palette.toml
    palettes/amber.palette.toml
    amber.palette.toml
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
| `--json` | One JSON document on stdout: `kind`, `name`, `scene_dir`, `project { root, found }`, `found` (a path or `null`), `tried` (the paths missed, in order), `order_change` and `built_in` (no file, and the engine has one of that name built in). Paths use forward slashes on every platform. The exit status is still 1 when nothing was found and nothing is built in |

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
