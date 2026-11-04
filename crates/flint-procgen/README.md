# flint-procgen

Procedural generation for the Flint engine. A small TOML file, a **spec**, describes an asset by rule instead of by hand: a tree, a set of PBR texture maps, a skinned creature. A **generator** turns the spec into the asset. The same spec and seed give the same bytes on every machine, so a spec is something you can check in, review and diff like code.

```bash
flint gen specs/examples/oak_tree.procgen.toml -o oak.glb          # a tree, three LODs
flint gen specs/examples/stone_wall.procgen.toml -o stone_wall.png # three PBR maps
flint edit specs/examples/pine_tree.procgen.toml                   # tune it live
```

The book's [Procedural Generation](https://docs.chaps.dev/concepts/procgen.html) page walks through each generator in prose; this README is the reference beside the code. To write a generator of your own, see [CONTRIBUTING.md](CONTRIBUTING.md).

## Quick start

Four commented specs live in [`specs/examples/`](../../specs/examples). Read one top to bottom, then run it:

| Spec | What it makes | Shows |
|---|---|---|
| [`oak_tree`](../../specs/examples/oak_tree.procgen.toml) | A broad oak, GLB with three LODs | `tree_v1`, the built-in L-system, seasonal leaf shades |
| [`pine_tree`](../../specs/examples/pine_tree.procgen.toml) | A Scots pine, GLB with three LODs | `tree_v1`, space colonization in a `cone` crown |
| [`stone_wall`](../../specs/examples/stone_wall.procgen.toml) | Fieldstone in mortar, three tiling PBR maps | `texture_v1`, the `voronoi_brick` pattern |
| [`wood_plank`](../../specs/examples/wood_plank.procgen.toml) | Oak floorboards, three PBR maps | `texture_v1`, a `pipeline` of ops |

```bash
# Generate. The output kind picks the format: meshes are GLB, images PNG.
flint gen specs/examples/pine_tree.procgen.toml -o pine.glb

# Check a spec without generating: the seed it would use and a cost estimate.
flint gen specs/examples/pine_tree.procgen.toml --dry-run

# A sibling: same spec, another seed.
flint gen specs/examples/oak_tree.procgen.toml --seed 7 -o oak_7.glb

# Four variants, seeds 1-4, written as planks/wood_plank_1_color.png and so on.
flint gen specs/examples/wood_plank.procgen.toml --batch 4 --seed-start 1 -o planks/

# Refuse keys the generator would ignore (typos, keys for another mode).
flint gen specs/examples/stone_wall.procgen.toml -o wall.png --strict

# Store the output with a provenance sidecar; a rerun reuses it.
flint gen specs/examples/oak_tree.procgen.toml -o oak.glb --register
```

`flint edit <spec>` opens the previewer: a parameter panel built from the generator's schema (hover a field for what it does), live regeneration, `R` for a new seed, `Ctrl+S` to save. A `pattern = "pipeline"` texture opens in the node editor instead.

## Concepts

**Spec.** A `*.procgen.toml` file naming a generator and giving it parameters. It is data, not code: everything about the asset is in the spec, and the generator holds no state.

**Generator.** A type implementing the [`Generator`](src/generator.rs) trait, registered under a versioned name (`tree_v1`). It publishes a JSON Schema for its `[params]`. That schema builds the previewer's panel, lets `flint gen` warn about keys it would ignore, and *is* the catalog below.

**Seed.** All randomness flows from one 64-bit seed through a [`SeededRng`](src/rng.rs). A generator forks it per stage by name (`rng.fork("leaves")`), so a stage added later never shifts the numbers an earlier stage draws.

**Determinism.** Same spec, same seed, same generator version: the same bytes. Change a generator's output and you change every asset made with it, which is why the names carry a version (`tree_v1`). Anything reproducible derives from the seed or from names, never from clocks, entity ids or hash-map order.

**Outputs.** A mesh (written as GLB, with an LOD chain when the spec has `[[lod]]` levels), an image or an image set (one PNG per map), or a skinned mesh (GLB with a skeleton).

```text
 spec.procgen.toml ──parse──▶ ProcGenSpec ──registry──▶ Generator::generate(spec, rng)
                                   │                            │
                     [seed] ──▶ SeededRng ──fork per stage──────┘
                                                                ▼
                                       GeneratorOutput: mesh · LODs · image set · skinned mesh
                                                                ▼
                                         flint gen: GLB / PNG files   ·   player: GPU, in memory
```

