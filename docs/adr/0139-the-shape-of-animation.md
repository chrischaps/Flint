# ADR 0139: The shape of animation: an `animation` core of clocks, keys and clips, with `flipbook` and `rig` built on it

Status: proposed, 2026-11-27 (#1613). Epic #1611 (Modularity, Phase 3).
Chris settled the shape in review on 2026-11-27 (decision 1, and the name
`flipbook`). The first draft put the clock in the kernel; he moved it out
(section 1 says why). The rest of the recommendations under "Decisions"
still wait for him.
Builds on [ADR 0136](0136-the-package-model.md) (the package model; its
section 7 row for `animation` is the one this record splits) and
[ADR 0137](0137-the-public-package-api.md) (registration and stages). It sits
beside #1612's record, which says how the animation stage and IK become
stages a package joins; this record says what is on each side of that seam.
Uses
[ADR 0134](0134-the-character-controller-stays-in-physics-with-fixed-action-names.md)'s
retired fields for the migration. Shapes #1620, #1621 and the 2D rig roadmap
(#1625).

## Context

`flint-animation` is one crate, 7,773 lines with its tests, and every game
links all of it. ADR 0136 lists it as the standard package `animation`,
whole. Read by what its modules use, it is four parts that barely touch:

| Part | Modules | Lines | Uses from the rest of the crate |
|---|---|---|---|
| Playback | `animator`, `playback_state`, `sequence`, `clip`, `sampler`, `player`, `sync`, `loader` | 2,644 | nothing |
| Nodes | `node_clip`, `node_sync` | 300 | playback; skeletal's track types |
| Skeletons | `skeleton`, `skeletal_clip`, `skeletal_sampler`, `skeletal_sync`, `blend`, `ik_pass` | 3,462 | playback |
| Flipbooks | `sprite_clip`, `sprite_sync` | 579 | **nothing**, not even playback |

(`lib.rs`, 466, holds `AnimationSystem` and touches all four. The shared
rule tests, `clip_rules_tests.rs`, are 322.)

What the grep shows beyond the line counts:

- **Playback already runs one clock for three kinds.** Property, node and
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
- **Keyframes are two systems.** Property tracks hold `[f32; 3]` values
  (rotation in Euler degrees) and sample in `sampler.rs`. Joint tracks hold
  vec3s and quaternions and sample with slerp in `skeletal_sampler.rs`. They
  share only `clip::Interpolation`.
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
  `sprite_clip.rs` refuses a clip without one, but `sprite_sync.rs` writes
  only `sprite.source_rect`, so the sprite draws its own texture (#1657).
- **`tween` is not in this crate.** The `tween` script module
  (`crates/flint-script/src/api/tween.rs`, 153 lines) and the
  `FieldTweens` it queues (`crates/flint-script/src/tween.rs`, 493) live in
  `flint-script`, on `flint_core::tween`. ADR 0136 lists the module with
  `animation` because it animates.
- **Outside the crate**, three crates name it: `flint-script` (the
  `animation` module through `animator`, and the two callbacks),
  `flint-player` (the frame's `update`, `drain_sprite_events`,
  `drain_sequence_cues`, `run_ik`, bone probes, bone matrices to the
  renderer, and scene loading: `.anim.toml`, `.sprite.toml`,
  `.sequence.toml`, skeletons and clips from glTF) and `flint-cli` (the
  model previewer, and `IkPass` in `render_stage.rs` and `render_still.rs`).
  GPU skinning is in `flint-render` and takes bone matrices as plain data.
- **No template animates.** Nothing under `crates/flint-cli/templates`
  names `animator`, `sprite_animator`, a clip file or a clip call. The 2D
  template's scene draws three `sprite` entities and moves none of them by
  clip.
- **Several things keep their own time.** The kernel's `GameClock`
  (`flint-runtime`) is the frame's time. Clips, sequences, `FieldTweens`,
  living's `BoilClock` and music's `Conductor` each keep their own cursor on
  top of it.

Moving the crate as one package would make every 2D game link skinning and
IK, and would freeze flipbooks outside the clock. Chris wants 2D skeletal
animation on the roadmap (#1625), so the shape has to leave room for it.

## Decision

### 1. Three packages, and the kernel keeps only its clock

```
kernel        GameClock (flint-runtime, as now): the frame's time
  └─ animation      standard. The generic part of playback: the cursor, keyframes and
     (the core)     interpolation, clips and tracks, layer and crossfade state,
                    sequences and cues, the `animator` component and its rules, the
                    clip-kind seam, property clips, tween
       ├─ flipbook  standard, requires animation. Sprite-sheet clips
       └─ rig       standard, requires animation. Nodes, skeletons, blending, IK,
                    bone probes, skinning
```

- **The kernel keeps `GameClock` and nothing else of animation.** ADR
  0136's kernel tier is "every game links it; kept small". Animation, and
  sprites, are features *many* games use, maybe *most*, but not all: a UI
  tool, a static diorama or a puzzle on fixed tiles animates nothing. The
  standard tier ("on unless a game drops it") is the place for that.
  Nothing in the kernel needs the playback code: `tween` is independent of
  it, no template animates, and the kernel crates that call it today
  (`flint-script`, `flint-player`, `flint-cli`) are the callers Phase 3
  turns into registration for every package (#1612).
- **The core names nothing a clip drives.** If it held flipbooks, it would
  depend on the `sprite` component, which leaves the kernel in Phase 4
  (#1665). If it held skeletons, it would depend on glTF import. So the
  dependencies run one way: `flipbook` and `rig` require `animation`,
  `animation` requires nothing but the kernel, and Phase 3's crate-graph
  rule (#1624) can hold that shape. A `flipbook` of a few hundred lines
  earns its place by keeping the core free of sprites, not by its size.
- **What a game lists:** a 2D game keeps `animation` and `flipbook` and
  writes `rig = false`. A 3D game keeps `animation` and `rig`, and drops
  `flipbook` unless it draws flipbook billboards. A game that animates
  nothing writes all three `= false`.
- **The playback cursor starts in `animation`.** It is the time position
  with speed, loop, pause and resume, a one-shot's end, and the cues it
  passes. If a package that isn't animation wants it later (music's
  `Conductor`, living's `BoilClock`, particles), then the cursor alone,
  about a hundred lines of `playback_state.rs`, moves down into the kernel
  beside `GameClock`. `music` shouldn't have to require `animation` to get
  a cursor. Until a second client exists, the seam would be a guess (the
  same rule as skinning, section 11).

### 2. Inside the shape, a clip kind is sorted by what it drives

Each consumer is decided by what its clips write:

- **Any field of the entity, named by the clip** (a property clip:
  `transform` or one named field). It names no component of its own, so
  it is the core's own kind, and the one that proves the core works with
  no consumer listed.
- **The sprite's frame** (a flipbook writes `sprite.source_rect`): `flipbook`.
- **A pose, many named transforms under the entity** (a node clip writes
  child entities found by name; a skeletal clip writes a skeleton's joints,
  which become bone matrices): `rig`. Only a pose has the ideas that make
  animation heavy: a rest pose to be additive against, a subtree to mask,
  joint-by-joint blending, IK chains, probes on a named bone, a skin.

### 3. Where each module goes

Every module of `crates/flint-animation/src/`:

| Module | Goes to | As |
|---|---|---|
| `animator.rs` | `animation` | unchanged: still the one file that spells `animator`'s keys |
| `playback_state.rs` | `animation` | the cursor, and the layer and crossfade state |
| `sequence.rs` | `animation` | sequences and cues; `seek_sequence` steps every clip kind (section 5) |
| `clip.rs` | `animation` | clips, tracks, keyframes, `Interpolation`, clip events |
| `sampler.rs` | `animation` | track sampling |
| `player.rs` | `animation` | the property clip registry and `advance` |
| `sync.rs` | `animation` | the property clip kind |
| `loader.rs` | `animation` | `.anim.toml` |
| `lib.rs` | `animation` | `AnimationSystem`, holding the clip kinds (section 5) |
| `clip_rules_tests.rs` | all three | the rule tests, run on each package's kinds |
| `sprite_clip.rs` | `flipbook` | rebuilt: the flipbook clip, `.sprite.toml` unchanged on disk |
| `sprite_sync.rs` | `flipbook` | rebuilt: the flipbook clip kind (section 6) |
| `node_clip.rs` | `rig` | node clips |
| `node_sync.rs` | `rig` | the node clip kind |
| `skeleton.rs` | `rig` | skeletons |
| `skeletal_clip.rs` | `rig` | skeletal clips and the joint track types nodes share (until #1664) |
| `skeletal_sampler.rs` | `rig` | joint track sampling (until #1664) |
| `skeletal_sync.rs` | `rig` | the skeletal clip kind, with layer composition and bone probes' `joint_position` |
| `blend.rs` | `rig` | pose blending |
| `ik_pass.rs` | `rig` | two-bone IK |

Outside the crate: `publish_bone_probes`
(`crates/flint-player/src/player_app/headless_animation.rs`) goes to `rig`
as a stage after the pose (#1612's stages). The previewer's and the render
commands' uses of skeletons and `IkPass` go through registration (#1621).
`FieldTweens` and the `tween` module go to `animation` (section 9).

Each package's crates go in `packages/<name>/crates/` (#1554):
`flint-animation` keeps its name and moves to
`packages/animation/crates/flint-animation`. `flint-flipbook` and
`flint-rig` are new, in `packages/flipbook/` and `packages/rig/`.

Ownership after the split:

| | kernel | `animation` | `flipbook` | `rig` |
|---|---|---|---|---|
| Crates | (`GameClock` in `flint-runtime`) | `flint-animation` | `flint-flipbook` | `flint-rig` |
| Components | `sprite` (until #1665) | `animator` | `sprite_animator` (until it retires) | `skeleton`, `ik_two_bone`, `bone_probe` |
| Archetypes | `sprite` (until #1665) | | `animated_sprite` | |
| Script modules | `sprite`, its drawing functions | `animation`, `tween` | `flipbook`: `play_sprite`, `stop_sprite`, `set_sprite_speed`, `is_sprite_playing` and their deprecated spellings | none today |
| Callbacks | | `on_animation_end`, `on_sequence_cue`, `tween_done` | | |
| Files | | `.anim.toml`, `.sequence.toml` | `.sprite.toml` | skeletons and clips from glTF |

This replaces ADR 0136 section 7's `animation` row with the last three
columns. It also moves `sprite_animator` and `animated_sprite` out of that
section's kernel row, and the line in #1621's scope that sends the
`animation` script module to `rig`: every one of that module's functions
(`play_clip`, `stop_clip`, `blend_to`, `set_anim_speed`, the `*_anim_layer`
family, `play_sequence`, `stop_sequence`) writes `animator` keys, so it is
the core's. The four sprite clip functions leave the kernel's `sprite`
module for `flipbook`'s; their names don't change.

### 4. All of the generic playback is the core's

The core takes playback, the crossfade's and the layers' state (their
clocks, weights and ramps), sequences, cues, keyframes, clips and
interpolation. Composing two clips' outputs into one stays with the kind
(section 5).

- None of it knows what a clip drives. A layer is a clip on its own clock
  with a weight that ramps; a crossfade is a second clip and a weight that
  ramps; a sequence is a list of timed `animator` writes.
- `animator.rs` stays one file. Splitting playback across packages would
  split the one place that spells `animator`'s keys (#733), and the key
  test with it.
- Sequences in the core are what flipbooks gain most: a sequence cue on a
  flipbook's frame (#1620's new test).

### 5. One clock state per entity; a clip kind samples and writes

Today each of the three syncs keeps its own map of `ClipPlaybackState` and
calls the rule set itself. After the move, the core's `AnimationSystem`
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

- The core registers **property**. `flipbook` registers **flipbook**, and
  `rig` registers **node** and **skeletal**, through ADR 0137's registry.
- **Which kind plays an entity's clip** is decided by the entity's shape,
  not by a name search across kinds. A flipbook plays only on an entity
  with `sprite`, a skeletal clip only on one with a registered skeleton,
  and a node clip only on one with a node map. A clip name that two kinds
  both know on the same entity is reported once, as a diagnostic naming the
  kind that plays it.
- **The order of a frame stays today's**: property, sequences, skeletal,
  node, flipbook. Kinds run at fixed slots, so a game with every package on
  steps exactly as now, and the goldens hold byte for byte. IK and bone
  probes follow as `rig`'s stage.
- **Seeking** (`seek_sequence`, the previewer's scrub) replays the sequence
  and steps every kind together, not the skeletal sync by name.
- **Composition is the kind's.** `rig`'s skeletal kind composes layers and
  crossfades as today (`blend.rs`). Its node kind may adopt them next,
  since nodes already share the joint track types. A kind that doesn't
  compose (`composes() == false`) plays the base clip. A layer written on
  its entity is reported once, the way ADR 0115 reports a value a reader
  can't use, instead of being carried silently.

### 6. Flipbooks become a clip kind, in `flipbook`

- **The cursor owns time; the flipbook owns frames.** The clip's length is
  the sum of its frames' `duration_ms`. The kind maps the cursor's time to
  the last frame whose start is at or before it, the same `>=` boundary
  `advance_frame` uses. `ClipPlaybackState` stays in seconds (`f64`).
- **Ping-pong is a fold, not a cursor mode.** A ping-pong clip's period is
  every frame forward, then frames `n-2` down to `1` back (the ends aren't
  shown twice, as now). The cursor loops over that period and the kind
  folds it, so the core learns nothing about flipbooks.
- **Looping is the clip's and the animator's together.** `loop_mode =
  "once"` in `.sprite.toml` plays once, and `animator.loop = false` makes
  any flipbook play once. With the default `loop = true`, the clip's own
  mode governs, so a migrated scene behaves as before.
- **A crossfade on a flipbook is a cut.** Frames are discrete, so
  `blend_target` (and a sequence's `blend` step) switches clip when the
  crossfade would start. Layers are not composed (section 5).
- **It writes only on change.** The kind writes `sprite.source_rect` only
  when the frame changes, as now, so a still sprite causes no
  change-detection churn.
- **The timing spec is `advance_frame`'s tests.** #1620 runs them against
  the new kind first. A frame that lands one step apart (an `f32`
  millisecond sum against an `f64` second at a boundary) is fixed in the
  kind, not by loosening the test.
- **Sprites gain** sequences and cues, `autoplay`, negative `speed`
  (reverse play), the cursor's pause and resume, and the layers' clocks for
  a later 2D composition.

### 7. `sprite_animator` merges into `animator`

Of the three options (merge, alias one to the other, keep both), **merge**.
A flipbook is a clip. Two components that both say "play this clip at this
speed" make a 2D game learn two sets of rules that differ by accident: a
pause restarts in one and resumes in the other. Keeping both would leave
flipbooks off the clock in all but name.

The migration, for #1620:

- `schemas/components/animator.toml` moves to `packages/animation/`. Its
  fields are unchanged.
- `schemas/archetypes/animated_sprite.toml` moves to `packages/flipbook/`
  and becomes `["transform", "sprite", "animator"]`, with
  `[...defaults.animator] speed = 1.0`.
- `schemas/components/sprite_animator.toml` stays for **one release**, in
  `packages/flipbook/`. Its three fields are listed under ADR 0134's
  `[component.sprite_animator.retired]`, with the hint "sprite_animator is
  animator now (ADR 0139): write [entities.<name>.animator] clip = ...
  playing = ...". A scene that still writes them gets `unknown-field`
  warnings ending "any more" with that hint, from `flint check` and on
  load. No new mechanism is needed.
- **An alias read for that release:** `Animator::of`, in `animator.rs`,
  reads an entity's `sprite_animator` when it has no `animator`, so old
  scenes keep animating while they warn. #1620 checks that a retired
  field's value survives the load for the alias to find. A follow-up
  removes the schema and the alias after the release.
- **The sprite clip functions keep their names** and move to `flipbook`'s
  script module, as the 2D names for the same calls. `play_sprite` is
  `play_clip`, `stop_sprite` is `stop_clip`, `set_sprite_speed` is
  `set_anim_speed`, and `is_sprite_playing` reads `animator.playing`. They
  write through `animator::write`, so the key test holds them. Renaming
  them again, a release after `sprite_play` became `play_sprite`, would
  churn every 2D script for nothing.
- **What a 2D game notices** (the changelog fragment says each):
  - `play_sprite` on a paused clip resumes it instead of restarting it. A
    finished one-shot still replays from its start.
  - `speed` takes `-10..10`.
  - `on_animation_end(clip)` fires for a one-shot of every kind, so a 3D
    script that defines it now hears skeletal one-shots too.
  - Sprites can take `autoplay`, `sequence` and `loop`.
  - A game that writes `flipbook = false` loses the four sprite clip
    functions; the drawing functions stay in the kernel.
- The book's sprites chapter (`docs/book/src/concepts/sprites-2d.md`) and
  the scenes in flint-examples and the games move to `animator` in the same
  release. The warnings name every scene that hasn't.

### 8. One event stream from playback

Playback reports two things for every kind: **a one-shot ended** (today
only flipbooks report it, as `SpriteAnimEndEvent`) and **a clip event
passed** (a property clip's `events`, found and dropped today, and a
flipbook frame's optional `event`, new). Scripts hear the first as
`on_animation_end(clip)` and sequence cues as `on_sequence_cue(sequence,
cue)`, as now. Delivering clip events to scripts, and naming their
callback, is #1656. With it, #1625's "fires a cue on a footstep" is a key on
a frame.

### 9. `tween` goes to `animation`

The `tween` module eases one field of one entity over game time: a two-key
property clip made by a call. By section 2 that is the core's own kind of
clip, so the module and `FieldTweens` move from `flint-script` into
`flint-animation`, with `tween_done`. A game that writes `animation =
false` loses `tween` too (UI fades made with it included). That is the
point of the switch: a game that drops animation means it. Folding tweens
and property clips into one mechanism is not part of this phase.

### 10. Keyframes: two systems now, one in the core next

The core owns `Interpolation`, the cursor and property tracks from #1620.
Joint tracks (vec3s and quaternions, with slerp) move to `rig` with
skeletons in #1621, as they are. Unifying them into one generic track in
the core is #1664: a value type with its own interpolation, one sampler,
and typed property targets (an `i32`, a `vec4`, a string). Without it, the
core's "keyframes" would be the property kind's alone, and `rig` would keep
a second track system for good. It isn't a blocker for #1620 or #1621; it
comes before #1625 needs typed targets.

### 11. Skinning: in `rig` until a second deform client; GPU skinning in `flint-render` until Phase 4

Skinning (bone matrices from a skeleton) stays inside `rig`'s skeletal
kind. It becomes its own part only when a second client deforms by bones;
a 2D mesh-deform rig (section 13) is the likely first. The GPU skinning
pipeline stays in `flint-render`, fed bone matrices as plain data, until
Phase 4's render-feature seam lets `rig` carry its own pass. A game with
`rig = false` links the pipeline but never fills it.

### 12. Sprites: animation leaves now, drawing in Phase 4

Sprites, like animation, are a feature many games use, not every game.
Sprite *animation* leaves the kernel now, as `flipbook`. Sprite *drawing*
can't yet: `flint-render` reads the `sprite` component in
`scene_renderer/extract.rs`, `scene_renderer/mod.rs` and `model_loader.rs`,
and a renderer feature leaves only through Phase 4's seam (ADR 0118).
#1665 makes `sprite` a standard package then, and `flipbook` adds it to its
`[requires]`.

Phase 3's gate holds meanwhile. The 2D template draws sprites and animates
nothing, so it plays on the kernel alone with every standard package
dropped. Once #1665 lands, the template lists `sprite`, and the gate's
wording says so.

### 13. The paper test: two 2D rigs walked through the shape

No code. Each part of a 2D rig, and where it would live. The roadmap issue
is #1625.

**A cutout rig** (bones as entities, sprite parts as their children, the way
Spine and DragonBones animate most characters):

| Part | Lives in | How |
|---|---|---|
| Bones | kernel (entities, `transform`, hierarchy) | each bone an entity; the root carries the `animator` |
| Parts (slots) | `sprite` (kernel until #1665) | each part a child entity of its bone with a `sprite`; a slot *is* an entity |
| Walk, run, idle clips | `rig` (node clips) | a node clip keyed by bone name drives the bones; the importer registers the node map as glTF scenes do now |
| Blending walk into run | `rig` (pose composition) | node clips adopt the skeletal kind's crossfade and layers; nodes already share `JointTrack` |
| Playback, crossfade timing, sequences | `animation` | unchanged |
| Footstep cue | `animation` (clip events, #1656) | an event on the clip, or a sequence cue |
| Foot and hand IK | `rig` (`ik_pass`) | `ik_two_bone` on bone entities works today; a 2D chain is a chain whose pole lies in its plane |
| Draw order | `animation` (a stepped property track on `sprite.layer`) | **gap:** property tracks write floats, and `sprite.layer` is an `i32` (#1664) |
| Attachment swaps (open hand, fist) | `animation` (a stepped track on the part's `sprite`) | **gap:** no property track writes a `vec4` (`source_rect`) or a string (`texture`) (#1664); until then a one-frame flipbook per attachment does it |
| Reading Spine, DragonBones or Flint's own TOML | a `rig-2d` package (requires `rig`) | the importer builds entities, sprites and node clips; nothing it makes is new to the runtime |

**A mesh-deform rig** (one sprite mesh whose vertices are weighted to
bones, bent smoothly at a joint):

| Part | Lives in | How |
|---|---|---|
| Bones | kernel (entities) | the same bone entities as a cutout rig, so one rig can mix parts and meshes |
| Clips, blending, IK, playback | as for cutout | unchanged |
| The skin: bone matrices from bones | `rig` (skinning) | **gap:** `Skeleton` is built from glTF's joints today; a deform rig needs a skeleton whose joints are bone entities. This is section 11's second deform client, where skinning becomes its own part |
| The weighted sprite mesh | `rig-2d` (data) | vertices, UVs on the sprite's texture, four weights each |
| Drawing it bent | `rig-2d`'s render half, Phase 4 | **gap:** the skinned pipeline is the 3D PBR path; a 2D skinned-sprite pass waits for the render-feature seam |

What the walk-through shows: **the 2D-only ideas need no 2D-only runtime.**
Slots are entities, attachments are sprites, and draw order is a sprite
field. The core's tracks move all three. What is 2D-only is an importer
and, for deform, a mesh and its pass. So `rig` holds nothing 2D, and a
`rig-2d` package that requires `rig` holds the importer and the 2D skin
when #1625 starts. A 2D game with flipbooks only lists neither; a 2D game
with a cutout rig lists `rig` and `rig-2d`. The three gaps (typed track
targets, a skeleton of entities, the 2D skinned pass) go to #1625, the first
by way of #1664. None of them is Phase 3's.

## Alternatives considered

- **Move `flint-animation` whole as the `animation` package.** One move,
  no rewrite. But every 2D game that wants a flipbook links skinning and
  IK, and flipbooks stay off the clock with their own rules. Rejected: it
  freezes the wrong shape, the one the epic says the moves must not.
- **The clock in the kernel, with `rig` as the one package** (this
  record's first draft, from the epic's wording). It gives a 2D game the
  same benefit. But it puts a feature most-but-not-all games use into the
  tier every game links, and nothing in the kernel needs it. Chris moved
  the clock out in review.
- **Flipbooks in the `animation` core.** One package fewer. But the core
  would then name `sprite`, which leaves the kernel in Phase 4, and every
  3D game that keeps `animation` would link sprite-sheet code.
- **Split by dimension (`animation-2d`, `animation-3d`).** It sorts by the
  game, not the clip. A 3D game's flipbook billboards and a 2D game's
  cutout rig land in the wrong half, and playback is written twice.
- **Bare playback in the core, with layers, crossfades and sequences in
  `rig`.** A smaller core, by about 1,100 lines (`sequence.rs` alone is
  1,082). But `animator.rs` splits across two packages, sprites never get
  sequences or cues (the reason to move them), and a sequence's `layer` step
  would live in a package a 2D game doesn't have.
- **Alias `animator` and `sprite_animator` to one another, or keep both.**
  No migration. But two components with the same three fields and
  different rules for pause, loop and speed stay forever. The one-release
  alias in section 7 gives old scenes the same safety without keeping the
  split.
- **Ping-pong as a cursor mode.** It would put a flipbook's idea into the
  cursor every kind shares. As a fold in the flipbook kind, the cursor
  stays three numbers (time, speed, loop).
- **The cursor in the kernel now.** Music, living and particles keep time
  too, but none has asked to share a cursor, and their clocks differ (the
  conductor follows the audio clock, ADR 0135). It moves down when a second
  client appears (section 1).
- **Skinning as its own package now.** It has one client. Splitting it
  before a 2D deform rig exists would be guessing at the seam.

## Consequences

- The kernel holds no animation code; `GameClock` stays where it is.
- A 2D game links `animation` and `flipbook`, and no skeleton, blend, IK or
  skinning code once #1621 lands. With `rig = false`, its `cargo tree` has
  no `flint-rig` (#1621's done-when). A game that animates nothing drops
  all three.
- Flipbooks follow the rules the other kinds follow (#732, #733), so
  sprites get sequences, cues, `autoplay`, reverse play and resume.
- The rules run once a frame per entity, instead of once per kind; the
  three per-kind state maps become one.
- `sprite_animator` warns for one release and then goes. The changelog
  fragment, the book and the examples move in #1620.
- `rig` is the home for 2D rigs. `rig-2d`, the importer's package, is
  #1625's to create.
- ADR 0136 section 7's `animation` row is replaced by the `animation`,
  `flipbook` and `rig` columns in section 3. Its kernel row loses
  `sprite_animator`, `animated_sprite` and the `tween` module. #1621's
  scope keeps the `animation` script module in the core.
- #1620 becomes "the `animation` package and `flipbook`", not "the clock
  joins the kernel"; #1621 adds `requires = ["animation"]`.

## Decisions

Each one is Chris's. Where he has decided, it says so; otherwise the
recommendation is written first.

1. **The shape** (section 1): the kernel keeps only `GameClock`; a standard
   `animation` core; `flipbook` and `rig` as standard packages that require
   it. **Decided by Chris, 2026-11-27**, with the name `flipbook`.
2. **The sorting rule** (section 2): a property clip is the core's own
   kind, a flipbook is `flipbook`'s, and a pose is `rig`'s. Recommended as
   written.
3. **Every module's place** (section 3), with `flint-animation` keeping
   its name in `packages/animation/`, new crates `flint-flipbook` and
   `flint-rig`, and the four sprite clip functions moving to `flipbook`'s
   script module under their current names. Recommended as written.
4. **How much of playback is the core's** (section 4): all of it.
   Composing layers stays with the kind. Recommended as written.
5. **One clock state and clip kinds** (section 5): the kind chosen by the
   entity's shape, today's frame order kept at fixed slots, seeking across
   every kind, and a layer a kind can't compose reported once. Recommended
   as written.
6. **Flipbooks as a clip kind** (section 6): seconds on the cursor,
   ping-pong as a fold, `loop` and the clip's mode together, a crossfade as
   a cut, and `advance_frame`'s tests as the timing spec. Recommended as
   written.
7. **`sprite_animator` and `animator`**: merge, alias one to the other, or
   keep both. Recommended: **merge**, with retired fields and an alias read
   for one release, and the behaviour changes in section 7 in the
   changelog.
8. **One event stream** (section 8): `on_animation_end` for every kind's
   one-shot; clip events delivered by #1656. Recommended as written.
9. **`tween`** moves to `animation` (section 9). Recommended as written.
10. **The cursor** starts in `animation` and moves to the kernel when a
    second, non-animation client appears (section 1); **keyframes** unify in
    the core in #1664 (section 10). Recommended as written.
11. **Skinning** stays inside `rig` until a second deform client exists, and
    GPU skinning in `flint-render` until Phase 4 (section 11). Recommended
    as written.
12. **Sprites** (section 12): drawing stays in the kernel until Phase 4,
    when #1665 makes it a standard package that `flipbook` requires.
    Recommended as written.
13. **2D rigs** (section 13): nothing 2D in `rig`; a `rig-2d` package that
    requires `rig`, for the importer and the 2D skin; the three gaps go to
    #1625. Recommended as written.
