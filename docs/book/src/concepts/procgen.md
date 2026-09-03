# Procedural Generation

The `flint-procgen` crate produces game-ready assets from small TOML specs: trees as glTF meshes with LOD chains, PBR texture sets from pattern parameters or a node graph, and skinned creatures from a bone hierarchy. Generation is deterministic (same spec, same seed, same bytes), runs from the CLI with `flint gen`, previews live in `flint edit`, and resolves on demand inside the player so a scene can reference a spec by name as if it were a file on disk.

Procgen is the offline, rule-driven half of asset creation; [AI asset generation](ai-generation.md) is the other half. They share the content store and the provenance sidecars.

## A Spec

A spec is a `*.procgen.toml` file with four parts: which generator, metadata, how to seed, and the generator's parameters.

```toml
generator = "tree_v1"

[meta]
name = "oak_tree_01"
description = "A sturdy oak with spreading branches"
version = "1.0.0"
tags = ["tree", "vegetation", "oak"]

[seed]
mode = "fixed"
value = 42

[params]
trunk_height = 4.5
trunk_radius_base = 0.35
branch_method = "lsystem"
branch_levels = 3
crown_shape = "sphere"
crown_radius = 3.5
leaf_style = "billboard"
leaf_color_base = "#2D8C0D"
bark_color_base = "#573214"

[[lod]]
level = 0
target_triangles = 8000

[[lod]]
level = 1
target_triangles = 2000
```

| Section | Field | Description |
|---------|-------|-------------|
| top | `generator` | Registered generator type name |
| `[meta]` | `name` | Unique name. This is what scenes and the resolver look up |
| | `description`, `version`, `tags` | Search and provenance metadata |
| `[seed]` | `mode` | `fixed` (use `value`), `random` (fresh seed every run), `derived` (hash `derive_from`) |
| `[params]` | | Opaque table; each generator defines and validates its own keys |
| `[[lod]]` | `level`, `target_triangles` | Optional decimation targets, level 0 is full detail |

Every generator publishes a JSON Schema for its `params` table, which is what the previewer uses to build its parameter editor and what `flint gen --dry-run` validates against.

## Generators

### `tree_v1`

Trunk, branches, leaves and bark in one pass. The trunk is a noise-displaced tapered tube (`trunk_height`, `trunk_radius_base`, `trunk_radius_top`, `trunk_segments`, `radial_segments`, `trunk_curve_noise`). Branches come from one of two methods: `branch_method = "lsystem"` with `lsystem_iterations` and `lsystem_angle_variation`, or `"space_colonization"`, which grows toward attractor points filling a crown of `crown_shape` (`sphere`, `ellipsoid`, `hemisphere`), `crown_radius` and `crown_height_ratio`. `branch_levels`, `branch_angle_min/max`, `branch_length_falloff`, `branch_radius_falloff` and `branch_radial_segments` shape the hierarchy. Leaves are `leaf_style = "billboard"` quads placed `leaves_per_tip` at each branch tip plus `leaf_along_branch_density` along branches deeper than `leaf_along_branch_min_depth`, with `leaf_size`, `leaf_size_variation`, `leaf_spread_radius`, `leaf_color_base` and `leaf_color_variation`. Bark gets a generated normal map (`bark_color_base`, `bark_roughness`, `bark_normal_strength`, `bark_normal_resolution`). With `[[lod]]` levels the output is a mesh chain, exported to one GLB.

### `texture_v1`

Produces an image set, by default albedo, normal and roughness (`output_maps`), at `width` by `height`, optionally `seamless`. Two routes:

**Patterns.** `pattern` selects a base-shape producer whose per-pixel field (cell ids, heights, edge distances) is turned into PBR maps by shared derivation parameters: `base_color`, `color_variation`, `mortar_color`, `mortar_threshold`, `roughness_base`, `roughness_variation`, `roughness_mortar`, `normal_strength`, `detail_scale`, `detail_strength`.

| Pattern | Good for |
|---------|----------|
| `voronoi_brick` | Stone walls, cobbles, cracked earth |
| `perlin_organic` | Layered rock, dirt, bark, natural surfaces |
| `tiling_grid` | Manufactured tiles, bricks, panels |

**Pipeline.** `pattern = "pipeline"` switches to a composable node graph: an ordered `[[params.ops]]` list where each op reads named fields and writes named outputs.

```toml
[params]
pattern = "pipeline"
width = 512
height = 512
seamless = true

[[params.ops]]
type = "voronoi_texture"
output = "membrane"
scale = 8.0
feature = "smooth_f1"

[[params.ops]]
type = "map_range"
input = "membrane"
output = "veins"
from_min = 0.0
from_max = 0.3
interpolation = "smootherstep"
```

The op set: `brick_grid`, `voronoi_grid`, `domain_warp`, `cell_height`, `cell_bulge`, `noise_layer`, `blend`, `mortar_groove`, `cell_color`, `mortar_color`, `derive_normal`, `cell_roughness`, `math`, `map_range`, `color_ramp`, `checker_texture`, `gradient_texture`, `wave_texture`, `white_noise`, `voronoi_texture`, `musgrave_texture`, `invert`, `brightness_contrast`, `hsv_adjust`, `gamma`, `clamp`, `blur`, `sharpen`, `edge_detect`, `edge_erode`. Each op has its own parameter schema; the texture pipeline editor exposes them all.

### `creature_v1`

A skinned mesh from a data-driven skeleton. Everything anatomical is in the spec; the generator only knows bones, shapes and chains.