## Spec format

```toml
generator = "tree_v1"            # a registered generator name

[meta]
name = "oak_tree"                # unique: what scenes and the resolver look up
description = "A broad oak"      # optional
version = "1.0.0"
tags = ["tree", "oak"]           # optional

[seed]
mode = "fixed"                   # "fixed" | "random" | "derived"
value = 42                       # fixed: the seed itself
# derive_from = "old_oak"        # derived: a string hashed to a seed

[params]                         # the generator's own keys; see the catalog
trunk_height = 4.5

[[lod]]                          # optional, meshes only
level = 0
target_triangles = 9000
```

| Field | Type | Meaning |
|---|---|---|
| `generator` | string | Registered generator name |
| `meta.name` | string | Unique name: scenes refer to the asset by it |
| `meta.description` | string, optional | Free text |
| `meta.version` | string | The spec's own version |
| `meta.tags` | list of strings, optional | Search and provenance |
| `seed.mode` | `fixed` \| `random` \| `derived` | `fixed` reproduces exactly, `random` draws a new seed each run, `derived` hashes `derive_from` |
| `seed.value` | integer | The seed for `fixed` |
| `seed.derive_from` | string | The string `derived` hashes |
| `params` | table | Generator-defined; checked against its schema |
| `lod[].level` | integer | LOD index, 0 the most detailed |
| `lod[].target_triangles` | integer | Triangle cap for that level |

`flint gen --seed N` overrides `[seed]` for one run.

## Generator catalog

Every key, generated from the generators' own schemas by `cargo run -p flint-procgen --example param_catalog`. A test fails when this section and the schemas differ. "Only when" tables list keys that a mode ignores; `flint gen` warns about them, `--strict` refuses them, and the previewer hides them.

<!-- catalog:begin -->
### `creature_v1` (mesh, written as GLB)

| Key | Type | Default | What it does |
|---|---|---|---|
| `body_parts` | array of tables |  | Geometry attached to bones |
| `bones` | array of tables |  | Bone hierarchy definitions |
| `limb_chains` | array of tables |  | Auto-generated bone chains with tapered geometry |
| `materials` | table |  | Named materials, as [params.materials.<name>] tables; body parts and limb chains pick one by name |
| `symmetry` | `bilateral` \| `none` | `"none"` | Symmetry mode for mirroring |

### `texture_v1` (image set, written as PNGs)

| Key | Type | Default | What it does |
|---|---|---|---|
| `height` | integer ≥ 1 | `512` | Image height in pixels |
| `output_maps` | list of `albedo`, `normal`, `roughness` | `["albedo", "normal", "roughness"]` | Which PBR maps to write; each becomes <stem>_<map>.png |
| `pattern` | `voronoi_brick` \| `perlin_organic` \| `tiling_grid` \| `pipeline` | `"voronoi_brick"` | The base shape: Voronoi stones, layered organic noise, a grid of tiles, or a pipeline of ops |
| `seamless` | bool | `true` | Tile without a seam. In a pipeline, noise and repeating ops round their frequencies to whole cycles per tile (simplex cross-fades); false keeps the unwrapped fields |
| `width` | integer ≥ 1 | `512` | Image width in pixels |

Only when `pattern` is `perlin_organic`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `frequency` | number > 0 | `4.0` | Noise lattice cells across the texture for the first octave; rounded to a whole number when seamless |
| `height_bands` | integer ≥ 1 | `8` | Bands the noise height is cut into; each band is a cell with its own shade |
| `lacunarity` | number > 0 | `2.0` | Frequency multiplier from one octave to the next |
| `noise_type` | `perlin` \| `simplex` | `"perlin"` | Noise basis: perlin, or simplex (fewer grid artefacts) |
| `octaves` | integer ≥ 1 | `6` | Noise octaves layered into the bands; more adds finer detail |
| `persistence` | number >0 to 1 | `0.5` | Amplitude multiplier from one octave to the next; lower is smoother |

Only when `pattern` is `pipeline`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `ops` | array of tables |  | The pipeline's operations, run in order (or as a graph when an op has an `id`); each type's keys are in `op_schema` |

