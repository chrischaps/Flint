# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Project Overview

Flint is a CLI-first, AI-agent-optimized 3D game engine written in Rust. The primary interface is CLI and code; visual tools validate results rather than create them. Phases 1-5 complete (ECS, constraints, PBR rendering, scripting/physics/audio/animation, AI asset generation). Game projects live in their own repositories with the engine as a git subtree.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run --bin flint -- <cmd> # Run CLI
cargo run --bin flint -- play demo/phase4_runtime.scene.toml  # Play a scene
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas  # Standalone player
cargo test                     # Run all tests
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --check              # Check formatting
```

## Development Workflow

Flint uses an **edit -> validate -> play** loop:

1. **`flint render`** -- Headless scene-to-PNG snapshot. Primary validation tool for AI agents.
   ```bash
   flint render levels/demo.scene.toml --output test.png --schemas schemas --width 1280 --height 720
   flint render levels/demo.scene.toml --output test.png --schemas schemas \
     --distance 20 --pitch 30 --yaw 45 --target 0,1,0 --no-grid
   # Debug/post-processing flags:
   #   --debug-mode wireframe|normals|depth|uv|unlit|metalrough
   #   --wireframe-overlay    --show-normals    --no-tonemapping
   #   --no-shadows           --shadow-resolution 2048
   #   --no-postprocess       --bloom-intensity 0.08  --bloom-threshold 1.0
   #   --exposure 1.5         --ssao-radius 0.5       --ssao-intensity 1.0
   #   --ssao-samples 64      (1-64; SSAO is the heaviest per-pixel pass — 16 is ~4x cheaper)
   #   --fog-density 0.02     --fog-color 0.7,0.75,0.82  --fog-height-falloff 0.1
   #   --dither-intensity 0.03   --desaturate 0.85
   #   --dof 0.6  --dof-focus 10  --dof-range 5
   #   --volumetric-density 1.0  --volumetric-samples 32
   #   --kuwahara-radius 4       --kuwahara-sharpness 8.0
   #   --kuwahara-hardness 8.0   --kuwahara-anisotropy 1.0
   #   --contour  --contour-width 1.25  --contour-threshold 0.08  --contour-strength 0.9
   #   --contour-luma 0.5  --contour-color 0.015,0.012,0.02  --contour-fade 30
   #   --kuwahara-boil 0.5  --contour-boil 1  --boil-on 2  --boil-time 0.5
   #     (shading boil, #214: boil needs the scene's [wooze]; --boil-time picks the drawing)
   #   --oren-nayar 0.7          --sheen-strength 0.15  --sheen-color 1,0.9,0.8
   #   --grade-lift 0.03,0.02,0.015  --grade-gamma 1,1,1  --grade-gain 1.04,1,0.94
   #   --film-grain 0.03         --grain-time 0  --fxaa
   #   --particle-time 2.0    (step emitters/effects deterministically before
   #                           capture; default none = no particles. ADR 0068)
   #   --msaa 4               (1|4; default 1 keeps pixel gates single-sample.
   #                           Also on flint-player. ADR 0058)
   #   --time 3.5             (living props, wooze field and other clocks at that game time;
   #                           byte-identical, and the same pixels as flint-player --frame-step
   #                           at that time. No scripts or physics. --frame-step S names the step)
   #   --time 0..4 --fps 12 -o x.gif   (a span as a GIF sharing one palette; a still pixel never flickers)
   #   --sweep wooze.glide=0,0.25,0.5,1  (labelled columns; also .step .amplitude .jitter .preset)
   #   --report json          (stdout: each living prop's lineage seed, pose indices, wooze values)
   # Note: --shadow-resolution defaults to 2048 (the renderer's construction
   # default) and is a real control since the texel-size upload (ADR 0049).
   #   --render-mode 1 --mode-mix 1.0 --mode-params 3,0,6,0
   #     (modes: 1=matrix 2=blood 3=drunk 4=tron [reality tears; params =
   #      mask scale, mask style 0 fbm/1 iris, rate, spare] 5=underwater
   #      [params = signed eye depth m, sea energy 0-1, daylight 0-1,
   #      biolum 0-1; masks by per-pixel waterline, not the tear mask])
   ```

2. **`flint edit <file>`** -- Unified interactive editor. Auto-detects file type and opens the right tool:
   ```bash
   flint edit levels/demo.scene.toml              # Scene viewer (hot-reload)
   flint edit levels/demo.scene.toml --spline      # Spline/track editor
   flint edit models/character.glb                 # Model previewer (orbit camera)
   flint edit models/character.glb --watch         # Model previewer with file watching
   flint edit specs/oak_tree.procgen.toml          # Procgen previewer (mesh/texture)
   flint edit specs/stone_wall.procgen.toml        # Texture pipeline editor (if pipeline pattern)
   flint edit terrain.terrain.toml                 # Terrain editor
   flint edit fx/fire.particles.toml               # Particle effect editor (curves, forces, bursts, scrub timeline;
                                                   #   --preset fire|smoke|sparks|rain creates a missing file;
                                                   #   --render out.png --anim-time 1.5 = deterministic snapshot)
   ```
   Supported extensions: `.scene.toml`, `.chunk.toml`, `.procgen.toml`, `.terrain.toml`, `.particles.toml`, `.glb`, `.gltf`.
   Common flags: `--width`, `--height`, `--no-grid`, `--watch`, `--seed`, `--no-inspector`, `--auto-orbit`.
   Scene viewer applies the scene's `[post_process]` block on load. **F4 opens the Rendering & Effects menu** (ADR 0053) — every render/post toggle plus its non-binary parameters (SSAO radius/samples, DoF, bloom, fog, grade/grain/FXAA, kuwahara, contour, shading boil, render modes, shadows + resolution, lighting levers, FOV, debug shading mode), an authored-vs-viewer-default post toggle, DoF-follow (focus plane tracks the last selected entity; values are plain view distances since the ADR 0055 depth fix), and a Particles section (the viewer simulates `particle_emitter` / `particle_effect` entities live; ADR 0068). WASD orbit, Q/E zoom — while an entity is selected, W/E/R switch gizmo mode instead of orbiting. It also seeds the orbit camera from the scene's authored `[camera]` block (Space returns to that framing; ADR 0051 in game repos using this pattern), and supports `--auto-orbit` turntable with O toggle and [ / ] speed like the model previewer.
   Model flags: `--clip <name>`, `--layer clip[:weight[:mask[:mode]]]` (repeatable), `--sequence <file.sequence.toml>`, `--sequence-loop`, `--anim-time <s>` (with `--render`; replays a sequence deterministically), `--pose-bone joint:x,y,z` (degrees, in the joint's frame, repeatable; no clip needed), `--show-skeleton`, `--anim-speed <f32>`, `--render <path.png>`, `--no-animate`, `--distance`, `--yaw`, `--pitch`, `--target`, `--fov`.
   A `*.sequence.toml` lists timestamped animator events (`blend`/`layer`/`speed`/`cue`); the previewer gets a Sequence panel (seek slider + event markers, R = restart), the player auto-loads them from `animations/` and scripts use `play_sequence` / `on_sequence_cue`. Layer weights can ramp via `fade_target`/`fade_duration` (`fade_anim_layer`).
   The model previewer has a Layers stack (clip/weight/Add-Over/mask/solo-mute per row), a "Skeleton colour" combo that paints the armature by last writer / layer weight / mask / keyed joints, and "Pose a bone" sliders (posed joints and everything past them draw orange).
   Scene flags: `--spline` (opens spline/track editor).
   Old commands (`serve`, `preview`, `gen-preview`, `tex-edit`, `terrain-edit`) still work as hidden aliases.

3. **`flint gen`** -- Run a procedural generation spec to produce meshes (GLB) or textures (PNG).
   ```bash
   flint gen specs/oak_tree.procgen.toml -o tree.glb
   flint gen specs/stone_wall.procgen.toml -o wall.png
   flint gen specs/oak_tree.procgen.toml --dry-run
   flint gen specs/examples/oak_living.procgen.toml -o oak.glb  # a [living] spec bakes its skinned rest (lineage_seed; --seed picks the lineage)
   flint gen specs/oak_tree.procgen.toml --seed 42 -o tree.glb
   flint gen --ai-create "twisted dead tree" -o tree.glb     # agent writes the spec (ANTHROPIC_API_KEY;
                                                             #   --ai-provider mock is offline and free)
   flint gen specs/oak_tree.procgen.toml --ai-assist --max-cost 0.50 -o tree.glb
   flint gen specs/oak_tree.procgen.toml --ai-refine --reference ref.png -o tree.glb
   #   Claude sees a headless render of what the spec builds each round and the refine
   #   loops while the measured distance falls (--refine-rounds N; #183)
   flint gen --cost-report --since 2026-10-01                # sums .flint/procgen_costs.json
   flint gen specs/examples/oak_living.procgen.toml --strip -o strip.png  # a [living] spec over time, as one labelled
                                                             #   contact sheet (hold-and-glide from the spec's step/glide/stagger,
                                                             #   mesh rebuilt per frame, one fixed camera; --frames 48 --fps 24
                                                             #   --columns N --cell-size 240; byte-identical per machine, #210)
   # --ai-model, --ai-iterations, --ai-generator,
   # --print-spec (spec alone on stdout), --save-spec. Every API call goes through
   # flint_procgen_ai::CostMeter: priced, appended to the JSON Lines ledger, and
   # refused before sending if its worst case could pass --max-cost.
   ```

4. **`flint play`** -- Full game runtime with physics, scripting, audio, and input.

**For AI agents:** After visual changes, always run `flint render` to verify before moving on. The render command loads models (searches `scene_dir/models/` then `../models/`), generates geometry, and computes the full transform hierarchy.

## Architecture

26-crate Cargo workspace (plus `tools/arch-analyzer`, a syn-based dependency/metrics extractor feeding the `tools/arch-viewer` web graph; neither is a default member):

```
flint-cli           CLI binary (clap). Commands: init, entity, scene, query, schema, edit, play, validate, asset, render, gen, prefab,
  │                 validate-suite, play-suite, calibrate, play-chart, replay-chart, render-suite, spike-rumble, bundle
  ├── flint-asset-gen AI asset generation: pluggable providers (Flux, Meshy, ElevenLabs, Mock)
  ├── flint-procgen   Procedural generation: Generator trait, registry, specs, GLB export
  └── flint-procgen-ai AI-assisted procgen: ProcGenAgent trait, spec creation/refinement