- `[[params.bones]]`: `name`, optional `parent`, `position`, `rotation`.
- `[[params.body_parts]]`: a `shape` (for example `ellipsoid`) with `dimensions`, attached to a `bone`, using a named `material`.
- `[[params.limb_chains]]`: `name`, `parent_bone`, `attach_offset`, segment definitions, and `mirror = true` to generate the bilateral twin. Chains create their own bones.
- `[params.materials.<name>]`: colour and PBR values referenced by body parts.
- `symmetry = "bilateral"` mirrors across X.

The output is a skinned GLB with a skeleton the [animation](animation.md) system can drive.

## Determinism and Seeds

All randomness flows through a `SeededRng` forked by name per stage (`"pattern"`, `"pipeline"`, and so on), so adding a stage never disturbs the numbers an earlier one draws. A `fixed` seed reproduces the asset bit for bit; `derived` hashes a string so a spec can be reseeded by, say, an entity name; `random` is for variants. `flint gen --seed N` overrides whatever the spec says, and `--batch N --seed-start S` writes N sequential-seed variants with the seed inserted into each filename.

Generators are stateless and `Send + Sync`; parameters live in the spec and randomness in the RNG.

## `flint gen`

```bash
flint gen specs/oak_tree.procgen.toml -o tree.glb
flint gen specs/stone_wall.procgen.toml -o wall.png
flint gen specs/oak_tree.procgen.toml --dry-run          # cost estimate, no output
flint gen specs/oak_tree.procgen.toml --seed 7 --register
flint gen specs/beetle.procgen.toml --batch 5 --seed-start 100
flint gen specs/oak_tree.procgen.toml --validate --style-guide styles/lowpoly.toml --strict
```

| Flag | Description |
|------|-------------|
| `-o, --output` | File or directory. Derived from `meta.name` and the output kind when omitted |
| `--seed` | Override the spec's seed |
| `--dry-run` | Print the generator's cost estimate without generating |
| `--format` | Force `glb` or `png` instead of inferring from the extension |
| `--batch N`, `--seed-start S` | Sequential-seed variants |
| `--register` | Store the output in `.flint/assets` and write a `.asset.toml` sidecar recording spec hash, seed and content hash |
| `--force` | Regenerate even when a registered output with the same spec hash and seed exists |
| `--validate`, `--strict`, `--style-guide` | Run the output validator (geometry, materials, style constraints); `--strict` turns warnings into exit code 1 |

Image sets write one PNG per map with the map name suffixed.

## Previewing

`flint edit` opens the right tool by inspecting the spec:

- **Procgen previewer** (any spec whose pattern is not `pipeline`): a 3D orbit viewport for meshes or texture tabs for images, plus a parameter panel generated from the generator's schema. Edits regenerate live. `R` rerolls the seed, `Ctrl+S` saves the spec, `Ctrl+Shift+S` saves as, `Ctrl+E` exports the output, `Space` resets the view, `T` toggles tiled preview for textures, `O` toggles auto-orbit with `[` and `]` for speed, `Tab` hides the panels. `--watch` reloads when the file changes on disk.
- **Texture pipeline editor** (`pattern = "pipeline"`): a three-pane node editor built on egui-snarl, with global parameters and the selected node's parameters on the left, the graph in the middle, and the output maps with a channel browser on the right. `Ctrl+Z` and `Ctrl+Shift+Z` undo and redo, `Delete` removes the selected node, `Ctrl+S` saves, `Ctrl+E` exports.

```bash
flint edit specs/oak_tree.procgen.toml
flint edit specs/alien_organic.procgen.toml     # pipeline pattern, opens the node editor
```

## Specs in Scenes

The player indexes every `*.procgen.toml` in `specs/` beside the scene, `../specs/` (the game root when scenes live in `levels/`), and `models/` by `meta.name`, later directories overriding earlier ones. When an entity's `model.asset`, `material.texture` or `sprite.texture` names something that is neither a file on disk nor already in the mesh cache but *is* an indexed spec, the resolver generates it and uploads the result straight to the GPU:

```toml
[entities.old_oak.model]
asset = "garden_tree"            # specs/garden_tree.procgen.toml

[entities.floor.material]
texture = "crypt_floor"          # specs/crypt_floor.procgen.toml, an image set
```

Outputs are cached in memory keyed by spec hash and seed under a 256 MB budget, and queued generation is limited to a few milliseconds per frame so a scene full of specs streams in without a hitch. A `random` seed makes every instance unique; a `fixed` seed makes them identical, which is the difference between a cave full of beetles and one boss.

Headless `flint render` does not run the resolver. For snapshots of procgen-backed scenes, generate the assets first with `flint gen -o models/<name>.glb` so they resolve as files.

## Architecture

- **Algorithms**: noise (Perlin, simplex, Worley, FBM), an L-system engine, space colonization, and a mesh builder with primitives, extrusion, normals, tangents, UVs and simplification.
- **Generators**: `tree`, `texture` (patterns, node graph, map derivation) and `creature` (body parts, limb chains, skeleton builder), registered through `GeneratorRegistry`.
- **Spec, seed and RNG**: `ProcGenSpec`, `Seed`, `SeededRng`.
- **Output**: `MeshData`, `ImageData`, `SkinnedMeshData`, GLB export with LOD chains and skins.
- **Runtime**: `ProcGenResolver` (discovery, cache, frame-budgeted queue), used by the player.
- **Validation**: `validate_output` with style-guide constraints.

`flint-procgen-ai` sits on top for tool-time spec creation and refinement from prompts; it is not linked into the player.

## Further Reading

- [AI Asset Generation](ai-generation.md): the provider-driven half of the pipeline
- [Assets](assets.md): the content store and sidecars `--register` writes into
- [Animation](animation.md): driving the skeletons `creature_v1` produces
- [CLI Reference](../cli-reference/overview.md): `flint gen` and `flint edit`
