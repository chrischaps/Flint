# Living Props beyond trees (#251): what else gets the woozy treatment, and how

Nov 5, 2026 · part of #206 · chooses the targets for #252 · no engine changes

So far one thing in Flint is woozy: the L-system tree. This note surveys everything else that could be, picks the likeliest few, and rules the rest out with reasons. Nothing here is a verdict on the look. #252 makes that call by eye. Where a route could be tried without code, I tried it, so the shortlist starts from evidence rather than guesses.

## Answers

- **Four targets, two of which cost nothing to try.** A vine already lives with a `[living]` table and no code. A procgen creature already takes shallow wooze with no code. Grass on the wooze clock is about a line of code. A noise pose is the one real prototype. It would give poses to anything with a fixed skeleton, which covers pine, willow and creatures together.
- **The cheapest first try is the vine.** Add `[living] lineage_seed = 7` to `garden_vine` and it drifts like a small oak, as its contact strip shows.
- **What ties the living path to trees isn't the runtime.** The pose ring, clock, echoes, field, species tuning and report are all generic. The tree-specific parts are the pose *source* (poses come from regrowing) and the skeleton's vocabulary (tubes and leaf cards, a still trunk, a fast layer for leaf cards only). The list is [below](#what-in-the-living-path-is-tree-specific).
- **Textures, terrain and a per-object boil are ruled out.** Particles and the mushroom are parked. Reasons are [below](#ruled-out).

## What #252 found

#252 tried all four targets. Verdicts, numbers and what to watch are in
`demo/woozy_grove_grass/README.md` (grass) and `demo/wooze_props/README.md` (the
rest). `python demo/wooze_props/experiment.py` reproduces them.

| # | Target | Verdict | Follow-up |
|---|---|---|---|
| 1 | `garden_vine`, living | **keep**: the stem holds and the shoots are redrawn. `demo/specs/grove_vine` is it | none for the wooze. The vine doesn't climb, which is a procgen question |
| 2 | beetle, shallow wooze | **keep** at 0.04 m / 0.35 m; deeper and whole-body swells parked. The showcase idle bob, on the wall clock, takes away its holds | #505: Rhai reads the field's clock |
| 3 | grass on the field's clock | **keep** (PR #259) | none |
| 4 | noise pose | **keep**: pine (colonization) and willow (grammar) live, limbs attached, silhouette kept. At regrowth's strength it reads as wind. Colonization depth breaks per-level stagger | #506: the noise pose in the engine |

## What there is to make woozy

| Generator | Specs | Living today? | Why |
|---|---|---|---|
| `tree_v1`, built-in L-system | 12 | Yes | Every seed grows the same segments (#207) |
| `tree_v1`, space colonization | 2 (`pine_tree`, `garden_tree`) | No | A different structure for every seed |
| `tree_v1`, text grammar | 1 (`weeping_willow`) | No | A stochastic grammar changes structure; a fixed one never changes |
| `creature_v1` | 6 (`beetle` and the showcase bugs) | No | `generate` never reads its RNG (`_rng`, `creature/mod.rs:40`), so every seed grows the same creature and there's nothing to regrow a pose from |
| `texture_v1` | 19 | n/a | An image, not a mesh: it reaches the screen only on a material |

Three of the twelve L-system specs aren't trees: `showcase_game/specs/crypt_mushroom`, `garden_vine` and `library_root`. They're living-capable today. No rock, bench or building generator exists. Hand-made ones already take shallow wooze (#215).

Checked with `flint gen --dry-run --strict` on copies with a `[living]` table:

```
crypt_mushroom  lineage 7: 3 segments on 4 bones, 16 leaf cards, a ring of 8 poses
garden_vine     lineage 7: 9 segments on 10 bones, 48 leaf cards, a ring of 8 poses
library_root    lineage 7: 9 segments on 10 bones, 0 leaf cards, a ring of 8 poses
beetle          not a living prop: generator 'creature_v1' can't separate a structure from its poses
pine_tree       not a living prop: space colonization grows a different structure for every seed ...
garden_tree     (the same)
weeping_willow  not a living prop: a text grammar can't drift ...
```

## The shortlist

| # | Target | Route | How it should read | Cost | Evidence |
|---|---|---|---|---|---|
| 1 | **`garden_vine`** (and other L-system props) | living | Side shoots and leaves wander and resettle on the beat. The stem holds, so it stays on its pillar | **None:** a `[living]` table | A contact strip. Reads as one plant redrawn |
| 2 | **Creatures** (`beetle`, `water_strider`) | shallow | Shell and legs swell and settle with the grove, so a bug doesn't look pasted in beside a boiling tree | **None:** a `shallow_wooze` component on the baked GLB | Measured below: moves on 17 of 35 frame pairs, its still twin on 0 |
| 3 | **Grass** | time-driven (the field's clock) | The meadow sways in holds and glides, in step with the trees, instead of on the wall clock | **Low:** `frame.rs:1613` sets `renderer.grass_time = clock.total_time`. Feed it a scene clock's beats × step instead, read at the camera as `boil_at` already does. No shader change, since the sway is `sin(time × speed + phase)` | Not tried: it needs that line |
| 4 | **A noise pose** for pine, willow and creatures | living, with a new pose source | Branches or limbs settle into a new held shape each beat. The risk is that it reads as jelly rather than another drawing of the same thing, because the drift isn't the generator's own | **Medium:** a `full_pose` that turns each rest segment by a seeded, spatially smooth noise instead of regrowing. It goes in the `living_spike` example first, not the engine | Not tried |

**Water and sky follow #3.** The ocean's waves run on `ocean_time` (`frame.rs:1616`) and the clouds drift on `sky.time`, so both take the same clock swap. I'd try them after grass, since a held sea is a bigger stylistic step than a held meadow.

**#252 should try 1, 2 and 3 first.** They're scene, spec and one-line edits. 4 is the experiment that decides whether the living path generalises past L-systems, so it's worth a prototype but not before the cheap three have been seen.

### 1. `garden_vine`: living, no code

The strips and GIFs are on the #251 presentation page and in the PR. `renders/` isn't committed.

At the spec defaults (variance 0.35, steady 0.8, six echoes), the shoots and leaf cards drift and the stem holds. It reads as the oak's look at a smaller scale. To reproduce, copy `showcase_game/specs/garden_vine.procgen.toml`, append `[living]` and `lineage_seed = 7`, then:

```sh
flint gen garden_vine.procgen.toml --strip --frames 24 --fps 12 -o garden_vine-strip.png
```

What it hits: the stem is the tree's trunk, bone 0, and never drifts. That's right for a vine on a pillar.

### 2. Creatures: shallow wooze, no code

`flint gen specs/beetle.procgen.toml -o models/beetle.glb` bakes the skinned beetle. A scene with a `[wooze]` table and two beetles, one with `shallow_wooze { amplitude = 0.04, wavelength = 0.35 }`, renders with `flint render --time 0..3 --fps 12`. Mean frame-to-frame change: 0.365 on the woozy half (17 of 35 frame pairs change, about what `glide = 0.4` predicts) against 0.0003 on the still twin (0 of 35). A wavelength near the bug's size makes it change shape rather than move whole.

What it hits: nothing tree-specific, because shallow wooze is generic. The showcase bugs also run `creature_idle.rhai`, a wall-clock bob and sway. #252 should watch whether that continuous motion fights the held beat. If it does, the script can read `delta_time` on the field's clock, or drop the bob.

### 3. Grass: the field's clock

The grass already moves, and it moves continuously. Beside trees that hold and glide, it's the same "pasted in" problem shallow wooze solved for benches. The clock is per scene, read at the camera, like the shading boil's. A per-patch clock would need the grass compute to know about zones, which isn't worth it until one clock reads wrong.

What it hits: nothing in `LivingGenerator`. It needs a scene clock in the player (`ShallowClock` already counts beats from the field).

### 4. A noise pose

This is the only candidate that touches the living path itself. Structure stays whatever the lineage seed grows: a colonization tree, a grammar tree, or a creature's bones read as segments with body parts bound rigidly like leaf cards. A pose turns each segment by smooth 3D noise sampled at the segment's rest midpoint and seeded by the shape seed, then goes through `with_variance` as now. It's spatially smooth so that neighbouring twigs bend together. It isn't new to the epic: it's what shallow wooze does to vertices, done to bones instead.

What it hits: everything in the next section's first four rows.

## What in the living path is tree-specific

Generic already, so a new target gets these free: the `LivingGenerator` trait's shape, found through `as_living()` on any registered generator (`living_props.rs:2377`); the pose ring, cache and budget; the warped clock; echoes; the wooze field and zones; shadow modes; species tuning and the F3 panel; `flint render --time` and `--report json`.

Tree-specific, and what each would have to become:

| Where | Assumption | Hit by |
|---|---|---|
| `TreeGenerator::full_pose` (`tree/living.rs`) | A pose comes from **regrowing** from the shape seed, and must give the same depth list | Colonization, grammars, creatures (#4). Needs a pose source that doesn't depend on the generator's own randomness |
| `TreeGenerator::check_living` | Only the built-in L-system can live | Pine, garden_tree, willow (#4) |
| `LivingSkeleton` (`living.rs`) | Parts are `BranchSegment` tubes plus `leaf_mesh` / `leaf_bindings`, the only rigid attachment kind | Creatures (#4): body parts would have to ride as "cards" |
| `SegmentPose` (`living.rs`) | A pose is a turn of the rest direction plus length and mean-radius scale: a tube's vocabulary | Creatures (#4): an ellipsoid body part has no three-axis scale |
| `rig` bone 0 `trunk` (`tree/living.rs`) | The trunk is held still and never drifts | `library_root`, whose trunk is most of the prop, so it barely moves (its strip is on the presentation page). Fine for the vine |
| `VarianceConfig::weight` (`spec.rs:428`), `Stagger::PerLevel` | Depth from the trunk decides how much a part may drift, and when | Creatures: depth from the root bone works, but "steady" means "the trunk side holds", which a body may not want |
| `keep_size` (`living.rs`) | One shared length scale keeps the box, since limbs stay on a trunk | Creatures, whose legs touch the ground: a box can stay still while feet sink |
| `JITTER_TARGETS = ["leaf_cards"]` (`spec.rs:100`), `CardBoil`, `BOIL_BONES` | The fast layer exists only for leaf cards | `library_root` and creatures get no fast layer at all |
| `LivingRig::first_leaf_vertex`, `leaf_parts`, the runtime tint (`living_props.rs`, `TINT_UNSENT`) | Leaf shades are the only parts that tint | Any prop whose colour wants to shift |
| `DRIFT_PARAMS` (`spec.rs:97`) | Drift names tree params (`branch_angle`, `lsystem_tropism`, `trunk_curve`), and nothing reads them yet | Anything that isn't a tree, once drift is read |
| `posed_mesh` | Rebuilds bark through `segments_to_mesh` | Nothing: it should stay per generator |

## Ruled out

- **A texture ring (`texture_v1` regenerated per pose).** The stone wall is 512² with three maps, about 3 MB a pose against about 6 KB for an oak pose, so 8 poses come to 24 MB per lineage. Blending two would need a two-texture material. And the layout ops (`voronoi`, `brick_grid`) take their own `op_<i>` forks, so a new shape seed would move the stones. That breaks identity, unless only the detail ops are reseeded.
- **A per-texture or per-object boil.** The shading boil (#214) is a screen-space pass over the whole frame, so it already moves every surface's paint and lines on the beat, textures included. A texture drift would duplicate it. A per-object boil would need an id or stencil mask that nothing writes.
- **Creatures by regrowth.** `creature_v1` ignores its RNG, so there are no shape seeds to pose from. Creatures get shallow wooze now (#2) and living through a noise pose (#4). As a side finding, the `flooded_garden` scene's comment that its water striders use a "random seed, each unique" is wrong for the same reason.
- **Space colonization by pinning the cloud.** #207 settled this: pinning the attraction cloud freezes the shape too.
- **Terrain.** It's drawn by its own pipeline, so shallow wooze doesn't reach it. And it's what everything stands on: moving the drawn ground away from the physics heightfield makes feet float or sink, and every prop on it would have to move with it.
- **Rocks, benches and buildings as generator output.** No such generator exists, and hand-made ones already take shallow wooze.

## Parked, not ruled out

- **Particles on the field's clock.** `simulate_to` is deterministic, so a warped time is easy to feed it. But a held clock freezes smoke mid-air, and the sim's collisions and sub-emitters would see a stepped `dt`. A particle also has no lineage to keep, which was the point of wooze. Try it after grass, and only if a grove's smoke reads as pasted in.
- **`crypt_mushroom` as a mushroom.** It lives (3 segments, 16 mesh leaf cards), but the spec is a stubby trunk with leaves on it, not a cap. All its motion is leaf cards around a still stem. A woozy cap needs a generator that knows caps. The library root is the other half of this lesson: a prop whose mass is the trunk barely moves.
