# flint-procgen

Procedural generation for the Flint engine. A small TOML file, a **spec**, describes an asset by rule instead of by hand: a tree, a set of PBR texture maps, a skinned creature. A **generator** turns the spec into the asset. The same spec and seed give the same bytes on every machine, so a spec is something you can check in, review and diff like code.

```bash
flint gen specs/examples/oak_tree.procgen.toml -o oak.glb          # a tree, three LODs
flint gen specs/examples/stone_wall.procgen.toml -o stone_wall.png # three PBR maps
flint edit specs/examples/pine_tree.procgen.toml                   # tune it live
```

The book's [Procedural Generation](https://docs.chaps.dev/concepts/procgen.html) page walks through each generator in prose; this README is the reference beside the code. To write a generator of your own, see [CONTRIBUTING.md](CONTRIBUTING.md).

## Quick start

Five commented specs live in [`specs/examples/`](../../specs/examples). Read one top to bottom, then run it:

| Spec | What it makes | Shows |
|---|---|---|
| [`oak_tree`](../../specs/examples/oak_tree.procgen.toml) | A broad oak, GLB with three LODs | `tree_v1`, the built-in L-system, seasonal leaf shades |
| [`oak_living`](../../specs/examples/oak_living.procgen.toml) | The same oak as a living prop | A `[living]` table |
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

A spec can also have a `[living]` table, which makes it a prop that keeps drifting between versions of itself. See `LivingGenerator` in `src/generator.rs` and [`specs/examples/oak_living.procgen.toml`](../../specs/examples/oak_living.procgen.toml). `flint gen <spec> --strip -o strip.png` renders one over time as a labelled contact sheet; the clock and the blend are `LivingClock` and `PoseStream` in `src/living.rs`.

## Generator catalog

