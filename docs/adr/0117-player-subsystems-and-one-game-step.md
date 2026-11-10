# ADR 0117: Player subsystems, named stages, and one game step shared with headless runs

Status: proposed, 2026-11-10 (#724, with #723, #725, #727, #745, #753, #754;
audit epics #673 and #675). Not accepted: the "Decisions needed" at the end are
Chris's to make before work starts.

## Context

`PlayerApp` was split once (#5), when it had about 38 fields. It now has 128
(`crates/flint-player/src/player_app/mod.rs:257-576`, at `54f1f6ba`). The
growth is optional features wired straight in:

- living props and wooze: 10 fields, plus `tick_procgen` (living props, the
  wooze clock, the boil clock, shallow wooze, grass wooze), `tick_hidden_side`
  and a second GPU submission for the A/B split;
- the music session: 4 fields, the first step of every frame, a ladder merge
  in every frame's post-override drain, an exit flag of its own, and
  drop-order rules spread through the transition code;
- the expression feed, the debug panels for one game's components, 16
  `pp_*` script overrides copied field by field.

`tick` itself (`frame.rs:998-1167`) is a readable 170-line schedule. The
problems are around it:

- **Every subsystem edits six files**: `mod.rs`, `init.rs`, `transition.rs`,
  `frame.rs`, `events.rs`, `headless_scripts.rs`.
- **Scene bring-up exists twice**, for startup (`init.rs`) and for
  transitions (`transition.rs:408-640`), and they have drifted: a scene
  reached by `load_scene()` ignores its own input config, keeps the last
  scene's post-processing and skybox, and a failed load leaves an empty world
  (#723, with #362 and #312).
- **The frame order exists twice**, in `tick` and in `HeadlessScripts`
  (`headless_scripts.rs:115-169`), which `flint render --scripts` uses. The
  headless copy omits the style reader, style tweens and several per-frame
  inputs, so scripts behave differently there (#725, with #579 and #581).
- **`PlayerExtension` can't host any of it.** It has `register_script_api`,
  `pre_update`, `on_window_event`, `on_key`, `ui`, `fonts` and `debug_panels`
  (`extension.rs:48-135`): no scene load or unload, no stage after scripts,
  physics or animation, no pre-render, no exit report.
- A game that uses none of these features still links and branches on all of
  them. The web build is held to 4 MB gzipped.

## Decision

1. **Stages are named in one enum**, in the order `tick` already runs them:

   ```rust
   pub enum Stage {
       Input, PreScripts, Scripts, Physics, PostPhysics,
       ScriptUi, Animation, Procgen, Sync, PreRender,
   }
   ```

2. **A `Subsystem` trait, internal to the player at first.**

   ```rust
   pub trait Subsystem {
       fn name(&self) -> &'static str;
       fn scene_loaded(&mut self, scene: &SceneFile, ctx: &mut SceneCtx) {}
       fn scene_unloading(&mut self, ctx: &mut SceneCtx) {}
       fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {}
       fn script_command(&mut self, cmd: &ScriptCommand, ctx: &mut StageCtx) -> bool { false }
       fn on_key(&mut self, key: &KeyInput) -> bool { false }
       fn pre_render(&mut self, ctx: &mut RenderCtx) {}
       fn exit_report(&self, report: &mut Report) {}
   }
   ```

   `PlayerApp` holds `Vec<Box<dyn Subsystem>>`. A subsystem is constructed
   when a scene has its component or table (a `living_prop`, a `[wooze]`
   table, a `music_session`), so a scene without it runs no code for it. A
   subsystem registers its own script module (ADR 0116), debug panels and
   debug keys.

3. **Scene bring-up is one function.** `enter_scene(SceneFile)` is called by
   startup and by a transition. Each subsystem and each built-in setting
   resets from the scene file, where an absent block means defaults. A failed
   load restores the previous scene or exits non-zero with the resolved path.

4. **The game step is one function without a window.** `GameCore` owns the
   world, clock, scripts, state machine, store, physics, animation and the
   subsystems, and has `step(dt, &Inputs) -> FrameOutputs`. `PlayerApp` adds
   the window, the renderer, audio output and the HUD. `flint render
   --scripts` and the playtest command (#714) drive the same `GameCore`.

5. **Then `PlayerExtension` gains the same scene and stage hooks**, so a
   game's Rust has what the engine's own features have. It is a superset
   change; existing extensions compile unchanged.

6. **Optional subsystems move behind Cargo features** (`music`, `living`),
   on the pattern `debug-hud` already uses, once they are behind the trait
   (#754).

## Steps

1. Group `PlayerApp`'s fields into `LaunchOptions`, `SceneSettings`,
   `PostOverrides` and `LivingState` structs. Mechanical, no behaviour
   change, and it takes the struct to about 40 fields.
2. `enter_scene`, shared by startup and transitions (#723). This is a bug
   fix and can land before the ADR is accepted.
3. `Stage`, `Subsystem`, and the expression feed as the first, simplest
   client. Then shallow wooze, then living props and wooze together.
4. `GameCore::step`, and `HeadlessScripts` deleted in its favour (#725).
5. The music session behind the trait and a feature (#745, after the gamepad
   wrapper moves in #748). Policies become a map keyed by subsystem name
   (#727).
6. The hooks on `PlayerExtension`; features (#754).

## Alternatives considered

- **A general ECS scheduler** (systems declared with their reads and writes,
  ordered by a graph). The usual engine answer, and far more than is needed:
  there are about ten stages, they are already in a fixed and reasoned order,
  and components are dynamic TOML, not typed storage a scheduler could
  borrow-check.
- **Only group the fields** (step 1 alone). Shrinks the struct and leaves
  six-file edits, the two bring-up paths and the two frame orders.
- **Widen `PlayerExtension` first** and write the engine's features as
  extensions. It would freeze a public trait before the engine's own
  subsystems have shown what the hooks must carry. Internal first, public
  second.

## Consequences

- **Borrowing is the hard part.** `tick` today reaches into `self` freely.
  The stage contexts have to lend the world, clock, input, renderer and
  script system separately, and living props touches most of them. Step 3's
  order (simplest client first) is there to find the context's shape early.
- **The A/B split takes two submissions a frame** and forks living state.
  It is the one case that may need a hook of its own in `pre_render`, or to
  stay special. Decide at step 3 with the code in hand.
- **Drop order.** The music session is declared first in `PlayerApp` so it
  drops before the audio engine (ADR 0017). The subsystem list must keep an
  explicit teardown order.
- **Dynamic dispatch per stage per subsystem per frame** is a handful of
  virtual calls; negligible, but `demo/phase4_runtime` and the woozy grove's
  `[living]` lines are measured before and after.
- Byte-identical frames are the gate at every step: the goldens, the
  `--frame-step` captures and `render_time_cli`.

## Decisions needed

1. **Internal trait first, public hooks second (recommended)**, or go
   straight to widening `PlayerExtension`?
2. **Does the rhythm layer stay in this repo** behind a feature, or move to
   the game's repo as an extension once the hooks exist? The ADR assumes it
   stays, behind a feature; moving it is cleaner for the engine and means the
   game carries about 8k lines.
3. **Is A/B compare (F5/F6/F7) worth keeping general?** It could become a
   general "two configurations, split screen" reviewer tool for looks and
   quality tiers, or stay a living-props tool behind its subsystem. The
   second is assumed.