flint-android       Android entry point (NativeActivity, APK asset extraction)
flint-player        Standalone player (game loop, physics, audio, animation, scripting, egui HUD)
  ├── flint-music    Rhythm sessions: suite manifests, charts, tempo maps, ladder/seam/reintegration, replay, offline render
  ├── flint-input-capture 1 kHz gamepad capture thread (gilrs XInput) stamped against the audio clock; verb maps
  ├── flint-debug-ui DebugPanel trait + F3/F4 panel roster (Rendering & Effects, ocean, sky, camera, grass, particles, Living Props...); shared `widgets` (drag helpers, CurveEditor, GradientEditor); optional via debug-hud
  ├── flint-script   Rhai scripting with hot-reload
  ├── flint-particles Particle sim: effect assets, curves, forces, bursts, sub-emitters, deterministic stepping
  ├── flint-animation Property tween + skeletal animation
  ├── flint-audio    Kira spatial audio
  ├── flint-terrain  Heightmap terrain with splat-map blending
  ├── flint-physics  Rapier 3D (character controller, static/kinematic/dynamic bodies, impulse joints)
  ├── flint-runtime  GameClock, InputState, InputConfig, EventBus, GameStateMachine, PersistentStore
  ├── flint-viewer   egui inspector with transform gizmos, undo/redo, TOML write-back
  ├── flint-import   glTF model importer
  ├── flint-asset    Content-addressed asset catalog
  ├── flint-constraint Constraint evaluation + auto-fix
  ├── flint-query    PEG query language (pest parser)
  ├── flint-scene    TOML scene serialization/deserialization
  ├── flint-render   wgpu PBR renderer (winit 0.30, wgpu 23, HDR post-processing, bloom, SSAO, fog)
  ├── flint-ecs      hecs wrapper with stable EntityId mapping (BiMap)
  ├── flint-schema   Component/archetype schema system
  └── flint-core     EntityId, ContentHash, Transform, Vec3, Color, FlintError, vfs (read-only file layer)