Only when `pattern` is `tiling_grid`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `columns` | integer ≥ 1 | `4` | Tiles across |
| `gap_width` | number 0–1 | `0.05` | Grout between tiles, as a fraction of a tile |
| `jitter` | number 0–0.5 | `0.0` | How far each tile strays from the grid, 0 none to 0.5 |
| `noise_strength` | number 0–1 | `0.03` | Fine surface noise on each tile's height |
| `rows` | integer ≥ 1 | `4` | Tiles down |

Only when `pattern` is `voronoi_brick`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `cell_count` | integer ≥ 1 | `12` | Voronoi cells (stones) across the whole texture |
| `cell_size` | number > 0 |  | Typical stone size in pixels; overrides cell_count |
| `irregularity` | number 0–1 |  | 1 - regularity; give one or the other |
| `mortar_width` | number 0–1 | `0.02` | Full width of the groove between stones, as a fraction of the texture width |
| `regularity` | number 0–1 | `0.0` | 0 scatters the stones at random, 1 relaxes them into an even layout |

Only when `pattern` is `voronoi_brick` or `perlin_organic` or `tiling_grid`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `base_color` | hex colour | `"#735A47"` | Albedo of the cells (stones, tiles, bands), sRGB hex; each cell gets its own shade of it |
| `color_variation` | number ≥ 0 | `0.06` | How far each cell's lightness strays from base_color (saturation moves by half that; hue stays) |
| `detail_octaves` | integer ≥ 1 | `4` | Octaves in the detail field, each twice as fine; any finer than 4 px are dropped |
| `detail_scale` | number ≥ 0 | `8.0` | Detail lattice cells across the texture width (first octave) |
| `detail_strength` | number ≥ 0 | `0.1` | RMS brightness change in the albedo (0.1 = 10%); also scales the detail's relief and roughness |
| `mortar_color` | hex colour | `"#40382E"` | Albedo in the grooves between cells (strata lines on perlin_organic), sRGB hex |
| `mortar_threshold` | number ≥ 0 |  | distance_to_edge below which mortar colour and roughness blend in; default 0.5 (half the groove) for voronoi_brick and tiling_grid, 0.05 (strata lines) for perlin_organic |
| `normal_strength` | number ≥ 0 | `1.0` | Depth of the normal map's relief; 0 is flat |
| `roughness_base` | number 0–1 | `0.75` | Roughness of the cells' faces |
| `roughness_mortar` | number 0–1 | `0.5` | Roughness in the grooves |
| `roughness_variation` | number ≥ 0 | `0.05` | Per-cell random spread of roughness |

Only when `pattern` is `voronoi_brick` or `tiling_grid`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `height_variation` | number 0–1 | `0.3` | Per-cell random height, 0 flat to 1 the most; shows in the normal map |

### `tree_v1` (mesh, written as GLB)

