# Wooze props: what else gets the wooze (#252)

The experiment pass after #251's shortlist (`docs/design/living-props-beyond-trees.md`),
in epic #206. Grass was the first target (`examples/woozy_grove_grass/`, PR #259). This is
the rest: a vine, a beetle and a noise pose. Each one is the cheapest version that
could be judged, beside a still twin or the thing it would replace.

```sh
cargo build --release --bin flint --bin flint-player
cargo build --release -p flint-procgen --example living_spike
python examples/wooze_props/experiment.py      # everything, about 20 minutes, into renders/wooze_props/
cargo run --release --bin flint -- play examples/wooze_props.scene.toml
```

`experiment.py` renders `examples/wooze_props.scene.toml`'s variants with
`flint render --time 0..4 --fps 12`, plays the bob in `flint-player`, then runs
`noise_pose.py`. No engine code changed. The prototype lives in the
`living_spike` example and the rest is demo files: the existing scenes render
as they did, byte for byte.

## How it's measured

As for the grass: a crop around each target, frame pairs at 12 fps over 4 s, with
the shading boil off (`--kuwahara-boil 0 --contour-boil 0`), since the boil redraws
every pixel 12 times a second. `#` is a pair that changes and `.` is a hold. Each
target's still twin is its control. The field is the grove's `linklater` preset:
a 0.5 s step, so a beat is 6 frames.

## 1. The vine: `garden_vine` with a `[living]` table

`examples/specs/grove_vine.procgen.toml` is the showcase vine with the grove oak's
`[living]` table and nothing else. On the left pillar it lives, and on the right it's
the same lineage baked once.

| Variant | Knobs | Living vine | Baked twin |
|---|---|---|---|
| `authored` | variance 0.35 / steady 0.8, jitter 0.05, 4 echoes | 47/47, mean 4.57 | 0/47 |
| `vine_full` | variance 1.0 / steady 0.5 | 47/47, mean 9.71 | 0/47 |
| `vine_bare` | no echoes, no jitter: drift alone | 40/47, mean 5.01, `#####.` | 0/47 |

The stem holds, since it's the tree's trunk (bone 0), so the vine stays on its
pillar while the shoots swing and the leaves re-sit. It reads as the oak's look at a
smaller scale, the same plant redrawn, at either variance. The vine never rests:
the leaf jitter snaps every 1.5 frames and the echoes trail. Drift alone holds one
frame in six, because per-level stagger staggers its holds. The oak behaves the same
way, since it's the same `[living]` table.

What looked wrong is the vine itself, not the wooze, and the baked twin shows it too.
It doesn't climb: `tree_v1` grows a free-standing sapling, which stands in front of
the pillar rather than on it. And its leaf cards float off twigs thinner than a pixel
at this distance.

**Verdict: keep.** It costs a `[living]` table, and it's finished here as a demo
spec, so no follow-up is needed for the wooze. A vine that really climbs would need a
generator that follows a surface, which is a procgen question and not a wooze one.

## 2. The beetle: `shallow_wooze` on a creature