Every key, generated from the generators' own schemas by `cargo run -p flint-procgen --example param_catalog`. A test fails when this section and the schemas differ. "Only when" tables list keys that a mode ignores; `flint gen` warns about them, `--strict` refuses them, and the previewer hides them. `flint gen --describe <generator> --markdown` prints one generator's part of this catalog (and, for `texture_v1`, every pipeline op's).

<!-- catalog:begin -->
### `creature_v1` (mesh, written as GLB)

| Key | Type | Default | What it does |
|---|---|---|---|
| `body_parts` | array of tables |  | Geometry attached to bones |
| `bones` | array of tables |  | Bone hierarchy definitions |
| `limb_chains` | array of tables |  | Auto-generated bone chains with tapered geometry |
| `materials` | table |  | Named materials, as [params.materials.<name>] tables; body parts and limb chains pick one by name |
| `symmetry` | `bilateral` \| `none` | `"none"` | Symmetry mode for mirroring |

### `form_v1` (mesh, written as GLB)

| Key | Type | Default | What it does |
|---|---|---|---|
| `aspect` | number >0 to 1 | `1.0` | The section's thin axis over its broad one: 1 round, 0.1 a ribbon |
| `bend_deg` | number 0–90 | `16.0` | Pose: how far a pose leans the tip, degrees, at drift amplitude 1. The slow lean of the whole form |
| `body_color` | hex colour | `"#3E382F"` | The body's colour, sRGB hex |
| `body_opacity` | number 0–1 | `1.0` | How much of the body shows: 1 solid; below 1 it's drawn translucent after everything opaque, and its echoes with it; 0 not at all, so the form is only its cards |
| `body_roughness` | number 0–1 | `0.800000011920929` | The body's roughness, 0 glossy to 1 matte |
| `card_aspect` | number >0 to 10 | `1.5` | A card's length over its width: above 1 a feather or flake along the surface |
| `card_color` | hex colour | `"#9C8E7A"` | The cards' middle colour, sRGB hex; shades spread warmer and cooler around it |
| `card_from` | number 0–1 | `0.03999999910593033` | Height fraction where cards begin, below `card_to` |
| `card_layout` | `scatter` \| `rows` | `"scatter"` | Where cards lie: `scatter` over the whole surface by area; `rows` in lines across the broad faces, a card a word, so a flat form reads as a page of writing. In rows `card_size` is a stroke's thickness and `card_aspect` a typical word's length over it, the cards lie flat, and `card_tilt` and `card_rise` have nothing to do |
| `card_lift` | number 0–1 | `0.012000000104308128` | How far a card sits off the surface, metres |
| `card_roughness` | number 0–1 | `0.699999988079071` | The cards' roughness, 0 glossy to 1 matte |
| `card_shades` | integer 1–8 | `4` | Shades the cards are spread over, each a part of its own the jitter tints |
| `card_size` | number >0 to 10 | `0.05999999865889549` | Typical card width, metres; each varies ±30% |
| `card_to` | number 0–1 | `0.9700000286102295` | Height fraction where cards end |
| `card_turn_deg` | number 0–180 | `35.0` | Largest turn of a card about its normal away from pointing up the surface, degrees |
| `cards` | integer 0–20000 | `520` | Cards on the surface: what boils. 0 is a smooth form. With `card_layout = "rows"` it's the most there can be, and the rows decide how many |
| `coil` | number 0–10 | `0.0` | Radius of a helical wander of the spine at the tip, metres, from nothing at the base |
| `coil_turns` | number 0–12 | `1.0` | Turns the coil makes over the spine |
| `curl_bias` | number 0.2–5 | `1.0` | Where the curl happens: 1 evenly, above 1 more toward the tip, below 1 more toward the base |
| `curl_deg` | number -270–270 | `0.0` | Degrees the spine's direction turns through from base to tip, toward `facing_deg`: a hook, a wave, a fold |
| `facing_deg` | number -360–360 | `0.0` | Direction of the lean and curl, degrees about up from +X (0 bends sideways to a camera on +Z); each lineage turns it by up to ±15° |
| `height` | number >0 to 100 | `1.2999999523162842` | Length of the spine from base to tip, metres (its height when it doesn't curl) |
| `lean` | number -10–10 | `0.07000000029802322` | Sideways offset of the tip toward `facing_deg`, metres, growing with the square of the height |
| `lift` | number 0–100 | `0.0` | How far the base stands above the prop's origin, metres: above 0 the form floats |
| `lobes` | number 0–0.5 | `0.05999999865889549` | Depth of the slow lobes the lineage puts in the surface, as a fraction of the radius |
| `profile` | list of numbers | `[0.6000000238418579, 0.949999988079071, 1.0, 0.8500000238418579, 0.550000011920929, 0.15000000596046448]` | The section's radius as fractions of `radius`, evenly spaced from base to tip and joined smoothly: a flame narrows to 0 at the tip, a stone stays wide |
| `radius` | number >0 to 50 | `0.27000001072883606` | The section's radius where `profile` is 1, metres |
| `rings` | integer 4–1024 | `96` | Rings of vertices along the body |
| `ripple` | number 0–2 | `0.5` | Pose: a wave up the spine, as a share of `bend_deg` |
| `ripple_waves` | number 0–6 | `1.399999976158142` | Pose: waves of the ripple along the length; each pose varies it ±25% |
| `round_base` | number 0–0.5 | `0.05999999865889549` | Fraction of the length over which the base rounds off to the spine: 0 is a flat base |
| `round_tip` | number 0–0.5 | `0.11999999731779099` | Fraction of the length over which the tip rounds off to the spine: 0 is a flat top |
| `section_deg` | number -360–360 | `0.0` | Turn of the section's broad axis away from the curl's plane, degrees: 90 folds a ribbon across its width |
| `segments` | integer 2–64 | `10` | Bones along the spine: more is a smoother bend and more palette per prop |
| `sides` | integer 6–1024 | `64` | Vertices around each ring |
| `slide` | number 0–1 | `0.0` | Pose: how far a pose moves the form along its section's broad axis at most, metres: along the lines of a page in rows. Unlike a bend it doesn't pivot at the base, so an echo can stand beside the form rather than fan from its root |
| `slide_reflow` | number 0–1 | `0.5` | Pose: how much of the slide changes along the length rather than moving the whole form: 0 slides it as one, 1 lets lines at one height go one way and lines at another the other |
| `stretch` | number 0–0.5 | `0.05000000074505806` | Pose: how much a pose lengthens or shortens the spine, as a fraction (`[living] variance.keep_size` evens the box out again) |
| `swell` | number 0–0.9 | `0.14000000059604645` | Pose: how much the section breathes wider and narrower along the length, as a fraction of its radius |
| `turn_deg` | number 0–180 | `0.0` | Pose: how far a pose turns the section about the spine at the tip, degrees: what a flat form shows |
| `twist_turns` | number -8–8 | `0.0` | Turns the section makes about the spine from base to tip, at rest |
| `waves` | integer 1–64 | `3` | Depth bands the segments fall into, base to tip, at most `segments`: per-level stagger ripples up the form in this many waves, and `[living] variance.steady` holds the low bands still |

Only when `card_layout` is `rows`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `paragraph_lines` | number 0–1000 | `6.0` | Rows: lines in a typical paragraph, whose last line stops short and whose next begins indented; 0 is one unbroken block |
| `row_faces` | `front` \| `back` \| `both` | `"both"` | Rows: which broad faces carry lines. The front is the face turned away from `facing_deg`, so a page curling away from you shows you its front |
| `row_margin` | number 0–0.9 | `0.10000000149011612` | Rows: the share of the section's half-width kept clear at each side, so lines stop short of the edge and never bristle at the outline |
| `row_pace` | number -5–5 | `0.0` | Rows: metres a second the words drift along their lines, the way a line reads (below 0 the other way); a word slides out under one margin and back in under the other, as text reflows. 0 holds them where they were written. `[living] row_rate` multiplies it at run time |
| `row_pace_spread` | number 0–1 | `0.30000001192092896` | Rows: how far each line's pace differs from `row_pace`, as a share either way, drawn once per line by the lineage: 0 moves every line together |
| `row_spacing` | number >0 to 10 | `0.03999999910593033` | Rows: metres from one line to the next, up the surface |
| `word_gap` | number 0–10 | `0.012000000104308128` | Rows: metres between words; each gap varies ±30% |

Only when `card_layout` is `scatter`:

| Key | Type | Default | What it does |
|---|---|---|---|
| `card_rise` | number -0.9–10 | `0.0` | How cards crowd along the length: 0 by area alone, above 0 toward the tip, below 0 toward the base |
| `card_tilt` | number 0–1 | `0.0` | How far cards stand up off the surface toward the tip: 0 flat like scales, 1 upright like a fringe, which boils the outline's edge |

### `sweep_v1` (mesh, written as GLB)

| Key | Type | Default | What it does |
|---|---|---|---|
| `closed` | bool | `false` | A loop: the last control point joins the first, and there are no end caps. A spline file's `[spline] closed` defaults to true; this to false, since a swept model is more often a run with two ends |
| `color` | hex colour | `"#808080"` | Base colour, sRGB hex |
| `control_points` | array of tables |  | The path, as `[[params.control_points]]` tables written as a `.spline.toml`'s are: `position = [x, y, z]` in metres, `twist` (banking, degrees, default 0) and `gap_after` (no geometry from here to the next point, default false) |
| `height` | number >0 to 100 | `0.2` | Without a `profile`, the height of a box cross-section, metres |
| `metallic` | number 0–1 | `0.0` | PBR metallic |
| `offset` | list of numbers | `[0.0, 0.0]` | [right, up] metres from the path to the profile's centre, in the path's own frame |
| `profile` | list of number lists |  | The cross-section as [right, up] points in metres from the path, in place of `width` and `height`: closed, a solid whose faces look outward whichever way round it goes; open, a surface facing the left of the way it is listed |
| `profile_closed` | bool | `true` | False makes `profile` an open surface, with no closing edge and no caps |
| `roughness` | number 0–1 | `0.5` | PBR roughness |
| `smooth` | bool | `false` | Shade the profile round instead of faceted, with texture u once around it: a pipe or a rail |
| `spacing` | number 0.01–100 | `0.5` | Metres between the path's samples: smaller is a smoother bend and more triangles |
| `texture_length` | number >0 to 1000 |  | Metres of sweep per texture repeat (v); on a loop nudged so the repeats meet at the seam. Unset: the profile's width |
| `width` | number >0 to 100 | `0.2` | Without a `profile`, the width of a box cross-section, metres |

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

## Texture op catalog

A `pattern = "pipeline"` texture is a list of `[[params.ops]]` tables, each naming its op by `type`. Every op is said once, in an `OpDescriptor` beside its struct in its family's module under [`src/generators/texture/ops/`](src/generators/texture/ops/): its channels, and each key's kind, default, range and help line. Its schema (`flint_procgen::op_param_schema("noise_layer")`), the node editor's panel and tooltips, `flint gen`'s typo warnings, the parser's defaults and its errors (a wrong type, an unknown choice or a number out of range names `ops[i].key`), and this catalog all come from it. Adding an op is its struct, `impl TextureOp` and descriptor there, plus one line in `OPS` (`src/generators/texture/pipeline.rs`). Generated by `cargo run -p flint-procgen --example param_catalog -- ops`, and a test fails when the two differ. A colour default is shown as its nearest 8-bit hex.

<!-- op-catalog:begin -->
A channel in angle brackets is the one the parameter of that name names (`<output>` is the channel `output` says).

### `brick_grid` (Brick Grid)

Lay rectangular cells in staggered rows (running bond): bricks, planks, tiles.

Writes `cell_id`, `edge_dist`, `mask`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `columns` | integer ≥ 1 | `8` | Bricks across each row |
| `gap_width` | number 0–1 | `0.04` | The mortar gap as a fraction of a brick's size; sets how fast edge_dist rises from the edge |
| `row_height_variation` | number 0–1 | `0.0` | How much row heights vary: 0 all the same, 1 the most |
| `rows` | integer ≥ 1 | `16` | Rows of bricks down the texture |
| `stagger` | number 0–1 | `0.5` | How far each row shifts from the one above, as a fraction of a brick: 0.5 is running bond, 0 stacks them |
| `warp_strength` | number ≥ 0 | `0.0` | How far warp_x and warp_y move the grid, in texture widths; 0 keeps the courses straight |
| `warp_x` | string |  | Channel (0.5 is no shift) that moves the grid sideways before each pixel finds its brick; needs warp_y and warp_strength |
| `warp_y` | string |  | Channel (0.5 is no shift) that moves the grid up and down before each pixel finds its brick; needs warp_x and warp_strength |
| `width_variation` | number 0–1 | `0.0` | How much brick widths vary along a row: 0 all the same, 1 the most |

### `voronoi_grid` (Voronoi Grid)

Lay irregular Voronoi cells, for fieldstone and cobbles; every cell op reads them.

Writes `cell_id`, `edge_dist`, `mask`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `cell_count` | integer ≥ 1 | `20` | Cells (stones) across the whole texture |
| `mortar_width` | number 0–1 | `0.03` | Width of the mortar between cells, as a fraction of the texture |
| `regularity` | number 0–1 | `0.0` | 0 scatters the cells at random; toward 1 relaxes them (Lloyd) into even sizes |
| `warp_strength` | number ≥ 0 | `0.0` | How far warp_x and warp_y move the cells, in texture widths; 0 leaves them unbent |
| `warp_x` | string |  | Channel (0.5 is no shift) that moves the cells sideways before each pixel finds its cell; needs warp_y and warp_strength |
| `warp_y` | string |  | Channel (0.5 is no shift) that moves the cells up and down before each pixel finds its cell; needs warp_x and warp_strength |

### `domain_warp` (Domain Warp)

Displace a channel by two others (bilinear lookup): swirl, bend or smear it.

Reads `<input>`, `<warp_x>`, `<warp_y>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `input` | string | `"input"` | Channel to warp |
| `output` | string | `"output"` | Channel the warped copy goes to; may be input itself |
| `strength` | number ≥ 0 | `0.1` | How far the displacement reaches, in texture widths |
| `warp_x` | string | `"warp_x"` | Channel giving the horizontal displacement |
| `warp_y` | string | `"warp_y"` | Channel giving the vertical displacement |

### `cell_height` (Cell Height)

Give each cell a random base height, so stones stand proud of one another.

Reads `cell_id`; writes `height`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `variation` | number 0–1 | `0.3` | How far each cell's height strays at random: 0 all level, 1 the most |

### `cell_bulge` (Cell Bulge)

Dome each cell, highest at its centre, whatever its size or shape.

Reads `cell_id`, `edge_dist`; modifies `height`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `falloff` | `cosine` \| `parabolic` \| `linear` | `"cosine"` | The dome's profile from edge to centre: cosine an S-curve, flat at both ends; parabolic a flat-topped dome; linear a cone |
| `strength` | number ≥ 0 | `0.3` | Height added at each cell's centre |

### `noise_layer` (Noise Layer)

Write FBM noise (0–1) into a channel, for a blend to mix in downstream.

Writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `cell_offset` | bool | `false` | Offset the noise per cell (reads the cell id), so neighbouring stones don't share a pattern |
| `frequency` | number ≥ 0.1 | `10.0` | Noise cells across the texture in the first octave; rounded to whole cycles when seamless |
| `noise_type` | `perlin` \| `simplex` \| `value` \| `voronoi` | `"perlin"` | The base noise: perlin, simplex, value, or voronoi (Worley cells) |
| `octaves` | integer 1–16 | `4` | FBM octaves, each twice as fine |
| `output` | string | `"noise"` | Channel the noise is written to |
| `scale_x` | number ≥ 0.01 | `1.0` | Horizontal squeeze of the noise: above 1 packs it tighter across, for vertical streaks |
| `scale_y` | number ≥ 0.01 | `1.0` | Vertical squeeze of the noise: above 1 packs it tighter down, for horizontal streaks |

### `blend` (Blend)

Blend one channel into another in place, optionally through a mask.

Reads `<source>`; modifies `<target>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `mask` | string |  | Channel scaling the blend per pixel (a grid's mask keeps it off the mortar); unset, it applies everywhere |
| `mode` | `add` \| `add_raw` \| `multiply` \| `mix` \| `screen` \| `overlay` \| `soft_light` \| `linear_light` \| `difference` \| `darken` \| `lighten` \| `color_dodge` \| `color_burn` \| `subtract` | `"add"` | How source meets target: add centres source on 0.5 first and add_raw doesn't; multiply and mix move toward source by strength; the rest are the usual image blend modes |
| `source` | string | `"noise"` | Channel blended in |
| `strength` | number ≥ 0 | `0.1` | How much of the source goes in |
| `target` | string | `"height"` | Channel changed in place |

### `mortar_groove` (Mortar Groove)

Sink the height along cell edges, cutting the grooves between stones.

Reads `edge_dist`; modifies `height`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `depth` | number 0–1 | `0.15` | How far the height drops at the bottom of the groove |
| `width` | number 0–1 | `0.04` | The edge_dist below which the groove cuts in |

### `cell_color` (Cell Color)

Colour each cell a shade of one base colour, varying lightness and keeping hue.

Reads `cell_id`; writes `r`, `g`, `b`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `base_color` | hex colour | `"#735947"` | Albedo of the cells, hex; each cell gets its own shade of it |
| `variation` | number 0–1 | `0.12` | How far each cell's lightness strays (saturation by half that; hue stays) |

### `mortar_color` (Mortar Color)

Blend a mortar colour into the albedo near cell edges.

Reads `edge_dist`; modifies `r`, `g`, `b`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `color` | hex colour | `"#40382E"` | Albedo of the mortar, hex |
| `threshold` | number 0–1 | `0.06` | The edge_dist below which the mortar colour blends in |

### `derive_normal` (Derive Normal)

Derive the tangent-space normal map from the height channel.

Reads `height`; writes `normal_x`, `normal_y`, `normal_z`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `strength` | number ≥ 0 | `1.0` | Depth of the relief; 0 is flat |

### `cell_roughness` (Cell Roughness)

Give each cell its own roughness, and the mortar near its edges another.

Reads `cell_id`, `edge_dist`; writes `roughness`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `base` | number 0–1 | `0.75` | Roughness of the cells' faces |
| `mortar` | number 0–1 | `0.55` | Roughness of the mortar |
| `mortar_threshold` | number 0–1 | `0.06` | The edge_dist below which the mortar roughness blends in |
| `variation` | number 0–1 | `0.05` | Per-cell random spread of roughness |

### `math` (Math)

Per-pixel arithmetic on one channel, with a second channel or a constant.

Reads `<input_a>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `clamp` | bool | `false` | Clamp the result to 0–1 |
| `input_a` | string | `"input"` | Channel a |
| `input_b` | string |  | Channel b; unset, b is value_b |
| `operation` | `add` \| `subtract` \| `multiply` \| `divide` \| `power` \| `sqrt` \| `abs` \| `min` \| `max` \| `fract` \| `modulo` \| `snap` \| `sin` \| `cos` \| `less_than` \| `greater_than` \| `sign` \| `floor` \| `ceil` \| `round` \| `pingpong` \| `wrap` \| `negate` | `"add"` | What to do with a and b (the one-argument ones ignore b); dividing by 0 gives 0 |
| `output` | string | `"output"` | Channel the result goes to |
| `value_b` | number | `0.0` | The constant b, when input_b is unset |

### `map_range` (Map Range)

Remap a channel from one range to another, straight or eased.

Reads `<input>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `clamp` | bool | `true` | Keep the result between to_min and to_max |
| `from_max` | number | `1.0` | Input value that maps to to_max |
| `from_min` | number | `0.0` | Input value that maps to to_min |
| `input` | string | `"input"` | Channel to remap |
| `interpolation` | `linear` \| `smoothstep` \| `smootherstep` | `"linear"` | The curve between: linear, or eased at both ends (smoothstep, smootherstep; both clamp the input to the range) |
| `output` | string | `"output"` | Channel the result goes to |
| `to_max` | number | `1.0` | Output at from_max |
| `to_min` | number | `0.0` | Output at from_min |

### `color_ramp` (Color Ramp)

Colour the albedo by a channel through a gradient of stops.

Reads `<input>`; writes `r`, `g`, `b`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `input` | string | `"input"` | Channel (0–1) that picks the colour |
| `interpolation` | `linear` \| `constant` \| `ease` | `"linear"` | Between stops: linear blends, constant holds each stop's colour up to the next, ease blends with an S-curve |
| `stops` | array of tables |  | The ramp's colours, as [[params.ops.stops]] tables, at least one; sorted by position |

Each `[[params.ops.stops]]` entry:

| Key | Type | Default | What it does |
|---|---|---|---|
| `color` | hex colour | `"#000000"` | The colour there, hex (alpha is ignored) |
| `position` | number 0–1 | `0.0` | Where on the input this colour sits |

### `checker_texture` (Checker Texture)

Write a checkerboard of 0s and 1s.

Writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `output` | string | `"output"` | Channel the checker is written to |
| `scale_x` | number ≥ 0.1 | `8.0` | Squares across; an even count when seamless |
| `scale_y` | number ≥ 0.1 | `8.0` | Squares down; an even count when seamless |

### `gradient_texture` (Gradient Texture)

Write a gradient across the texture (a ramp: it never tiles).

Writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `gradient_type` | `linear` \| `quadratic` \| `easing` \| `diagonal` \| `spherical` \| `quadratic_sphere` \| `radial` | `"linear"` | Its shape: left to right (linear, quadratic, easing), corner to corner (diagonal), out from the centre (spherical, quadratic_sphere) or around it (radial) |
| `output` | string | `"output"` | Channel the gradient is written to |

### `wave_texture` (Wave Texture)

Write bands of sine, saw or triangle waves, optionally bent by noise: wood grain, ripples.

Writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `detail` | integer 0–16 | `0` | Octaves of the bending noise; 0 turns distortion off |
| `direction` | `x` \| `y` \| `diagonal` | `"x"` | The way the bands run across: x varies left to right, y top to bottom |
| `distortion` | number ≥ 0 | `0.0` | How far noise bends the bands; needs detail above 0 |
| `output` | string | `"output"` | Channel the waves are written to |
| `scale` | number ≥ 0.1 | `4.0` | Bands across the texture; whole when seamless (even along diagonal) |
| `wave_type` | `sine` \| `saw` \| `triangle` | `"sine"` | The profile of each band |

### `white_noise` (White Noise)

Write an independent random value (0–1) per pixel: grit, sparkle.

Writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `output` | string | `"output"` | Channel the noise is written to |

### `voronoi_texture` (Voronoi Texture)

Write a Voronoi (Worley) distance field, and optionally each pixel's cell.

Writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `cell_output` | string |  | Channel each pixel's cell number is written to; unset, none is |
| `exponent` | number ≥ 0.1 | `2.0` | The minkowski metric's exponent: 1 is manhattan, 2 euclidean; read only with metric = "minkowski" |
| `feature` | `f1` \| `f2` \| `smooth_f1` \| `distance_to_edge` | `"f1"` | What is measured: distance to the nearest point (f1), the second nearest (f2), a rounded f1, or distance to the cell's edge |
| `metric` | `euclidean` \| `manhattan` \| `chebyshev` \| `minkowski` | `"euclidean"` | How distance is measured: round cells (euclidean), diamonds (manhattan), squares (chebyshev), or minkowski with exponent |
| `output` | string | `"output"` | Channel the distance (0–1) is written to |
| `randomness` | number 0–1 | `1.0` | How far each cell's point strays from a regular grid: 0 a grid, 1 fully random |
| `scale` | number ≥ 0.1 | `4.0` | Cells across the texture |

### `musgrave_texture` (Musgrave Texture)

Write fractal noise of Musgrave's kinds: fBm, multifractal, ridged, hybrid, hetero terrain.

Writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `cell_offset` | bool | `false` | Offset the noise per cell (reads the cell id), so neighbouring stones don't share a pattern |
| `dimension` | number | `1.0` | Fractal dimension (H): higher weakens the fine octaves, for a smoother result |
| `frequency` | number ≥ 0.1 | `4.0` | Noise cells across the texture in the first octave; rounded to whole cycles when seamless |
| `gain` | number | `2.0` | How strongly one octave drives the next (ridged and hybrid) |
| `lacunarity` | number ≥ 0.1 | `2.0` | How much finer each octave is than the last |
| `musgrave_type` | `fbm` \| `multifractal` \| `ridged_multifractal` \| `hybrid_multifractal` \| `hetero_terrain` | `"fbm"` | How octaves combine: summed (fbm), multiplied (multifractal), as sharp ridges (ridged_multifractal), or rougher where higher (hybrid_multifractal, hetero_terrain) |
| `noise_type` | `perlin` \| `simplex` \| `value` \| `voronoi` | `"perlin"` | The base noise: perlin, simplex, value, or voronoi (Worley cells) |
| `octaves` | integer 1–16 | `4` | Octaves summed |
| `offset` | number | `1.0` | Added to each octave before combining (ridged, hybrid and hetero terrain) |
| `output` | string | `"output"` | Channel the noise (about 0–1) is written to |
| `scale_x` | number ≥ 0.01 | `1.0` | Horizontal squeeze of the noise: above 1 packs it tighter across |
| `scale_y` | number ≥ 0.01 | `1.0` | Vertical squeeze of the noise: above 1 packs it tighter down |

### `invert` (Invert)

Turn a channel's values over (v becomes 1 − v), fully or partway.

Reads `<input>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `factor` | number 0–1 | `1.0` | How far toward 1 − v: 0 leaves the values, 1 inverts them |
| `input` | string | `"input"` | Channel to invert |
| `output` | string | `"output"` | Channel the result goes to |

### `brightness_contrast` (Brightness/Contrast)

Shift and stretch a channel about 0.5, clamped to 0–1.

Reads `<input>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `brightness` | number -1–1 | `0.0` | Added to every value |
| `contrast` | number -1–10 | `0.0` | Stretch about 0.5: the result is (v − 0.5)(1 + contrast) + 0.5 + brightness; −1 flattens to grey |
| `input` | string | `"input"` | Channel to adjust |
| `output` | string | `"output"` | Channel the result goes to |

### `hsv_adjust` (HSV Adjust)

Turn the albedo's hue and scale its saturation and value, in place.

Modifies `r`, `g`, `b`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `hue_offset` | number -1–1 | `0.0` | Turn of the hue, in whole turns of the colour wheel (0.5 is the opposite colour) |
| `saturation_factor` | number 0–3 | `1.0` | Saturation multiplier: 0 is grey, 1 unchanged |
| `value_factor` | number 0–3 | `1.0` | Value (brightness) multiplier: 1 unchanged |

### `gamma` (Gamma)

Raise a channel to a power, bending its midtones.

Reads `<input>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `gamma` | number 0.01–10 | `1.0` | The exponent: v^gamma, so above 1 darkens the midtones and below 1 lifts them |
| `input` | string | `"input"` | Channel to adjust |
| `output` | string | `"output"` | Channel the result goes to |

### `clamp` (Clamp)

Hold a channel's values between a minimum and a maximum.

Reads `<input>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `input` | string | `"input"` | Channel to clamp |
| `max` | number | `1.0` | Highest value let through |
| `min` | number | `0.0` | Lowest value let through |
| `output` | string | `"output"` | Channel the result goes to |

### `blur` (Blur)

Gaussian-blur a channel (two separable passes, wrapping at the edges).

Reads `<input>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `input` | string | `"input"` | Channel to blur |
| `output` | string | `"output"` | Channel the result goes to |
| `radius` | integer 0–64 | `2` | Pixels the kernel reaches each way; 0 copies the channel unchanged |
| `sigma` | number 0.1–32 | `1.0` | The Gaussian's spread, in pixels: larger is softer within the radius |

### `sharpen` (Sharpen)

Sharpen a channel with an unsharp mask.

Reads `<input>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `input` | string | `"input"` | Channel to sharpen |
| `output` | string | `"output"` | Channel the result goes to |
| `radius` | integer 1–64 | `2` | Radius of the blur it compares against, in pixels |
| `strength` | number 0–10 | `1.0` | How much of the difference from the blurred copy is added back |

### `edge_detect` (Edge Detect)

Write the strength of a channel's edges (Sobel): high where it changes fast.

Reads `<input>`; writes `<output>`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `input` | string | `"input"` | Channel whose edges are found |
| `output` | string | `"output"` | Channel the edge strength goes to |

### `edge_erode` (Edge Erode)

Nibble irregular chips into cell edges by displacing them with noise, here and there along each edge.

Modifies `cell_id`, `edge_dist`, `mask`.

| Key | Type | Default | What it does |
|---|---|---|---|
| `edge_width` | number 0–1 | `0.06` | Only pixels with edge_dist below this are moved |
| `noise_frequency` | number ≥ 0.1 | `40.0` | Frequency of the displacing noise: higher is finer, chippier edges |
| `noise_type` | `perlin` \| `simplex` \| `value` \| `voronoi` | `"perlin"` | The displacing noise: perlin, simplex, value, or voronoi (Worley cells) |
| `octaves` | integer 1–8 | `1` | FBM octaves of the displacing noise; 1 is the sharpest grain |
| `strength` | number 0–0.5 | `0.02` | How far an edge is pushed, in texture widths; fades toward the cell's interior |
| `threshold` | number 0–1 | `0.3` | How sparse the chips are: a slow noise gates the erosion, which happens only where it's above this; 0 erodes everywhere, 1 nowhere |

<!-- op-catalog:end -->

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
assert!(flint_procgen::validate_params(&schema, &spec.params).is_empty());

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
| `src/param_ui.rs` | Schema → editor fields, the catalogs' tables |
| `src/params.rs` | `validate_params`: a spec's params against its generator's schema, at every depth (#718) |
| `src/export_glb.rs`, `src/skinning.rs` | GLB export with LOD chains and skins |
| `src/resolver.rs`, `src/cache.rs`, `src/queue.rs` | The player's spec resolver, memory cache and frame-budgeted queue |
| `src/validate.rs` | Output validation against style-guide constraints |
| `crates/flint-procgen/examples/` | Galleries (`lsystem_gallery`, `colonization_gallery`, ...) and `param_catalog` |

`flint-procgen-ai` builds on this crate to write and refine specs from a prompt; the player never links it.