| Key | Type | Default | What it does |
|---|---|---|---|
| `bark_color_base` | hex colour | `"#573214"` | Bark albedo, sRGB hex |
| `bark_normal_resolution` | integer | `256` | Side of the square bark normal map in pixels; a power of two, at least 16 |
| `bark_normal_strength` | number | `1.0` | Depth of the bark normal map's furrows and cracks; 0 is smooth |
| `bark_roughness` | number 0–1 | `0.9` | Bark material roughness |
| `branch_levels` | integer ≥ 1 | `3` | **Deprecated.** Has no effect: the L-system's depth is lsystem_iterations, and space colonization grows until its points run out |
| `branch_max_segments` | integer ≥ 1 | `5000` | Cap on branch segments the growth may emit, a guard against runaway grammars |
| `branch_method` | `lsystem` \| `space_colonization` | `"lsystem"` | How limbs grow: an L-system (a built-in rule, or a grammar of your own) or space colonization toward points filling the crown |
| `branch_radial_segments` | integer ≥ 3 | `6` | Sides around the thickest limbs; thinner ones get fewer, down to three |
| `branch_start` | number 0–1 | `0.6` | Where on the trunk the lowest limb leaves it, 0 the ground to 1 the tip. A grammar ignores it and grows from the tip |
| `crown_radius` | number > 0 | `3.0` | Crown size, metres: the built-in L-system's reach, or the colonized volume's radius. A grammar uses lsystem_size instead |
| `leaf_along_branch_density` | number 0–1 | `0.0` | Chance of a cluster on each branch segment as well as the tips; 0 leaves the tips only |
| `leaf_along_branch_min_depth` | integer ≥ 0 | `2` | Shallowest branching level that gets along-branch leaves (1 = limbs off the trunk) |
| `leaf_color_base` | hex colour | `"#2D8C0D"` | Leaf albedo before the season tints it, sRGB hex |
| `leaf_color_variation` | number ≥ 0 | `0.08` | Lightness spread between the shades (HSL, ±) |
| `leaf_density` | number 0–1 | `0.8` | Fraction of branch tips that get a cluster of leaves |
| `leaf_season` | number 0–1 | `0.0` | Time of year: 0 the base colour, 0.4 yellow, 0.7 orange, 1 red |
| `leaf_season_variation` | number 0–1 | `0.06` | How far each shade's season strays from leaf_season, so a tree turns unevenly |
| `leaf_shades` | integer 1–8 | `4` | Distinct leaf shades, each its own material |
| `leaf_size` | number ≥ 0.01 | `0.15` | Leaf card size, metres |
| `leaf_size_variation` | number 0–1 | `0.3` | Random size spread, as a fraction (0.3 = ±30%) |
| `leaf_spread_radius` | number ≥ 0 | `0.3` | Radius a cluster's leaves scatter within, metres |
| `leaf_style` | `billboard` \| `mesh` \| `none` | `"billboard"` | Leaf cards: quads facing along their branch, diamonds tilted off it, or no leaves |
| `leaves_per_tip` | integer ≥ 1 | `5` | Leaves in each cluster |
| `radial_segments` | integer ≥ 3 | `8` | Sides around the trunk |
| `trunk_curve_noise` | number ≥ 0 | `0.1` | Largest sideways sway of the trunk, metres (wavelength about 0.6 of its height); 0 grows it straight |
| `trunk_height` | number ≥ 0.01 | `5.0` | Trunk length in metres, from the ground to where it ends |
| `trunk_radius_base` | number ≥ 0.01 | `0.3` | Trunk radius at the ground, metres |
| `trunk_radius_top` | number ≥ 0 | `0.1` | Trunk radius at its tip, metres; the limbs carrying on from the tip start this thick |
| `trunk_segments` | integer ≥ 2 | `10` | Rings along the trunk; more follow its sway more smoothly |

Only when `branch_method` is `lsystem`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `branch_angle_max` | number | `40.0` | Largest angle a limb leaves its parent at, degrees; a grammar without lsystem_angle turns by the middle of min and max |
| `branch_angle_min` | number | `20.0` | Smallest angle a limb leaves its parent at, degrees |
| `branch_length_falloff` | number >0 to 1 | `0.7` | Each branching level's length as a fraction of its parent's |
| `branch_radius_falloff` | number >0 to 1 | `0.6` | Each branching level's radius as a fraction of its parent's |
| `lsystem_angle` | number |  | A grammar's turn angle, degrees; defaults to the preset's, else the middle of branch_angle_min/max |
| `lsystem_angle_variation` | number ≥ 0 | `10.0` | Random spread added to every turn of the built-in rule, degrees. A grammar ignores it |
| `lsystem_axiom` | string |  | Your own grammar's starting string; with lsystem_rules it replaces the built-in rule |
| `lsystem_iterations` | integer ≥ 1 | `3` | Rewriting passes: the crown's branching depth. Each one multiplies the segment count |
| `lsystem_preset` | `fractal_plant` \| `weed` \| `stochastic_plant` \| `bush` |  | A built-in grammar from The Algorithmic Beauty of Plants, in place of the built-in rule |
| `lsystem_rules` | list of strings |  | Your own grammar's productions, e.g. "X -> F[+X][-X]FX" or weighted "F (0.7) -> FF" |
| `lsystem_size` | number > 0 | `4.0` | How far a grammar's crown reaches from the trunk tip, metres, when lsystem_step is unset |
| `lsystem_step` | number > 0 |  | A grammar's fixed step length in metres; when unset its crown is scaled to lsystem_size |
| `lsystem_tropism` | number | `0.0` | A grammar's bend toward the ground per step; negative bends up toward the light |

