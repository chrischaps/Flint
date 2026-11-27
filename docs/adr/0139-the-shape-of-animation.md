# ADR 0139: The shape of animation: a clip is sorted by what it drives, the clock is the kernel's, and a pose is the rig package's

Status: proposed, 2026-11-27 (#1613). Every recommendation under
"Decisions" at the end waits for Chris. Epic #1611 (Modularity, Phase 3).
Builds on [ADR 0136](0136-the-package-model.md) (the package model; its
section 7 row for `animation` is the one this record splits) and
[ADR 0137](0137-the-public-package-api.md) (registration and stages). Beside
#1612's ADR (0138), which says how the animation stage and IK become stages
a package joins; this record says what is on each side of that seam. Uses
[ADR 0134](0134-the-character-controller-stays-in-physics-with-fixed-action-names.md)'s
retired fields for the migration. Shapes #1620, #1621 and the 2D rig roadmap
(#1625).

## Context

`flint-animation` is one crate, 7,773 lines with its tests, and every game
links all of it. ADR 0136 lists it as the standard package `animation`,
whole. Read by what its modules use, it is four parts that barely touch:

| Part | Modules | Lines | Uses from the rest of the crate |
|---|---|---|---|
| The clock | `animator`, `playback_state`, `sequence`, `clip`, `sampler`, `player`, `sync`, `loader` | 2,644 | nothing |
| Nodes | `node_clip`, `node_sync` | 300 | the clock; skeletal's track types |
| Skeletons | `skeleton`, `skeletal_clip`, `skeletal_sampler`, `skeletal_sync`, `blend`, `ik_pass` | 3,462 | the clock |
| Flipbooks | `sprite_clip`, `sprite_sync` | 579 | **nothing**, not even the clock |

(`lib.rs`, 466, holds `AnimationSystem` and touches all four; the
shared rule tests, `clip_rules_tests.rs`, are 322.)

What the grep shows beyond the line counts:

- **The clock already is one clock, for three kinds.** Property, node and
  skeletal clips share one `ClipPlaybackState`, one `advance_time` and one
  rule set (`Animator::start`, `Animator::apply_to`, `animator::publish`;
  #732, #733). `animator.rs` is the one file that spells the `animator`
  component's keys, and a test holds it.
- **Flipbooks stand outside it.** `sprite_sync.rs` keeps its own state
  (`elapsed_ms` in `f32`, a frame index, a direction), its own loop and
  ping-pong (`advance_frame`), and its own component, `sprite_animator`
  (`clip`, `playing`, `speed`). So a sprite has no layers, no sequences, no
  cues, no `autoplay`, no reverse play, and a pause that restarts.
- **Nodes borrow skeletal types.** `node_clip.rs` is a list of
  `skeletal_clip::JointTrack`, sampled by `skeletal_sampler`. A node clip
  and a skeletal clip are the same thing aimed at different targets.
- **The layer and crossfade state is generic; using it is not.**
  `AnimLayer` (clip, weight, mode, mask, its own time and speed, a weight
  ramp) and the crossfade fields live in `ClipPlaybackState` and know
  nothing of joints, but only `SkeletalSync` composes them. `mask` names a
  root joint. Property and node clips carry the fields empty.
- **Sequences know only the animator.** `sequence.rs` writes `animator`
  keys through `animator::write` and emits cues. The one tie to skeletons is
  `AnimationSystem::seek_sequence` in `lib.rs`, which replays the sequence
  and the skeletal sync together so `flint edit` can scrub.
- **Three event paths, one of them dead.** A flipbook's one-shot end reaches
  scripts as `on_animation_end(clip)` (`call_sprite_anim_ends`). Sequence
  cues reach them as `on_sequence_cue(sequence, cue)`. A property clip's
  `events` are found by `player::advance` and then dropped: `sync.rs` never
  reads `AdvanceResult::events` (#1656).
- **A flipbook clip's `texture` is required and never used.**
  `sprite_clip.rs` refuses a clip without one; `sprite_sync.rs` writes only
  `sprite.source_rect`, so the sprite draws its own texture (#1657).
- **`tween` is not in this crate.** The `tween` script module
  (`crates/flint-script/src/api/tween.rs`) runs `FieldTweens` in
  `flint-script` on `flint_core::tween`. ADR 0136 lists it with
  `animation` only because it animates.
- **Outside the crate**, three crates name it: `flint-script` (the
  `animation` module through `animator`, and the two callbacks),
  `flint-player` (the frame's `update`, `drain_sprite_events`,
  `drain_sequence_cues`, `run_ik`, bone probes, bone matrices to the
  renderer, and scene loading: `.anim.toml`, `.sprite.toml`,
  `.sequence.toml`, skeletons and clips from glTF) and `flint-cli` (the
  model previewer, and `IkPass` in `render_stage.rs` and `render_still.rs`).
  GPU skinning is in `flint-render` and takes bone matrices as plain data.

Moving the crate as one package would make every 2D game link skinning and
IK, and would freeze flipbooks outside the clock. Chris wants 2D skeletal
animation on the roadmap (#1625), so the shape has to leave room for it.

## Decision

### 1. A clip is sorted by what it drives

The question to ask of a kind of clip is not "is it 2D or 3D" but "what does
it write":

- **The entity's own fields.** A property clip writes `transform.position`,
  `rotation`, `scale`, or one float on a named component. A flipbook writes
  `sprite.source_rect`. One entity, a few fields, no structure. These are
  the **kernel's**.
- **A pose: many named transforms under the entity.** A node clip writes
  the transforms of child entities found by name. A skeletal clip writes a
  skeleton's joints, which become bone matrices. Only a pose has the ideas
  that make animation heavy: a rest pose to be additive against, a subtree
  to mask, joint-by-joint blending, IK chains, probes on a named bone, a
  skin. These are the **`rig` package's**.

The clock that plays a clip, whatever it drives, is the kernel's.

### 2. Where each module goes

Every module of `crates/flint-animation/src/`:

| Module | Goes to | As |
|---|---|---|
| `animator.rs` | kernel | unchanged: still the one file that spells `animator`'s keys |
| `playback_state.rs` | kernel | the one clock state, with layer and crossfade state |
| `sequence.rs` | kernel | sequences and cues; `seek_sequence` steps every clip kind (section 4) |
| `clip.rs` | kernel | property clips, `Interpolation`, clip events |
| `sampler.rs` | kernel | property track sampling |
| `player.rs` | kernel | the property clip registry and `advance` |
| `sync.rs` | kernel | the property clip kind |
| `loader.rs` | kernel | `.anim.toml` |
| `sprite_clip.rs` | kernel | rebuilt: the flipbook clip, `.sprite.toml` unchanged on disk |
| `sprite_sync.rs` | kernel | rebuilt: the flipbook clip kind on the clock (section 5) |
| `lib.rs` | kernel | `AnimationSystem`, holding the clip kinds (section 4) |
| `clip_rules_tests.rs` | both | the rule tests run on property and flipbook in the kernel, and on node and skeletal in `rig` |
| `node_clip.rs` | `rig` | node clips |
| `node_sync.rs` | `rig` | the node clip kind |
| `skeleton.rs` | `rig` | skeletons |
| `skeletal_clip.rs` | `rig` | skeletal clips and the joint track types nodes share |
| `skeletal_sampler.rs` | `rig` | joint track sampling |
| `skeletal_sync.rs` | `rig` | the skeletal clip kind, with layer composition and bone probes' `joint_position` |
| `blend.rs` | `rig` | pose blending |
| `ik_pass.rs` | `rig` | two-bone IK |

Outside the crate: `publish_bone_probes`
(`crates/flint-player/src/player_app/headless_animation.rs`) goes to `rig`,
as a stage after the pose (#1612's stages). The previewer's and the render
commands' uses of skeletons and `IkPass` go through registration (#1621).

The kernel keeps the crate name **`flint-animation`**: what is left is what
"animation" means to every game, and `flint_animation::animator` keeps its
callers. `rig` is a new crate, `flint-rig`, in `packages/rig/crates/`.

Ownership after the split:

| | kernel | `rig` (standard) |
|---|---|---|
| Crates | `flint-animation` | `flint-rig` |
| Components | `animator` (with `sprite`, `sprite_animator` until it retires) | `skeleton`, `ik_two_bone`, `bone_probe` |
| Archetypes | `animated_sprite` | |
| Script modules | `animation`, `tween`, `sprite` | none today |
| Files | `.anim.toml`, `.sprite.toml`, `.sequence.toml` | skeletons and clips from glTF |

This replaces ADR 0136 section 7's `animation` row with two, and the line
in #1621's scope that sends the `animation` script module to `rig`: every
one of its functions (`play_clip`, `stop_clip`, `blend_to`,
`set_anim_speed`, the `*_anim_layer` family, `play_sequence`,
`stop_sequence`) writes `animator` keys, so it is the kernel's. `rig` is in
the **standard** tier: on unless a game writes `rig = false`, as every 3D
game today needs it without listing it.

### 3. All of the clock is the kernel's

The issue asks whether the kernel takes bare playback only, or layers,
crossfade and sequences too. **All of it:** playback, the crossfade's and
the layers' state (their clocks, weights and ramps), sequences and cues.

- None of it knows what a pose is. A layer is a clip on its own clock with
  a weight that ramps; a crossfade is a second clip and a weight that
  ramps; a sequence is a list of timed `animator` writes. Only *composing*
  two clips' outputs into one needs to know what they drive, and that stays
  with the kind (section 4).
- `animator.rs` stays one file. Splitting the clock would split the one
  place that spells `animator`'s keys (#733) across two packages, and the
  key test with it.
- Sequences are the point of the move for 2D. A sequence cue on a flipbook's
  frame (#1620's new test) needs sequences in the kernel.
- The cost is small. The clock is 2,644 lines, a third of the crate, and
  the heavy third (`skeletal_sync`, `blend`, `ik_pass`, `skeleton`) leaves.

### 4. One clock state per entity; a clip kind samples and writes

Today each of the three syncs keeps its own map of `ClipPlaybackState` and
calls the rule set itself. After the move the kernel's `AnimationSystem`
keeps one state per `animator` entity and runs the rules once. A **clip
kind** is the part that knows what its clips drive. The shape, for #1620 to
make real (names illustrative):

```rust
pub trait ClipKind: Send {
    fn name(&self) -> &'static str;                 // "property", "flipbook", "node", "skeletal"
    /// This clip's length if this kind plays it on this entity: the clip is
    /// one of its own, and the entity has what the kind writes.
    fn duration_of(&self, world: &FlintWorld, entity: EntityId, clip: &str) -> Option<f64>;
    /// Sample the entity's state (base, crossfade, layers) and write it.
    fn apply(&mut self, world: &mut FlintWorld, entity: EntityId, state: &ClipPlaybackState);
    /// Whether this kind composes layers and crossfades, or only plays the base.
    fn composes(&self) -> bool { false }
    fn forget(&mut self, entity: EntityId);
    fn clear(&mut self);
}
```

- The kernel registers **property** and **flipbook**. `rig` registers
  **node** and **skeletal** through ADR 0137's registry.
- **Which kind plays an entity's clip** is decided by the entity's shape,
  not by a name search across kinds: a flipbook plays only on an entity with
  `sprite`, a skeletal clip only on one with a registered skeleton, a node
  clip only on one with a node map. A clip name two kinds both know on the
  same entity is reported once, as a diagnostic naming the kind that plays
  it.
- **The order of a frame stays today's**: property, sequences, skeletal,
  node, flipbook. Kinds run at fixed slots, `rig`'s at the skeletal and
  node slots, so a game with every package on steps exactly as now and the
  goldens hold byte for byte. IK and bone probes follow as `rig`'s stage.
- **Seeking** (`seek_sequence`, the previewer's scrub) replays the sequence
  and steps every kind together, not the skeletal sync by name.
- Composition is the kind's: `rig`'s skeletal kind composes layers and
  crossfades as today (`blend.rs`); its node kind may adopt them next, since
  nodes already share the joint track types. A kind that does not compose
  (`composes() == false`) plays the base clip, and a layer written on its
  entity is reported once, the way ADR 0115 reports a value a reader can't
  use, instead of being carried silently.

### 5. Flipbooks become a clip kind

- **The clock owns time; the flipbook owns frames.** The clip's length is
  the sum of its frames' `duration_ms`; the kind maps clock time to the last
  frame whose start is at or before it, the same `>=` boundary
  `advance_frame` uses. `ClipPlaybackState` stays in seconds (`f64`).
- **Ping-pong is a fold, not a clock mode.** A ping-pong clip's period is
  every frame forward then frames `n-2` down to `1` back (the ends are not
  shown twice, as now); the clock loops over that period and the kind folds
  it. The clock learns nothing new.
- **Looping is the clip's and the animator's together.** `loop_mode = "once"`
  in `.sprite.toml` plays once; `animator.loop = false` makes any flipbook
  play once. With the default `loop = true`, the clip's own mode governs,
  so a migrated scene behaves as before.
- **A crossfade on a flipbook is a cut.** Frames are discrete, so
  `blend_target` (and a sequence's `blend` step) switches clip when the
  crossfade would start. Layers are not composed (section 4).
- **Writes only on change.** The kind writes `sprite.source_rect` only when
  the frame changes, as now, so a still sprite causes no change-detection
  churn.
- **The timing spec is `advance_frame`'s tests.** #1620 runs them against
  the new kind first; a frame that lands one step apart (an `f32` ms sum
  against an `f64` second at a boundary) is fixed in the kind, not by
  loosening the test.
- **Sprites gain** sequences and cues, `autoplay`, layers' clocks for a
  later 2D composition, negative `speed` (reverse play) and the clock's
  pause-and-resume.

### 6. `sprite_animator` merges into `animator`

Of the three options (merge, alias one to the other, keep both),
**merge**. A flipbook is a clip; two components that both say "play this
clip at this speed" make a 2D game learn two sets of rules that differ by
accident (pause restarts in one and resumes in the other). Keeping both
leaves flipbooks off the clock in all but name.

The migration, for #1620:

- `schemas/components/animator.toml` moves from the `animation` group to
  the kernel's in `packages/engine/package.toml`. Its fields are unchanged.
- `schemas/archetypes/animated_sprite.toml` becomes
  `["transform", "sprite", "animator"]`, with `[...defaults.animator]
  speed = 1.0`.
- `schemas/components/sprite_animator.toml` stays **one release**, its
  three fields listed under ADR 0134's `[component.sprite_animator.retired]`
  with the hint "sprite_animator is animator now (ADR 0139): write
  [entities.<name>.animator] clip = ... playing = ...". A scene that still
  writes them gets `unknown-field` warnings ending "any more" with that
  hint, from `flint check` and on load. No new mechanism.
- **An alias read for that release:** `Animator::of`, in `animator.rs`,
  reads an entity's `sprite_animator` when it has no `animator`, so old
  scenes keep animating while they warn. #1620 checks that a retired field's
  value survives the load for the alias to find. A follow-up removes the
  schema and the alias after the release.
- **The `sprite` script functions stay**, the 2D names for the same calls:
  `play_sprite` is `play_clip`, `stop_sprite` is `stop_clip`,
  `set_sprite_speed` is `set_anim_speed`, and `is_sprite_playing` reads
  `animator.playing`. They write through `animator::write`, so the key test
  holds them. Renaming them again, a release after `sprite_play` became
  `play_sprite`, would churn every 2D script for nothing.
- **What a 2D game notices** (the changelog fragment says each):
  `play_sprite` on a paused clip resumes it instead of restarting it (a
  finished one-shot still replays from its start); `speed` takes
  `-10..10`; `on_animation_end(clip)` fires for a one-shot of every kind,
  so a 3D script that defines it now hears skeletal one-shots too; sprites
  can take `autoplay`, `sequence` and `loop`.
- The book's sprites chapter (`docs/book/src/concepts/sprites-2d.md`) and
  the scenes in flint-examples and the games move to `animator` in the same
  release; the warnings name every scene that hasn't.

### 7. One event stream from the clock

The clock reports two things for every kind: **a one-shot ended** (today
only flipbooks report it, as `SpriteAnimEndEvent`) and **a clip event
passed** (a property clip's `events`, found and dropped today; a flipbook
frame's optional `event`, new). Scripts hear the first as
`on_animation_end(clip)` and sequence cues as `on_sequence_cue(sequence,
cue)`, as now. Delivering clip events to scripts, and their callback's name,
is #1656. With it, #1625's "fires a cue on a footstep" is a key on a frame.

### 8. `tween` is the kernel's

The `tween` module eases one field of one entity over game time. By section
1 that is the kernel's kind of animation, and it already lives in kernel
crates (`flint-script`, `flint_core::tween`) and names nothing of
`flint-animation`. It stays where it is. A tween and a property clip are
close cousins (a tween is a two-key clip made by a call); folding one into
the other is not part of this phase.

### 9. Skinning: in `rig` until a second deform client; GPU skinning in `flint-render` until Phase 4

Skinning (bone matrices from a skeleton) stays inside `rig`'s skeletal
kind. It becomes its own part only when a second client deforms by bones
(a 2D mesh-deform rig, section 10, is the likely first). The GPU skinning
pipeline stays in `flint-render`, fed bone matrices as plain data, until
Phase 4's render-feature seam lets `rig` carry its own pass. A game with
`rig = false` links the pipeline but never fills it.

### 10. The paper test: two 2D rigs walked through the shape

No code. Each part of a 2D rig, and where it would live. The roadmap issue
is #1625.

**A cutout rig** (bones as entities, sprite parts as their children, the way
Spine and DragonBones animate most characters):

| Part | Lives in | How |
|---|---|---|
| Bones | kernel (entities, `transform`, hierarchy) | each bone an entity; the root carries the `animator` |
| Parts (slots) | kernel (`sprite`) | each part a child entity of its bone with a `sprite`; a slot *is* an entity |
| Walk, run, idle clips | `rig` (node clips) | a node clip keyed by bone name drives the bones; the importer registers the node map as glTF scenes do now |
| Blending walk into run | `rig` (pose composition) | node clips adopt the skeletal kind's crossfade and layers; nodes already share `JointTrack` |
| Playback, crossfade timing, sequences | kernel (the clock) | unchanged |
| Footstep cue | kernel (clip events, #1656) | an event on the clip, or a sequence cue |
| Foot and hand IK | `rig` (`ik_pass`) | `ik_two_bone` on bone entities works today; a 2D chain is a chain whose pole lies in its plane |
| Draw order | kernel (a step track on `sprite.layer`) | **gap:** property tracks write floats and `sprite.layer` is an `i32`; tracks need a target that writes the field's own type |
| Attachment swaps (open hand, fist) | kernel (a step track on the part's `sprite`) | **gap:** no property track writes a `vec4` (`source_rect`) or a string (`texture`); the same typed-target change closes it, or a one-frame flipbook per attachment does today |
| Reading Spine, DragonBones or Flint's own TOML | a `rig-2d` package (requires `rig`) | the importer builds entities, sprites and node clips; nothing it makes is new to the runtime |

**A mesh-deform rig** (one sprite mesh whose vertices are weighted to
bones, bent smoothly at a joint):

| Part | Lives in | How |
|---|---|---|
| Bones | kernel (entities) | the same bone entities as a cutout rig, so one rig can mix parts and meshes |
| Clips, blending, IK, clock | as for cutout | unchanged |
| The skin: bone matrices from bones | `rig` (skinning) | **gap:** `Skeleton` is built from glTF's joints today; a deform rig needs a skeleton whose joints are bone entities. This is section 9's second deform client, where skinning becomes its own part |
| The weighted sprite mesh | `rig-2d` (data) | vertices, UVs on the sprite's texture, four weights each |
| Drawing it bent | `rig-2d`'s render half, Phase 4 | **gap:** the skinned pipeline is the 3D PBR path; a 2D skinned-sprite pass waits for the render-feature seam |

What the walk-through shows: **the 2D-only ideas need no 2D-only runtime.**
Slots are entities, attachments are sprites, draw order is a sprite field,
and all three are moved by kernel tracks. What is 2D-only is an importer
and, for deform, a mesh and its pass. So `rig` holds nothing 2D, and a
`rig-2d` package that requires `rig` holds the importer and the 2D skin
when #1625 starts. A 2D game with flipbooks only lists neither, and a 2D
game with a cutout rig lists `rig` and `rig-2d`. The three gaps (typed track
targets, a skeleton of entities, the 2D skinned pass) go to #1625; none is
Phase 3's.

## Alternatives considered

- **Move `flint-animation` whole as the `animation` package.** One move,
  no rewrite. Every 2D game that wants a flipbook links skinning and IK, and
  flipbooks stay off the clock with their own rules. Rejected: it freezes
  the wrong shape, the one the epic says the moves must not.
- **Split by dimension (`animation-2d`, `animation-3d`).** It sorts by the
  game, not the clip. A 3D game's billboard flipbooks and a 2D game's cutout
  rig land in the wrong half, and the clock is written twice.
- **Bare playback in the kernel, layers, crossfades and sequences in
  `rig`.** A smaller kernel, by about 1,100 lines (`sequence.rs` alone is
  1,082). But `animator.rs` splits
  across two packages, sprites never get sequences or cues (the reason to
  move them), and a sequence's `layer` step lives in a package a 2D game
  doesn't have.
- **Alias `animator` and `sprite_animator` to one another, or keep both.**
  No migration. Two components with the same three fields and different
  rules for pause, loop and speed stay forever, and the flipbook kind would
  read whichever one an entity has. The one-release alias in section 6 gets
  old scenes the same safety without keeping the split.
- **Ping-pong as a clock mode.** It would put a flipbook's idea into the
  clock every kind shares. As a fold in the flipbook kind the clock stays
  three numbers (time, speed, loop).
- **Skinning as its own package now.** It has one client. Splitting it
  before a 2D deform rig exists would guess at the seam.

## Consequences

- A 2D game links the clock, property clips and flipbooks, and no skeleton,
  blend, IK or skinning code once #1621 lands. With `rig = false` its
  `cargo tree` has no `flint-rig` (#1621's done-when).
- Flipbooks follow the rules the other kinds follow (#732, #733): sprites
  get sequences, cues, `autoplay`, reverse play and resume.
- The rules run once a frame per entity instead of once per kind; the three
  per-kind state maps become one.
- `sprite_animator` warns for one release and then goes. The changelog
  fragment, the book and the examples move in #1620.
- `rig` is the home for 2D rigs; `rig-2d`, the importer's package, is
  #1625's to create.
- ADR 0136 section 7's `animation` row is replaced by the kernel and `rig`
  rows in section 2, and #1621's scope keeps the `animation` script module in
  the kernel.

## Decisions

Each one is Chris's. The recommendation is written first.

1. **The factoring rule** (section 1): a clip that writes the entity's own
   fields is the kernel's; a clip that drives a pose of named parts is
   `rig`'s. Recommended as written.
2. **Every module's place** (section 2), with the kernel crate keeping the
   name `flint-animation`, `rig` a new crate `flint-rig` in the standard
   tier, and the `animation`, `tween` and `sprite` script modules in the
   kernel. Recommended as written.
3. **How much of the clock is kernel** (section 3): bare playback only, or
   playback with the crossfade and layer state, sequences and cues.
   Recommended: **all of it**; composing layers stays with the kind.
4. **One clock state and clip kinds** (section 4), the kind chosen by the
   entity's shape, today's frame order kept at fixed slots, seeking across
   every kind, and a layer a kind can't compose reported once. Recommended
   as written.
5. **Flipbooks as a clip kind** (section 5): seconds on the clock, ping-pong
   as a fold, `loop` and the clip's mode together, a crossfade as a cut, and
   `advance_frame`'s tests as the timing spec. Recommended as written.
6. **`sprite_animator` and `animator`**: merge, alias one to the other, or
   keep both. Recommended: **merge**, with retired fields and an alias read
   for one release, the `sprite` functions kept as the 2D names, and the
   behaviour changes in section 6 in the changelog.
7. **One event stream** (section 7): `on_animation_end` for every kind's
   one-shot; clip events delivered by #1656. Recommended as written.
8. **`tween`**: kernel or `rig`. Recommended: **kernel**, where it is.
9. **Skinning** stays inside `rig` until a second deform client exists, and
   GPU skinning in `flint-render` until Phase 4. Recommended as written.
10. **2D rigs** (section 10): nothing 2D in `rig`; a `rig-2d` package
    requiring `rig` for the importer and the 2D skin; the three gaps to
    #1625. Recommended as written.