`examples/specs/grove_beetle.procgen.toml` is `specs/beetle.procgen.toml` with a fixed
seed (`creature_v1` ignores it anyway: #251). The left beetle has
`shallow_wooze { amplitude = 0.04, wavelength = 0.35 }` and the right one has none.

| Variant | Knobs | Wide shot | Close (`beetles`) |
|---|---|---|---|
| `authored` | 0.04 m in 0.35 m swells | 31/47, mean 0.73, `..####` | 31/47, mean 0.63 |
| `beetle_deep` | 0.08 m in 0.25 m swells | 31/47, mean 0.98 | 31/47, mean 1.04 |
| `beetle_whole` | 0.04 m in 3 m swells | 31/47, mean 0.59 | 31/47, mean 0.58 |

The still twin moves on 0/47 in every variant. The woozy beetle holds two frames in
six, on the field's beat, like the bench in the grove. From the close shot's stills:
with a swell near the bug's size (0.35 m against a 0.6 m body), the shell's outline
is redrawn a little each beat. At 0.08 m in 0.25 m swells the shell goes lumpy and the
head shifts, which is more than a beetle this small seems to want. At 3 m it keeps its
shape and shifts slightly as a whole: the bench's kind of wooze, less a redraw than a
nudge.

### The bob

The showcase creatures run `creature_idle.rhai`: a 3 cm bob and a sway on the wall
clock. `examples/scripts/beetle_idle.rhai` is the same motion. `experiment.py` plays both
beetles with it in `flint-player` (scripts don't run under `flint render`), from a
camera low over them, and once without it as the control.

Frame pairs in `flint-player` at 12 frames a second of game time (`--frame-step`),
boil off:

| Run | Woozy beetle | Plain beetle |
|---|---|---|
| no scripts (the control) | 41/48, mean 0.95, holds `..` on the beat | 33/48, mean 0.15 |
| both bob | 48/48, mean 1.18, never holds | 48/48, mean 0.63 |

The control's plain beetle shows the player's floor. Its crop changes by a fraction of
a level (0.15, against the bob's 0.63), and summed over the run the change isn't on
the beetle itself. Under `flint render` the same crop reads 0/47.

**Verdict: keep shallow wooze at 0.04 m / 0.35 m, as the scene has it. Park the deep
swell and the whole-body swell**, both of which are fair alternatives to try by eye.

**The bob fights the beat.** With it, the woozy beetle never rests (48/48), so the
held beat that makes it belong beside the trees is gone. A 3 cm bob on the wall clock
is bigger than the wooze, which is 0.04 m times linklater's amplitude of 0.5, so 2 cm.
A script can't fix this today: Rhai can set the field (`set_wooze`) but can't read its
clock. Follow-up: #505, a Rhai read of the field and its clock, so a bob can hold on
the beat.

## 3. The noise pose (prototype)

`living_spike noise` (`crates/flint-procgen/examples/living_spike.rs`) is a
`LivingGenerator` of its own. Its structure is whatever `generate` bakes, from any
branch method. A pose turns each rest segment by smooth 3D noise, sampled at the
segment's midpoint and seeded by the shape seed: a bend with no twist (`--turn`
degrees, `--wavelength` metres between swells), plus a small length change
(`--stretch`). It runs through the engine's own `PoseStream`, `with_variance` and
per-segment blending, so it moves the way a living tree would. `noise_pose.py`
renders each frame with `flint edit <glb> --render` from one fixed camera.

| Variant | Tree | Knobs | Frame pairs | Farthest tip move a frame |
|---|---|---|---|---|
| `oak_regrow` | grove oak, 243 segments | regrowth, as it lives now | 47/47, mean 7.74 | 0.82 m |
| `oak_noise` | the same skeleton | 12°, 3 m swells, variance 0.35/0.8 | 47/47, mean 4.02 | 0.16 m |
| `oak_noise_strong` | the same | 30°, amount 0.8 | 47/47, mean 8.37 | 0.90 m |
| `pine_noise` | pine, colonization, 640 segments | 12°, 3 m, per-level stagger | 47/47, mean 1.29 | 0.16 m |
| `pine_still` | the same | stagger none | 15/47, mean 2.77, `....##` | 0.16 m |
| `pine_bend` | the same | stagger none, 20°, 1.5 m | 15/47, mean 3.15 | 0.26 m |
| `pine_strong` | the same | stagger none, 30°, 2 m, amount 1.0 / steady 0.5 | 15/47, mean 6.75 | 1.39 m |
| `willow_noise` | willow, text grammar, 1380 segments | 12°, 3 m | 47/47, mean 3.87 | 0.15 m |

- **It generalises.** A colonization pine and a grammar willow, neither of which can
  regrow a pose, both live. Limbs stay attached through a glide, because a child
  starts at its parent's posed end.
- **It keeps the silhouette.** At the oak's own variance, a frame changes about half
  as much as under regrowth, and the crown keeps its outline. Regrowth swaps limbs
  around: the same tree drawn again by a different hand.
- **At regrowth's strength it reads as wind, not jelly.** At 30° the oak moves as far
  as it does under regrowth, but its whole crown leans one way, as if a gust had
  caught it. A 3 m swell turns a whole limb together. Shorter swells
  (`pine_bend`) bend limbs along their length instead.
- **Per-level stagger breaks on colonization.** A colonization tree's `depth` counts
  growth steps along a limb (31 on the pine), not branch order, so the stagger spreads
  it over 4.6 beats and it never holds. With no stagger it holds four frames in six,
  as the oak does.

**Verdict: keep, as the way pine, willow and the other colonization and grammar
trees live.** Follow-up: #506, a `[living]` noise pose in the engine, with a branch-order depth for colonization. Creatures by noise pose stay parked: their body
parts would have to ride as cards, and `SegmentPose` has no three-axis scale (#251).

## What to look at

Everything below is in `renders/wooze_props/` (`noise/` for the noise pose).

- **`authored_wide.gif`:** does the vine on the left read as the same plant redrawn?
  Do the vine and the beetle look as though they belong beside the oak, or is the
  baked twin on the right the one that looks pasted in?
- **`authored_beetles.gif` against `beetle_whole_beetles.gif`:** the woozy beetle is
  on the right in the close shot. Does a shell that swells and settles read as drawn,
  or does a beetle that shifts whole read better?
- **`bob.gif`:** both beetles bob, and the left one also has wooze. Does the
  continuous bob on top of the held beat read as alive, or as a stutter?
- **`noise/oak_regrow__oak_noise.gif` and `__oak_noise_strong.gif`:** regrowth on the
  left and noise on the right. Which looks like a new drawing, and which like wind?
- **`noise/pine_noise__pine_still.gif`:** per-level stagger on the left, none on the
  right. The left one never rests.
- **`noise/*_poses.png`:** four held poses side by side, for silhouettes.

Chris judges the feel. The measures say only when things move and how much.