```

## Key Design Principles

- **Dynamic components** stored as `toml::Value` -- archetypes defined at runtime in schema TOML files, not Rust types
- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load
- **Scene format is TOML** -- `[scene]` metadata + `[entities.<name>]` sections; reload is full re-parse
- **Schemas in `schemas/`** -- `components/*.toml` for field types, `archetypes/*.toml` for bundles. Multiple `--schemas` paths; later overrides earlier (game overrides engine)
- **Scripting via Rhai** -- `script` component with `.rhai` files in `scripts/`; callbacks: `on_init`, `on_update`, `on_collision`, `on_trigger_enter/exit`, `on_action`, `on_interact`, `on_draw_ui`, `on_scene_exit/enter`, `on_animation_end`, `on_sequence_cue`, `on_event` (custom events from `fire_event_data`, the engine or `ExtensionCtx::fire_event`); `ScriptCommand` for deferred effects; `DrawCommand` for immediate-mode 2D UI
- **Particles** -- inline `particle_emitter` or reusable `particles/*.particles.toml` effects via `particle_effect` (multi-emitter, curves, forces, bursts, sub-emitters; ADR 0068). Deterministic: per-emitter seeds from entity *names*, fixed-step `simulate_to`, `flint render --particle-time`. One shared `ParticleInstance` type; every consumer uploads through `SceneRenderer::update_particles_from` after `ParticleSystem::pack`. Scripts: `play_effect`/`stop_effect`/`set_effect_param`. F3 Particles panel in the player; F4 Particles section in the viewer
- **Living props** (epic #206) -- a `LivingGenerator` splits a prop into `structure(spec, lineage_seed)` (segments with stable `id`/`parent`, leaf cards bound to segments; a tree's is exactly what `flint gen --seed <lineage>` bakes) and `pose(spec, &skeleton, shape_seed)` (per-segment rotation, length and radius scale, relative to the structure). `tree_v1` supports the built-in L-system only; space colonization and text grammars report themselves unsupported. `pose` is `full_pose` held to `[living] variance` (#229): each segment pulled toward the skeleton by `amount × ((1 − steady) + steady × (depth / max_depth)^1.3)`, then with `keep_size` one shared length scale centres the box's sides on the rest pose's; `LivingSkeleton::with_variance` re-holds a `full_pose` without regrowing, and `VarianceConfig::FULL` is bit-identical to `full_pose`. `[living]` in a `*.procgen.toml` is validated at parse. `flint gen` on a supported spec bakes its skinned rest from `lineage_seed` (#209): bone 0 is the trunk, then one bone per segment, and `LivingGenerator::rig` returns the mesh with `LivingRig::palette`, which turns a pose or a blend into bone matrices. Reach it from a `dyn Generator` with `as_living()`. Book: concepts/procgen.md
- **All game UI is script-driven** -- no hardcoded HUD; `hud_controller` entity + `hud.rhai` pattern; engine provides draw primitives (`draw_text/rect/circle/line/sprite`), scripts compose UI
- **Game project pattern** -- games in own repos with engine as git subtree at `engine/`; `--schemas engine/schemas --schemas schemas` for layered overrides
- **Player extensions** (ADRs 0071-0075) -- a game binary embeds `PlayerApp` and installs `PlayerExtension`s (`add_extension`): `register_script_api` (native Rhai fns; same signature replaces Flint's), `pre_update(ExtensionCtx)` before scripts' `on_update` (world, clock, input, last frame's camera, `gpu` for `upload_mesh`/`remove_mesh`, `attach_mesh`, `fire_event` -> scripts' `on_event` same frame), `on_window_event` (only input events consumable) and `ui(egui)` drawn between script UI and the Foreground debug panels. `fonts(&mut FontDefinitions)` edits egui's fonts before each install (ADR 0081); `fonts/fonts.toml` `[egui] proportional/monospace` names egui's default faces, and every named family falls back to egui's fonts. With any extension installed, egui sees input first; a release follows its press. Runtime meshes are frustum-culled per draw by their AABBs (every game). Picking: `Camera::screen_to_ground` / Rhai `screen_to_ground`. Book: guides/player-extensions.md
- **Every runtime read goes through `flint_core::vfs`** (#97) -- `vfs::read` / `read_to_string` / `read_dir` / `exists` / `is_file` / `is_dir`, never `std::fs` or `Path::exists` in a runtime crate. The default source is the disk; the web player installs a `MemorySource` filled from a game bundle (`vfs::install`; `flint bundle <game>` packs one, `flint_core::bundle` is the format, `flint-player --bundle` plays one natively, audio arrives in a late bundle and `vfs::generation()` tells the player to load it; docs/design/web.md), and tests use `vfs::scoped` (this thread only). Decode images with `flint_render::open_image` (not `image::open`) and glTF through `flint_import::import_gltf` (not `gltf::import`), both of which read via the vfs. Writes (scene save, saves, replays) and hot-reload mtime polling stay on `std::fs`, natively; input overrides go through `flint_player::user_files` (OS config dir, or `localStorage` on the web, #100), and on the web so does the `persist_*` store (`save_<game_id>.toml`, #104). Tests and the CLI keep `std::fs`
- **Fixed-timestep physics** (1/60s) via accumulator; animation/particles run at variable rate
- **Physics joints** (ADR 0069) -- `joint` component (hinge/prismatic/spherical/fixed) on a dynamic entity, attached to `joint.parent` or the transform parent. Bodies are placed from `get_world_matrix` and dynamic poses write back as local `position` + `rotation_quat`; unit scale is assumed on the physics chain. TOML units are degrees/metres, converted to Rapier radians at sync. Scripts drive `motor_target` via `set_joint_target` and pose rest-quaternion nodes with `rotate_local` / `set_rotation_quat` (`set_rotation` still deletes the quaternion). Bench: `demo/joint_test.scene.toml`
- **Living props** (epic #206) -- a `living_prop` component on a model whose asset names a `[living]` spec. `flint_procgen::LivingProps` (owned by `ProcGenResolver`) keeps one skeleton and rig per spec and lineage, and per prop a ring of poses (plus one spare) a few steps ahead of its clock, filled by `process_living` after `process_frame` with whatever's left of the frame budget; poses have their own cache, since the resolver's queue dedupes by spec name. The player rewrites `model.asset` to `<spec>@<lineage>` and sets `runtime`, uploads through `load_skinned_model`, and gives each prop its own bone buffer. Shape seeds come from the entity's *name* plus the step index. `--exit-after` prints `[living]` lines on whether rings stayed full and within budget, and what blending cost. Each frame (#212) `LivingProps::blend_prop` reads every part's place on the warped clock, blends its two poses through `LivingPose::blend_from` (which the strip shares) and writes only what moved through `write_entity_bones`; 40 oaks cost about 0.35 ms. A scene's `[living]` table holds `shadows = "blended" | "drift"` and `echoes`. Leaf jitter fits in the palette: the last `BOIL_BONES` bones hold only an offset, and a card's vertices weight them by `n x r` (`CardBoil`, `LivingRig::runtime_joints`), which glTF forbids, so those weights go only to the renderer and never into the baked GLB. Leaf tints are quantized and set per part with `set_entity_part_tint`, so a grove shares a few materials. Echoes (#228): `[living] echoes = { count = 6, spread = 0.9, alpha = 0.2 }` in the spec, or field by field in a scene's `[living]`; `count = 0` is pixel-identical to none and costs nothing. Each echo is `LivingProps::blend_echo` (the prop's own blend at an offset time) into `write_entity_echo_bones`, which is what makes it exist; the renderer draws it front faces only with no depth write, after everything opaque, reusing the entity's transform and sharing materials by content. 40 oaks with 6 each add about 3.7 ms a frame, mostly palettes. With echoes on, `flint gen --strip` draws through the same skinned path. Bench: `demo/living_props.scene.toml`. Book: concepts/procgen.md
- **Two-bone IK** (ADR 0070) -- `ik_two_bone` component on the second bone of a transform chain names `target`, `pole` and `tip` entities; `IkPass` (flint-animation) solves after scripts/physics/animation each frame and before the renderer reads world matrices, writing only the two bones' `rotation_quat` from their captured rest. Lengths come from the model's rest offsets. `flint render` runs it once
- **Post-processing** -- HDR pipeline (`Rgba16Float`) with bloom, SSAO, fog, volumetric (god rays), Kuwahara (anisotropic painterly filter), vignette; `[post_process]` TOML block; F4 opens the Rendering & Effects debug menu in the player and viewer (all toggles + parameters, ADR 0053); when active, PBR shaders output linear HDR
- **`flint render` over time** (#216) -- `render.rs` loads a `Stage` per sweep column, each a world, renderer and clocks of its own. `flint_player::LivingRun` is the player's living-prop load and step with no window and no budget (`ProcGenResolver::step_living_now`), and only runs in a scene with `living_prop` entities, whose plain renders were empty before; every other scene's plain render stays byte-identical. To match the player it keeps the authored camera position (going through `update_orbit` rounds it, which changed 7,000 pixels at 1280 x 720) and draws no grid. `--screenshot-at` has a tolerance, since 210 steps of 1/60 sum to a hair under 3.5. Stdout is the JSON report's: flint-cli's tracing goes to stderr, and so do the load path's progress lines, so a new `println!` on that path breaks `--report json`. GIFs go through `gif_writer.rs`, one palette and transparent unchanged pixels, because `image`'s encoder shimmers still pixels. Not a new dep: `gif` and `color_quant` were in the tree through `image`
- **Wooze field** (#213) -- `flint_core::wooze::WoozeField::sample(position, time)` returns `{ amplitude, step, glide, jitter }`, the one answer to how woozy a place is. A scene's `[wooze]` table (global knobs, `preset`) and `[[wooze.zone]]` entries (`shape = "sphere" | "box"`, `center`, `radius` / `size`, `falloff`, any knob) become it through `flint_scene::WoozeDef::field`; `flint validate` fails on a bad zone, naming it, through `WoozeDef::check`, and the player warns and goes without it. No table, no field, nothing samples. In a scene with a field it decides `step` and `glide` for every prop, whatever the spec says. Rhai: `set_wooze(map[, seconds])`, `set_wooze_zone(name, map[, seconds])`, `remove_wooze_zone(name[, seconds])`; ramps are what make the field a function of time. A consumer must not pop on a change: count the clock in beats advanced by `dt / step`, and read glide and amplitude through a `flint_core::wooze::Cycle`, which holds what a part started its cycle with until the cycle ends. `amplitude` is applied per part with `SegmentPose::pulled`; `jitter` scales card turns, and goes into tints before they're quantized so materials stay few. Presets: `flint_core::wooze::PRESETS`, `linklater` a placeholder until #219. Schema: `schemas/components/wooze.toml`, which isn't a component. Bench: `demo/wooze_field.scene.toml`. Book: concepts/procgen.md
- **Living Props knobs** (#218) -- the F3 **Living Props** panel (`flint-debug-ui/src/living_props_panel.rs`, in any scene with a `living_prop`) and Rhai's `tune_living(map)`, `freeze_living(bool)`, `step_living()`, `save_living_preset(name)`, `load_living_preset(name)` share one path, `player_app/living_tuning.rs`: a change is a `flint_scene::WoozePreset` whose unset fields leave a knob alone, so a preset file loads exactly what a person turned. A ring keeps each pose twice (`RingPose`: `full` as it grew, `held` at the lineage's variance) and the cache only the first, so `LivingProps::set_variance` re-holds with no regrowing and reaches cached poses: poses in use across the grove first, then the rest taking turns with filling. While a knob moves, filling goes to the pose needed soonest (`pose_needed_soonest`), ahead of re-holds and of back-poses for waiting clocks; at rest rings fill as they always did, which keeps frames byte-identical. `set_echoes` / `set_stagger` reshape the ring to cover old and new clocks, and hold the new ones as `PendingClocks` until `ring.holds` what they read, so `frames_starved` counts only what's in use (`in_use_start`, not `ring.first`). Hidden echoes keep their buffers (`set_entity_echo_count`, opacity 0). A preset file is `wooze/<name>.wooze.toml` beside the scene or one level up; `load_scene` fills it into the scene's tables (`flint_scene::apply_preset_file`), so everything downstream only sees knobs, and the scene's own values win except for `[boil]` and stagger over a component's. Freeze is `living_dt` 0; a step is `LivingProps::advance_beats` before the tick, a whole beat per prop whatever its step. `demo/living_sweep.scene.toml` moves every knob: its `--exit-after` lines should say 0 frames missing a pose and 0 kept their last bones. Book: guides/debug-panels.md, concepts/procgen.md
- **Shallow wooze** (#215) -- a `shallow_wooze` component (`amplitude` metres, `wavelength` metres; schema in `schemas/components/`) on an entity with a mesh, or above several, in a scene with a `[wooze]` table: each vertex moves through world-space vector noise on the field's clock, in the main and shadow passes alike. `flint_core::wooze::ShallowClock` counts beats as a prop does, with no phase offset, so meshes in one place share one continuous displacement. `player_app/shallow_wooze.rs` gives each group one clock (the topmost component's), and `SceneRenderer::set_shallow_wooze` takes this frame's `ShallowWoozeDraw`s. Pay-nothing is the point, so don't add to `shader.wgsl` or `shadow_shader.wgsl` for this: woozy draws have their own vertex stages (`wooze_vertex.wgsl`, `wooze_shadow_shader.wgsl`, both prefixed with `wooze_noise.wgsl`) over the existing fragment stages, in `WoozePipelineSet`s and `ShadowPass::wooze`, built on the first woozy draw. The drift rides in padding: the last 48 bytes of a transform arena slot (every arena bind group now binds the whole 256-byte slot) and of a shadow draw slot. A plain `flint render` and the wireframe view show the rest pose; `--time` and `--sweep` draw it (`WoozeClocks`). 268 woozy entities cost about 0.6 ms a frame. A test mirrors the noise in Rust, so change both together. Bench: `demo/shallow_wooze.scene.toml`. Book: concepts/procgen.md
- **Woozy Grove** (#217) -- `demo/woozy_grove.scene.toml`, the walkable place the look gets judged: 38 living trees from `demo/specs/grove_{oak,cypress,birch,thorn}.procgen.toml` (L-system, no `[[lod]]`), a bench and lantern on one `shallow_wooze`, zones `hush` / `drowse` / `shiver` around a clearing that takes `preset = "linklater"` (#219: amplitude 0.5, step 0.5, glide 0.6, jitter 1, with the scene's `boil_on = 1`; a built-in holds only the field's knobs, so a scene with that preset and the default `boil_on = 2` boils at half the rate). `tools/living-strip/living_strip.py` makes the book's before/after strip and slit-scan from the scene: a slit-scan (one row of pixels stacked over time) shows a lurch, hold or glide in a still, which frame diffs and contact sheets don't. Judge a look's cost by the `[living]` lines, not fps: a grove that has run out of poses still reports 60 fps, each starved prop simply keeping its last bones. Check them on a long run and under CPU load too; a 30 s idle run once showed 0 where a loaded one showed 3,016 of 3,560 frames short, because `fill_within` refused every pose while its estimate sat just under the budget. `demo/woozy_grove_walk.txt` is a route through them for `--input-script`. `crates/flint-cli/tests/woozy_grove_cli.rs` keeps it loading. Book: concepts/procgen.md
- **Shading boil and contour** (#214) -- `contour_enabled` runs a screen-space ink pass after Kuwahara (`crates/flint-render/src/contour.rs`, `contour_shader.wgsl`): depth as the second difference of 1 / distance, so flat ground draws no lines, plus luminance, one full-screen draw, resources only once enabled (`ensure_contour_resources`). It draws in any scene. Boil is opt-in twice: `kuwahara_boil` / `contour_boil` above 0, and a `[wooze]` table. `flint_core::wooze::BoilClock` counts the field's beats, with six film frames to a beat so that `boil_on` 1, 2 and 3 all land on it, on game time. Whoever owns the clock stamps `SceneRenderer::set_boil(BoilFrame)` each frame (player, viewer, `flint render --boil-time`); the field is sampled at the camera, and `jitter` alone is the strength. A boil of 0 skips the noise entirely, which is what keeps it byte-identical. Bench: `demo/shading_boil.scene.toml`, where nothing else moves, so `--frame-step 1/24` frames come in byte-identical pairs on twos. Book: concepts/post-processing.md
- **Scene transitions** -- `TransitionPhase` lifecycle with script-driven visuals; `PersistentStore` survives transitions; `GameStateMachine` pushdown automaton with per-system `SystemPolicy`
- **Input system** -- TOML-based `InputConfig` with layered loading (built-in -> game -> user overrides -> CLI); keyboard/mouse/gamepad/touch unified via `Binding` enum
- **On-screen touch controls** (#133) -- an input config's `[touch_controls]` block (sticks, buttons, look region, menu; a later layer's block replaces the whole) is evaluated by `flint_runtime::TouchControls` inside `InputState` and painted by the player (`touch_overlay.rs`) on its own Foreground egui layer. The one engine-drawn UI, since a stick is a device: a finger on a stick/button skips egui and extensions (`taken_before_the_game`), is held by touch id and never reaches scripts' touch list; look drags feed `raw_mouse_delta` and are shared. Shown after a real touch, hidden by a key or pad (`--touch-controls auto|always|never`, web `?touch=`); geometry in points inside the web page's safe-area insets. `--input-script` has `touch`/`drag`/`lift`. A button's `value` (default 1; -1 drives an axis's other end) and scripts' `set_touch_controls(false)` (put away under a menu so taps reach it, #134); a finger counts for `any_input_just_pressed`. The shipped games' layouts live in their repos (Float, FlintKart `config/input.toml`). Book: concepts/touch-controls.md, guides/playing-on-mobile.md
- egui 0.30's `Color32::from_white_alpha` / `from_rgba_unmultiplied` premultiply in linear light, but the HUD blends in display space: a translucent white drawn with them reads several times too bright. Build translucent light colours with `from_rgba_premultiplied(a, a, a, a)`

## Critical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` -- needs `.into()` in tests
- wgpu v23: `Instance::new()` takes owned `InstanceDescriptor`; max 4 bind groups (0-3); `wgpu::Queue` is not `Clone` -- pass `&Queue` through, do not stash one in a struct
- wgpu v23 on DX12: the shader-visible sampler heap has 2048 slots and every bind group's samplers are copied into it, with no sharing, so samplers in a per-entity or per-material bind group cap how many can live ("Not enough memory left" in `create_bind_group`, DX12 only; test with `WGPU_BACKEND=dx12`). Keep samplers in groups bound once a frame: PBR materials sample with the light group's `material_sampler` (group 2, binding 4), and a material bind group holds none
- winit v0.30: `ApplicationHandler` trait with `run_app()`; `DeviceEvent::MouseMotion` for raw mouse delta
- Rapier v0.22: character controller types in `rapier3d::control`, NOT `rapier3d::prelude`
- Kira v0.11: `Decibels(f32)` and `PlaybackRate(f64)` are tuple structs; uses `glam` via `mint`
- `AudioManager::new()` / `Gilrs::new()` may fail in headless/CI -- wrap in `Option`
- Rhai v1.24 `sync`: `call_fn()` Scope visible only to direct callee, NOT sub-functions; entity IDs as `i64`; no implicit numeric coercion (`0.0` is NOT `0`); draw `_ex` functions take `layer` as int (`0`), not float (`0.0`)
- `ScriptCallContext` uses raw `*mut FlintWorld` -- only valid during `call_update()`/`process_events()` scope
- `on_update()` is parameterless -- use `delta_time()` API; `on_interact` checks `interactable.range` + `.enabled`
- `on_draw_ui()` ALWAYS runs even when scripts paused (pause menus need it)
- HDR format `Rgba16Float` -- all pipelines must use this when post-processing active; `resize_postprocess()` on window resize
- `render_draw_commands()` is a free function to avoid borrow conflicts; `load_pending_sprites()` before egui borrow
- Screen size for draw commands must be egui logical points, NOT physical pixels
- Skeletal animation: separate `SkinnedVertex`/`skinned_pipeline`; bone matrices in storage buffer (bind group 3)
- Particles: no depth write; one shared storage buffer indexed by `first_instance`; skip draw for 0 alive; `bytemuck::cast_slice` for zero-copy
- `flint-terrain` does NOT depend on `flint-render`; uses standard `Vertex` format so reuses shadow pipeline
- `flint-android` excluded from `default-members` -- use `cargo ndk -p flint-android`; min API 26. `cargo test --workspace` fails on `ndk-sys` on Windows; use plain `cargo test`
- Windows links a 1 MB main-thread stack; debug builds of flint-cli's clap parser overflow it -- `.cargo/config.toml` links with `/STACK:8388608`. Don't remove it, and don't grow `Commands` assuming stack is free.
- Scene entity `merge_component()` does field-level merge INTO archetype defaults, not full replacement
- `flint_core::toml_util` -- use `toml_f64`/`toml_f32`/`toml_vec3`/etc. instead of inline coercion patterns
- Never use `archetype = "furniture"` on non-visual entities -- furniture includes `bounds`, renders teal boxes
- Per-frame GPU allocations are the renderer's CPU cliff: `update_from_world` rebuilds the draw lists every frame, so anything it needs per entity must come from a cache (`draw_bind_cache` for transform/material binds keyed by entity + sub-mesh with a material hash, `box_mesh_cache` for procedural boxes, `GpuMesh` buffers shared via `Arc`, one dynamic-offset uniform buffer for all shadow draws). Never `create_buffer`/`create_bind_group` inside a per-entity loop; a 270-entity scene went from ~17 ms to ~4 ms when those were removed. Skinned draws use the same cache (`SKINNED_DRAW_KEY`, bone binds in `skinned_bone_binds`, #121). On the web every `create_*` is also a JS object: Float's skinned path made 386 buffers and 327 bind groups a frame, and Firefox stalled ~250 ms at each GC. Count WebGPU calls per frame in the browser before trusting a desktop timing (`?perf=1` shows them: the page wraps `submit`/`writeBuffer`/`createBindGroup`/`createBuffer`/`beginRenderPass`, #139)
- One submission a frame (#139): `SceneRenderer::encode_to` records the whole frame (grass, shadows, scene, post-process, depth upscale, world lines) into the caller's encoder, the player adds its HUD and the GPU timer's resolve (`encode_gpu_timings`), submits once, then calls `frame_submitted` (grass and timer read-backs). `render_to` is that with its own submit. Every `queue.write_buffer` lands before the submission, so a uniform buffer may hold one value per frame: steps that differ get slots of their own (bloom's per-step slots), and encoding two cameras into one submission would show the second camera's uniforms to both. Cached draws' transforms live in `TransformArena` pages (`bind_cache.rs`, a 256-byte slot each, one write per page); post-process bind groups live in `PostProcessResources::binds` (dropped on resize); ones over the caller's depth view are made per frame unless the scene draws at a render scale, since wgpu 23 gives a view no identity to key on
- Living Props A/B (#238) is the one place a frame takes two submissions: a split (`F7`, `--compare A,B --split`) submits the shown side's drawing, then writes the other side's bones and boil and draws it with `encode_again_to` into a `SplitView` whose half is copied over the frame byte for byte (both views are the swapchain format without its sRGB suffix). Each side is a `LivingProps::fork` with its own field, boil clock and shallow wooze (`player_app/living_ab.rs`); a flip (`F6`) swaps which is the scene's own and calls `redraw_all`, since the bone buffers held the other's. Anything a blend skips because "nothing changed" has to assume nothing about the renderer: a remembered tint of 0 once kept the other side's leaf tints. `F5` freezes the living clock
- Offline capture: `flint-player --frame-step S` advances the clock exactly S seconds per frame (wall time ignored), and with `--screenshot-every S` every frame is written; `--input-script FILE` plays timed `down`/`up`/`tap` keys, `axis` values and gamepad `press`/`release` (on pad slot 9) at game times (format in `player_app/input_script.rs`); a scripted key also goes through the debug keys (`handle_debug_key`), so `tap F2` opens the stats overlay, and `--stats` opens it from the first frame. Together they make deterministic, smooth gameplay footage and scripted regression runs
- Perf switches: `flint-player --exit-after N` prints `[perf] avg ms/frame`, the spread of dt and of present-to-present times (p50/p90/p99/max and frames over 1.5x p50), and the average draw calls, entity draws, culled draws and triangles on exit; `FLINT_VSYNC=0|mailbox` picks the present mode (Fifo can pace at a virtual display's rate, e.g. 7 Hz with Virtual Desktop attached); `FLINT_FRAME_LATENCY=1..3` sets the swapchain's frame latency (default 2); `FLINT_PACING=0` turns off vsync pacing; `FLINT_SHADOWS=0` disables the shadow pass at start; `--render-scale 0.25..1|auto` draws the 3D scene at that fraction of the window on each side (HDR chain + its own depth, upscaled by the composite, depth copied back so world text/lines still test against it) under a full-resolution HUD, `auto` stepping down to hold 60 fps; the web caps the scene at DPR 1.5 by default and takes `?scale=` (#122, `SceneRenderer::set_render_scale`); `--perf` prints a line every 10 s with each frame phase's CPU time, the worst scripts and each pass's GPU time, and the web page's `?perf=1` (`?perf=cpu` without GPU timing) shows the same in a corner with a Copy line button (#120, docs/design/web.md)
- Vsync pacing: under Fifo the player gives `GameClock` the monitor's refresh period (`set_vsync_period`, re-read when the window moves), and `delta_time` comes in whole refresh periods that keep game time on the wall clock. On a hybrid-GPU laptop Fifo lets frames through in bursts (6, 6, 6, then 45 ms at a 60 Hz mean); paced, every frame steps 16.7 ms. Stalls over 250 ms pass through, and frames faster than the refresh on average fall back to raw time. `raw_delta_time` is the unpaced wall time. See the book's Profiling guide
- CI's `wasm32 check` (`.github/workflows/ci.yml`) builds `flint-player` and every crate it links for `wasm32-unknown-unknown` on each PR. Run it locally with `bash scripts/wasm-check.sh` before pushing runtime-crate changes. It also denies `Instant::now`/`SystemTime::now` and their `elapsed()`, which compile on wasm but panic there: runtime code uses the `web-time` crate (a workspace dep: `web-time = { workspace = true }`), which #100 moved every older site to. Desktop-only deps in a runtime crate go under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`, as in flint-player's Cargo.toml (docs/design/web.md)
- Web frame loop (#121): `flint_web.rs` runs winit with `ControlFlow::Wait` and `about_to_wait` requests a redraw, so the player draws once per `requestAnimationFrame` and the main thread sleeps between frames. Never `ControlFlow::Poll` on the web: it chains `scheduler.postTask` tasks back to back, Chrome shrugs but Firefox starved its refresh driver (83 ms frames, multi-second stalls, half the audio buffers late). `?loop=poll` brings the old loop back for comparisons
- Web player (#102): draw the swapchain through `RenderContext::frame_view` (sRGB view of a plain WebGPU canvas), never a default view. Sound and pointer lock need a gesture: the page's Begin calls `flint_player::web_begin()`, and on wasm `set_cursor_captured(true)` is a wish that `sync_pointer_lock` turns into a lock while the page has activation; mouse look flows only while the lock is held. Headless Chrome never grants pointer lock, and headed Chrome only with the window focused (`Page.bringToFront`). Touch (#132): winit's web backend sends `pointerType == "touch"` as `WindowEvent::Touch` scaled by `devicePixelRatio`, and `touch_to_surface` rescales to canvas pixels (they differ under Chrome's device emulation on a scaled desktop). A `(pointer: coarse)` page asks for no pointer lock. Test with CDP `Emulation.setTouchEmulationEnabled` + `Input.dispatchTouchEvent`
- Web audio (#136): the browser mixes in an `AudioWorklet`, on the audio thread, running a second wasm (`crates/flint-audio-worklet`: kira only, no imports, `[profile.worklet]` opt-level 3, built by Trunk.toml's `pre_build` hook and copied beside the page). The pages share no memory with it (no COOP/COEP on Pages), so `AudioEngine` calls become `flint_audio::remote` commands posted once a frame and each sound crosses once as samples; handles are `engine::SoundHandle` / `SpatialTrack`. A new engine call that plays or changes sound needs a `protocol::Command` too. `?mixer=main` (or a scene with a `music_session`, which needs `manager_mut`) keeps the old main-thread cpal stream, which is also the fallback when the worklet fails
- play.chaps.dev (#103): `.github/workflows/play.yml` builds the player with trunk, packs every game in `tools/play-site/games.toml` (each pinned to a commit) with `flint bundle`, and deploys this repo's Pages from Actions. `?game=<id>` plays `games/<id>/<id>.flintpack`; `?embed=1` is the iframe view. `index.html`'s doorway shows the chooser, or the game's film when WebGPU is missing, before any wasm loads: `Trunk.toml`'s loader awaits `window.flintDoorway` and preloads only the JS glue. Keep the two together, or a browser that can't play downloads 7 MB of wasm (docs/design/web.md)
- Web size (#123): the site builds with `trunk build --release --cargo-profile web-release` (workspace `[profile.web-release]`: opt-level z, fat LTO, 1 codegen unit, panic = abort, with `opt-level = 3` package overrides for the hot crates and `+simd128` in `.cargo/config.toml` (#137); 3.6 MB gzipped, down from 5.1). Speed-build a crate only when the phone shows it hot: each override costs bytes, and the 4 MB gzip line holds. The workspace `image` dep is PNG/JPEG/HDR only and `flint-render` restores image's defaults natively, so add a format for desktop there, not in the workspace. rhai's `no_optimize`/`no_custom_syntax`/`only_i64` are on for wasm only (flint-script's Cargo.toml): web scripts see `i64` integers and nothing else. Measure a size change with trunk and record it in web.md's Size table
- `FlintWorld::spawn` ids come from a process-wide counter and scenes load from a `HashMap`, so entity ids differ run to run -- derive anything reproducible (seeds) from entity **names**, never ids
- Lights are `[entities.x.light]` with a `type`, and a directional `direction` points *toward* the light. No system reads a `directional_light` table and `flint validate` passes it, so a scene with only that gets `default_scene_lights` and no warning (several benches do). One point light and no directional means no sun at all
- A character always starts facing +Z (there's no starting yaw), so lay a scene's way in along it. It reads mouse look inside the fixed-step loop, so a frame of two physics steps turns it twice as far: `--input-script` routes with `look` need `--frame-step 0.0166667`, not 1/30

## Documentation Pipeline

The book is `docs/book` (mdBook), published to docs.chaps.dev by hand from this machine. There is no drift checker and no CI for the docs: when a change alters a command, flag, key or component the book describes, update the book in the same PR, checking against the code rather than memory.

```bash
mdbook build docs/book                                   # must be clean before any docs commit; output in docs/book/book/ (gitignored)
powershell -ExecutionPolicy Bypass -File docs/build.ps1 -SkipRustdoc -Serve  # preview at http://localhost:3000 (drop -SkipRustdoc for the API docs)
powershell -ExecutionPolicy Bypass -File docs/build.ps1 -Publish            # mdBook + rustdoc into ~/dev/docs.chaps.dev/flint, committed and pushed
bash docs/publish.sh --publish                           # the same in bash (--skip-rustdoc, --serve, --docs-repo <dir>)
```

Without `-Publish` / `--publish` the scripts build and copy into the docs repo but don't commit. `docs/publish.bat` is `build.ps1 -Publish`. Windows PowerShell 5.1 (`powershell`) runs `build.ps1`; `pwsh` may not be installed.

- Publishing pushes to the separate `docs.chaps.dev` repo (default `~/dev/docs.chaps.dev`; `-DocsRepo` / `--docs-repo` to change it). It is outward-facing: only on request.
- Renders for the book are made locally with `flint render` (no GPU in CI) and land in `docs/book/src/images/` in kebab-case.

## Project Structure

- `crates/` -- All workspace crates
- `schemas/` -- Engine component and archetype TOML definitions
- `styles/` -- Style guide TOML definitions for AI generation
- `demo/` -- Showcase scenes, scripts, audio, animations, `particles/` effect assets
- `testGame/` -- Test project (levels/, schemas/)
- `docs/book/` -- mdBook source; `docs/build.ps1` / `docs/publish.sh` -- build, preview and publish it
- `docs/design/flint-design-doc.md` -- Full design document for remaining phases

<!-- claims:begin (from chrischaps/Dashboard skills/setup-claims; re-run its install.sh to update) -->

## Claiming issues

Agents on several machines work this repo, all as the same GitHub user, so an issue is claimed before anyone works on it. A claim is the branch `claim/issue-<n>` on origin, holding one empty commit that names the agent and host. Git accepts only the first push of a new branch, so exactly one claimant wins. `scripts/issue.sh` does the whole cycle (bash; on Windows it runs under Git Bash):

```bash
scripts/issue.sh list                  # who holds what, on every machine, and for how long
scripts/issue.sh claim <n>             # claim and make the worktree ../Flint-<n>; exit 2: taken, pick another
scripts/issue.sh claim <n> --here      # claim for the worktree you're already in (a desktop-app worktree)
scripts/issue.sh claim <n> --base <b>  # branch from, and land on, the feature branch <b>; adds to either form above
scripts/issue.sh renew <n>             # at the start of each session on long work
scripts/issue.sh land <n>              # verify, push the branch and open a PR to main; the merge closes the issue and that releases the claim
scripts/issue.sh release <n>           # giving up: push your branch and comment where it stands first
scripts/issue.sh reap [--dry-run]      # delete the claims whose issue is closed; the SessionStart hook runs it
```

A claim ends when its issue closes: `.github/workflows/release-claim.yml` reaps it within a minute of the merge (or of any close), whichever machine held it, so nobody runs `release` after a merge.

- **A hook enforces it.** `.claude/settings.json` runs `scripts/issue-guard.sh` before every `git commit` and `git push`. It refuses issue work (a branch named `issue-<n>`, or a commit saying `Closes #<n>`) unless this session holds the claim, from the worktree it was claimed for. A chore with no issue (a typo, a dependency bump) needs no claim: keep it off `issue-<n>` branches and out of `Closes` lines. A branch built on a sibling's carries the sibling's `Closes` line; once that issue is closed (the sibling merged), the push passes without its claim. A SessionStart hook reaps the claims on closed issues, then lists the rest.
- **Only the session that claimed an issue renews, releases or lands it,** and only from its worktree. An existing worktree for an issue you didn't claim is another session's: leave it alone. Never delete another agent's `claim/*` branch, with one exception: a claim on a closed issue guards nothing, and `scripts/issue.sh reap` is the one way to clear it.
- **Work on a feature branch is claimed with `--base <branch>`,** never by changing `scripts/issue.conf`, which would ride into the default branch with the merge. The claim commit records the base, and `renew`, `--steal`, `land` and the hook all follow it. GitHub closes issues only for merges to the default branch, so merging the PR into the claim's base closes the issue, with a comment naming the PR and the branch, and releases the claim. Cut the feature branch from a default branch that already holds the release-claim workflow.
- **Claims go stale after 24 hours.** `claim <n> --steal` takes over a stale one; check the issue's comments and `origin/issue-<n>` for work in progress first, and continue from it. Taking a younger claim is Chris's call only: `claim <n> --steal --override "<his reason>"`.
- Settings for this repo are in `scripts/issue.conf`. The tool is vendored from `skills/setup-claims` in chrischaps/Dashboard: change it there, not here. `bash scripts/issue-test.sh` tests it against a throwaway remote.

<!-- claims:end -->