Only when `branch_method` is `space_colonization`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `branch_pipe_exponent` | number ≥ 1 | `2.0` | Pipe model exponent n in r_parent^n = sum(r_child^n): 2 is da Vinci's rule, about 3 keeps twigs sturdier |
| `crown_height_ratio` | number > 0 | `1.5` | Crown height as a multiple of crown_radius, for the ellipsoid, box and cone |
| `crown_shape` | `sphere` \| `ellipsoid` \| `hemisphere` \| `box` \| `cone` | `"sphere"` | The volume the attraction points fill; it rests on the trunk at branch_start |
| `influence_distance` | number > 0 | `4.0` | How far a point pulls the nearest branch tip, metres |
| `kill_distance` | number > 0 | `0.5` | How close a branch must come to consume a point, metres; smaller fills the crown more finely |
| `num_attraction_points` | integer ≥ 0 | `500` | Points scattered through the crown; branches grow until they have consumed them. More gives a denser crown |
| `step_size` | number > 0 | `0.3` | Length of each growth step, metres |
<!-- catalog:end -->

The texture pipeline's ops (`brick_grid`, `noise_layer`, `domain_warp`, `wave_texture` and about 30 more) each have a schema of their own: `flint_procgen::op_param_schema("noise_layer")`, or the node editor's panel. The book's procgen page lists them.

## Runtime integration

Scenes never have to call `flint gen`. The player builds a [`ProcGenResolver`](src/resolver.rs) that indexes every `*.procgen.toml` by `meta.name` in `specs/` beside the scene, `../specs/` and `models/`, later directories overriding earlier ones. When an entity names something that is neither a file on disk nor already loaded, but is an indexed spec, the resolver generates it and the player uploads the result straight to the GPU:

```toml
[entities.old_oak.model]
asset = "oak_tree"               # specs/examples/oak_tree.procgen.toml, if indexed

[entities.floor.material]
texture = "wood_plank"           # an image set: color, normal, roughness
```

Generated assets are cached in memory by spec hash and seed (256 MB budget, least recently used out first), and queued generation spends a few milliseconds a frame, so a scene full of specs streams in without a hitch. A `fixed` seed makes every instance of a spec identical; a `random` one makes each unique.

Headless `flint render` does not run the resolver: generate the assets into `models/` first with `flint gen` so they resolve as files.

From Rust, the same path in four lines:

```rust
use flint_procgen::{register_built_in_generators, GeneratorRegistry, ProcGenSpec};

let spec = ProcGenSpec::parse(r##"
    generator = "texture_v1"
    [meta]
    name = "tiles"
    version = "1.0.0"
    [seed]
    mode = "fixed"
    value = 3
    [params]
    pattern = "tiling_grid"
    width = 64
    height = 64
    columns = 4
    rows = 4
    base_color = "#B0A890"
"##)?;

let mut registry = GeneratorRegistry::new();
register_built_in_generators(&mut registry);

// Keys the generator would ignore, before spending time on it.
let schema = registry.get(&spec.generator).unwrap().param_schema();
assert!(flint_procgen::lint_params(&schema, &spec.params).is_empty());

let maps = registry.generate_from_spec(&spec)?.into_image_set().unwrap();
assert_eq!(maps.len(), 3); // color, normal, roughness
assert_eq!(registry.generate_from_spec(&spec)?.into_image_set().unwrap(), maps); // deterministic
# Ok::<(), flint_procgen::ProcGenError>(())
```

## Layout

| Path | What lives there |
|---|---|
| `src/generator.rs`, `src/registry.rs` | The `Generator` trait and the name → generator registry |
| `src/spec.rs`, `src/seed.rs`, `src/rng.rs` | Spec parsing, seeds, the forkable RNG |
| `src/generators/{tree,texture,creature}` | The built-in generators |
| `src/algorithms/` | Noise (Perlin, simplex, Worley, FBM), L-systems, space colonization, mesh building and simplification |
| `src/param_ui.rs` | Schema → editor fields, `lint_params`, this catalog |
| `src/export_glb.rs`, `src/skinning.rs` | GLB export with LOD chains and skins |
| `src/resolver.rs`, `src/cache.rs`, `src/queue.rs` | The player's spec resolver, memory cache and frame-budgeted queue |
| `src/validate.rs` | Output validation against style-guide constraints |
| `examples/` | Galleries (`lsystem_gallery`, `colonization_gallery`, ...) and `param_catalog` |

`flint-procgen-ai` builds on this crate to write and refine specs from a prompt; the player never links it.
