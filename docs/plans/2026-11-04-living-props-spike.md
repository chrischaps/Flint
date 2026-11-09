# Living Props spike (#207): does a tree keep its structure, and how does the dial read?

Nov 4, 2026 · part of #206 · throwaway work, no engine changes

## Answers

- **L-system trees keep their structure across seeds.** Every seed of both oak specs grows the same segments, at the same depths, with the same parent links, in the same order. Segment *i* of one seed is segment *i* of another, so poses can blend segment by segment.
- **Space-colonization trees don't.** Each seed grows a different number of segments (608 to 705) to a different depth (19 to 31 levels). They can't blend.
- **What varies on an L-system tree is the leaves and a few bark vertices.** Leaf card counts change with the seed (85 to 130 on the small oak) because placement uses coin flips and length-based cluster counts. Bark vertex counts differ by up to 8 across seeds with identical segments, so vertex-level blending of the baked mesh won't line up.
- **Blend space: per segment (per bone).** The spike blends each segment's direction and length, then re-chains each child to its parent's blended end. Connectivity holds at every frame with no mesh correspondence needed. This is the per-bone blend #212 plans, so the skeleton design stands.
- **Space colonization: later.** Pinning its attraction cloud to the lineage would fix its structure and also its shape, leaving nothing to drift. Living Props should start with L-system specs only, and colonization trees get shallow wooze (#215) for now.
- **First read on the dial:** see "Frame strip" below.

## Counts across 8 seeds

`living_spike counts` grows each spec at seeds 1 to 8 with the generator as it is (`generate_tree` plus `generate_leaves`, LODs off) and compares each seed's (depth, parent) list with seed 1's.

| Spec | Method | Segments | Depths | Tips | Leaf cards | Bark verts | Total verts | Same structure |
|---|---|---|---|---|---|---|---|---|
| `specs/oak_tree.procgen2.toml` | L-system | 27 (all seeds) | d1:7 d2:12 d3:8 | 18 | 85 to 130 | 411 to 413 | 1091 to 1452 | 8 of 8 |
| `specs/examples/oak_tree.procgen.toml` | L-system | 243 (all seeds) | d1:11 d2:40 d3:80 d4:80 d5:32 | 162 | 2472 to 2624 | 2257 to 2265 | 22041 to 23250 | 8 of 8 |
| `specs/examples/pine_tree.procgen.toml` | space colonization | 608 to 651 | 29 to 31 levels | 157 to 185 | 5229 to 5814 | 4698 to 5021 | 57003 to 63025 | 1 of 8 |
| `showcase_game/specs/garden_tree.procgen.toml` | space colonization | 667 to 705 | 19 to 24 levels | 141 to 179 | 2136 to 2478 | 4539 to 4939 | 21627 to 24544 | 1 of 8 |

The code reading in the ticket was right: `stochastic_tree_lsystem` rewrites every `Forward` the same way, and the RNG only draws angles. The scaffold sort by limb size in `generate_lsystem_branches` is stable, and limb sizes don't depend on the seed, so the order survives too.

## Which params can drift without changing the structure

`living_spike drift` nudges every numeric param of a spec by ±20% (integers by ±1) at one seed.

- **Changes the structure:** only `lsystem_iterations`.
- **Keeps the structure and the leaf count:** every trunk param, `branch_angle_min/max`, `branch_radius_falloff`, `lsystem_angle_variation`, `leaf_size`, `leaf_size_variation`, `leaf_color_variation`, `radial_segments`, `bark_roughness`. `branch_levels` has no effect on the L-system path (the schema already marks it deprecated).
- **Keeps the structure but changes the leaf count:** `leaf_density`, `leaves_per_tip`, `leaf_along_branch_density`, `leaf_along_branch_min_depth`, `leaf_spread_radius`, `leaf_shades`, and on some seeds `branch_length_falloff` and `crown_radius` (along-branch clusters are counted from segment length).

So the drift params in the design's example (`branch_angle`, `lsystem_tropism`, `trunk_curve`) are safe, and so are the pose seeds themselves. Leaves have to be placed once per lineage and ride their segment (as the design already says), or any length change will pop cards in and out.

## The prototype

`crates/flint-procgen/examples/living_spike.rs`, `frames` mode:

- **Lineage seed** (default 7): the trunk (`fork("trunk")`) and the leaves (`fork("leaves")`, placed on pose 0).
- **Poses** (default 8): only the branches are regrown, each from `fork("pose<k>")` of the lineage, so they share the trunk they attach to.
- **Variance:** `--amount` pulls each pose toward pose 0, and `--steady` weights what's left toward the twigs: `w = amount × ((1 − steady) + steady × (depth / max_depth)^1.3)`, as in the look mock-up.
- **Warped clock** from the design: `t' = floor(t/s) + ease(clamp((frac(t/s) − (1 − g)) / g, 0, 1))`, with smoothstep for the ease. `--stagger` offsets each depth level by that fraction of a step.
- **Blend:** per segment, nlerp the direction, lerp the length and radii, and start each child at its parent's blended end. Each leaf card is bound to its nearest segment on pose 0 (parameter along it, offset from it) and rotated with that segment.
- **Output:** a GLB per frame, rebuilt from the blended segments with `segments_to_mesh`. Frames were rendered with `flint render` on lavapipe.

Reproduce (from the repo root, after `cargo build --release -p flint-cli -p flint-procgen --example living_spike`):

```sh
target/release/examples/living_spike counts specs/examples/oak_tree.procgen.toml specs/examples/pine_tree.procgen.toml
target/release/examples/living_spike drift specs/oak_tree.procgen2.toml --seed 1
target/release/examples/living_spike frames specs/examples/oak_tree.procgen.toml --out frames \
  --glide 0,0.3,1 --frames 24 --fps 12 --step 0.5 --stagger 0.15 --amount 0.35 --steady 0.8
```

There is no one-step way to render the GLBs as a sequence yet (#222). The spike put three trees in a scene with `models/tree_a.glb` and so on, copied each frame's GLBs over those, and ran `flint render` per frame.

## Frame strip

Three copies of lineage 7, left to right glide 0, 0.3 and 1, step 0.5 s, stagger 0.15, 12 fps, 2 seconds. Two runs: full variance (`--amount 1`) and the subtle setting from the mock (`--amount 0.35 --steady 0.8`). The strips and GIF are on the #207 Artifact page and in the PR.

First read from the stills, to be re-tuned by eye in the grove (#219):

- **Full variance is too much at any glide.** Each pose is a fresh tree, so the crown's mass jumps from side to side. At glide 0 it reads as a different tree every half second.
- **At 35% with twig weighting, all three read as one tree.** The silhouette holds and the motion lives in the outer branches and leaves.
- **Stagger matters more than glide for the drawn feel.** With depths offset, something changes on most frames even at glide 0, so the tree shimmers instead of popping. (Not rendered without stagger; by construction glide 0 would then be one hard cut per step.)
- **The sweet spot may sit higher than the design's first guess of 0.2 to 0.4.** With small variance, glide 0.6 to 1 plus stagger gives the near-constant drift that the mock-up's preferred preset had, and the hand-drawn edge should come from a separate fast, small jitter layer (the design's two amplitude layers) or shading boil (#214), not from snapping the big drift. Worth confirming by eye with the GIF.

## Notes for later tickets

- **#208/#209:** fork poses from the lineage by label and leave the trunk and leaf placement on the lineage. Give `BranchSegment` an id equal to its index on the L-system path; the index is already stable.
- **#209:** the bark mesh's vertex count varies by a few vertices across seeds, so a skinned rest mesh should be built once from the lineage pose, not per pose.
- **#212:** nlerp of segment directions plus length scale was enough to look right at this size; slerp only matters for large angle changes, which the variance setting keeps small.
- **Clock:** the spike used one clock for all three trees. Per-prop phase offsets are a one-line change and probably wanted, so the grove doesn't change in unison.
