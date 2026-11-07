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
   #   --debug-mode wireframe|normals|depth|uv|unlit|metalrough|style
   #                          (style = the entity style mask, #263: needs post-processing;
   #                          prints and draws a legend of each id's hue, #309)
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
   #   --oren-nayar 0.7          --sheen-strength 0.15  --sheen-color 1,0.9,0.8
   #   --grade-lift 0.03,0.02,0.015  --grade-gamma 1,1,1  --grade-gain 1.04,1,0.94
   #   --film-grain 0.03         --grain-time 0  --fxaa
   #   --particle-time 2.0    (step emitters/effects deterministically before
   #                           capture; default none = no particles. ADR 0068)
   #   --msaa 4               (1|4; default 1 keeps pixel gates single-sample.
   #                           Also on flint-player. ADR 0058)
   #   --render-scale 0.5     (scene at a fraction of the image, as the player's)
   #   --gpu-time 240         (draw N frames with GPU timing, print ms per pass)
   #   --ascii                (full-screen ASCII style, #265 = [post_process.styles.ascii])
   #   --ascii-cell 8 --ascii-font builtin --ascii-ramp classic|blocks|minimal|detailed
   #   --ascii-color source|tint|raw|mono|palette --ascii-tint 0.35,1,0.45 --ascii-background 0.15
   #   --ascii-palette cga|cga-warm|cga16|phosphor|amber|mono|<palettes/name.palette.toml> (#276)
   #   --ascii-ground dim|black|inverse  --ascii-glow 0.6
   #   --ascii-scope screen|entities --ascii-grid screen|object
   #                          (#266; a scene's ascii_style entities need no flag)
   #   --ascii-brightness 0 --ascii-contrast 1 --ascii-ink 1 --ascii-dither 0 (#277)
   #   --ascii-edges 1 --ascii-edge-threshold 0.5 --ascii-edge-max 3
   #                          (#267 line glyphs on depth/crease/mask edges; capture with --no-grid,
   #                           the editor grid writes depth and gets traced)
   #   --pixels-per-point 2   (high-DPI scale factor; ASCII cells are in points)
   #   --style ascii:cell=12,ramp=blocks   (#271: any registered style, keys checked
   #                          against its descriptor; repeatable; `--style help`;
   #                          also on `flint edit <model|effect> --render`)
   #   --style pencil:spacing=5,direction=screen   (#273: hatching, contours, paper;
   #                          direction surface|screen|tensor, seed, boil; demo/pencil_sketch)
   #   --style-time 1.5       (post time animated styles read; default 0)
   #   --edge-depth 0.6 --edge-normal 2.5   (#301: the shared geometry edges' thresholds;
   #                          `--style edges_view` shows them)
   #   --list-styles [--json] (#303: every registered style, its stage, table, and each
   #                          param's default/range/help; with a scene, its user styles too)
   #   --look ascii_noir      (#279: a styles/<name>.look.toml preset merged over the scene's
   #                          [post_process]; built-ins ascii_noir|matrix|ink_wash|graphite|watercolor;
   #                          `--look help` lists them with their tune sweeps)
   #   --sweep ascii.cell=6,8,12   (one labelled contact sheet, a column per value; keys:
   #                          <style>.<key>, post.<field> (`|` splits vectors), look=a,b;
   #                          cells 640x360 unless --width/--height. Cells are in points:
   #                          confirm a winner at the final size)
   #   --ascii-levels 4 --ascii-footprint 2 --ascii-no-edges --ascii-edge-depth 0.6
   #   --ascii-edge-normal 2.5   (tone bands, tone footprint, edge sensitivity)
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
   Scene viewer applies the scene's `[post_process]` block on load. **F4 opens the Rendering & Effects menu** (ADR 0053) — every render/post toggle plus its non-binary parameters (SSAO radius/samples, DoF, bloom, fog, grade/grain/FXAA, kuwahara, render modes, shadows + resolution, lighting levers, FOV, debug shading mode), an authored-vs-viewer-default post toggle, DoF-follow (focus plane tracks the last selected entity; values are plain view distances since the ADR 0055 depth fix), and a Particles section (the viewer simulates `particle_emitter` / `particle_effect` entities live; ADR 0068). WASD orbit, Q/E zoom — while an entity is selected, W/E/R switch gizmo mode instead of orbiting. It also seeds the orbit camera from the scene's authored `[camera]` block (Space returns to that framing; ADR 0051 in game repos using this pattern), and supports `--auto-orbit` turntable with O toggle and [ / ] speed like the model previewer.
   Model flags: `--clip <name>`, `--layer clip[:weight[:mask[:mode]]]` (repeatable), `--sequence <file.sequence.toml>`, `--sequence-loop`, `--anim-time <s>` (with `--render`; replays a sequence deterministically), `--anim-speed <f32>`, `--render <path.png>`, `--no-animate`, `--distance`, `--yaw`, `--pitch`, `--target`, `--fov`.
   A `*.sequence.toml` lists timestamped animator events (`blend`/`layer`/`speed`/`cue`); the previewer gets a Sequence panel (seek slider + event markers, R = restart), the player auto-loads them from `animations/` and scripts use `play_sequence` / `on_sequence_cue`. Layer weights can ramp via `fade_target`/`fade_duration` (`fade_anim_layer`).
   The model previewer has a Layers stack (clip/weight/Add-Over/mask/solo-mute per row) and a "Skeleton colour" combo that paints the armature by last writer / layer weight / mask / keyed joints.
   Scene flags: `--spline` (opens spline/track editor).
   Old commands (`serve`, `preview`, `gen-preview`, `tex-edit`, `terrain-edit`) still work as hidden aliases.

3. **`flint gen`** -- Run a procedural generation spec to produce meshes (GLB) or textures (PNG).
   ```bash
   flint gen specs/oak_tree.procgen.toml -o tree.glb
   flint gen specs/stone_wall.procgen.toml -o wall.png
   flint gen specs/oak_tree.procgen.toml --dry-run
   flint gen specs/oak_tree.procgen.toml --seed 42 -o tree.glb
   flint gen --ai-create "twisted dead tree" -o tree.glb     # agent writes the spec (ANTHROPIC_API_KEY;
                                                             #   --ai-provider mock is offline and free)
   flint gen specs/oak_tree.procgen.toml --ai-assist --max-cost 0.50 -o tree.glb
   flint gen specs/oak_tree.procgen.toml --ai-refine --reference ref.png -o tree.glb
   #   Claude sees a headless render of what the spec builds each round and the refine
   #   loops while the measured distance falls (--refine-rounds N; #183)
   flint gen --cost-report --since 2026-10-01                # sums .flint/procgen_costs.json
   # --ai-model, --ai-iterations, --ai-generator,
   # --print-spec (spec alone on stdout), --save-spec. Every API call goes through
   # flint_procgen_ai::CostMeter: priced, appended to the JSON Lines ledger, and
   # refused before sending if its worst case could pass --max-cost.
   ```

   **Goldens (#271):** `flint render-suite crates/flint-cli/tests/golden/stylize.goldens.toml`
   renders every stylize case and fails past its tolerance (`--bless` rewrites, `--case`,
   `--out-dir` gets renders + `<case>.diff.png` + `report.json`); `cargo test -p flint-cli
   --test render_goldens` runs it. A new style adds a case. A `.suite.toml` there is still music to WAV.

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
  ├── flint-debug-ui DebugPanel trait + F3/F4 panel roster (Rendering & Effects, ocean, sky, camera, grass, particles...); shared `widgets` (drag helpers, CurveEditor, GradientEditor); optional via debug-hud
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
- **All game UI is script-driven** -- no hardcoded HUD; `hud_controller` entity + `hud.rhai` pattern; engine provides draw primitives (`draw_text/rect/circle/line/sprite`), scripts compose UI
- **Game project pattern** -- games in own repos with engine as git subtree at `engine/`; `--schemas engine/schemas --schemas schemas` for layered overrides
- **Player extensions** (ADRs 0071-0075) -- a game binary embeds `PlayerApp` and installs `PlayerExtension`s (`add_extension`): `register_script_api` (native Rhai fns; same signature replaces Flint's), `pre_update(ExtensionCtx)` before scripts' `on_update` (world, clock, input, last frame's camera, `gpu` for `upload_mesh`/`remove_mesh`, `attach_mesh`, `fire_event` -> scripts' `on_event` same frame), `on_window_event` (only input events consumable) and `ui(egui)` drawn between script UI and the Foreground debug panels. `fonts(&mut FontDefinitions)` edits egui's fonts before each install (ADR 0081); `fonts/fonts.toml` `[egui] proportional/monospace` names egui's default faces, and every named family falls back to egui's fonts. With any extension installed, egui sees input first; a release follows its press. Runtime meshes are frustum-culled per draw by their AABBs (every game). Picking: `Camera::screen_to_ground` / Rhai `screen_to_ground`. Book: guides/player-extensions.md
- **Every runtime read goes through `flint_core::vfs`** (#97) -- `vfs::read` / `read_to_string` / `read_dir` / `exists` / `is_file` / `is_dir`, never `std::fs` or `Path::exists` in a runtime crate. The default source is the disk; the web player installs a `MemorySource` filled from a game bundle (`vfs::install`; `flint bundle <game>` packs one, `flint_core::bundle` is the format, `flint-player --bundle` plays one natively, audio arrives in a late bundle and `vfs::generation()` tells the player to load it; docs/design/web.md), and tests use `vfs::scoped` (this thread only). Decode images with `flint_render::open_image` (not `image::open`) and glTF through `flint_import::import_gltf` (not `gltf::import`), both of which read via the vfs. Writes (scene save, saves, replays) and hot-reload mtime polling stay on `std::fs`, natively; input overrides go through `flint_player::user_files` (OS config dir, or `localStorage` on the web, #100), and on the web so does the `persist_*` store (`save_<game_id>.toml`, #104). Tests and the CLI keep `std::fs`
- **Fixed-timestep physics** (1/60s) via accumulator; animation/particles run at variable rate
- **Physics joints** (ADR 0069) -- `joint` component (hinge/prismatic/spherical/fixed) on a dynamic entity, attached to `joint.parent` or the transform parent. Bodies are placed from `get_world_matrix` and dynamic poses write back as local `position` + `rotation_quat`; unit scale is assumed on the physics chain. TOML units are degrees/metres, converted to Rapier radians at sync. Scripts drive `motor_target` via `set_joint_target` and pose rest-quaternion nodes with `rotate_local` / `set_rotation_quat` (`set_rotation` still deletes the quaternion). Bench: `demo/joint_test.scene.toml`
- **Two-bone IK** (ADR 0070) -- `ik_two_bone` component on the second bone of a transform chain names `target`, `pole` and `tip` entities; `IkPass` (flint-animation) solves after scripts/physics/animation each frame and before the renderer reads world matrices, writing only the two bones' `rotation_quat` from their captured rest. Lengths come from the model's rest offsets. `flint render` runs it once
- **Glyph atlas** (#264) -- `flint_render::glyph_atlas`: `GlyphSet` = a coverage atlas (`R8Unorm` via `GpuGlyphAtlas`) + named luminance ramps + edge glyphs, from `fonts/<name>.glyphs.toml` (TTF via ab_glyph, PNG grid, or the hand-drawn built-in 8x8 set in `builtin_8x8.txt`); `GlyphSetCache::get("builtin"|"<name>")`; it and `PaletteCache` hot-reload natively (#323: `poll()` stats the files an entry read or searched, `crate::file_watch::FileStamps`; the ASCII style polls every 15 frames). Top-level keys like `default_ramp` go before the first `[table]`. Look at one with `cargo run -p flint-render --example glyph_atlas -- out/ [set]`. Book: concepts/glyph-atlas.md
- **Stylize passes** (#262, ADR 0111) -- `flint_render::stylize`: a style is a module exporting `STYLE: StyleDescriptor` (name, `StyleStage::Hdr` before fog/bloom/tonemap at scene resolution or `Display` after the composite at output resolution, params with ranges, `create`) plus its name in `register_styles!`; its `StylePass::encode` reads a `StyleFrame` (params, depth, camera, `target()` scratch textures, kept bind groups). Styles of a stage chain in registration order (or a table's `order = <n>`, lower first, ties by registration; `stylize::chain_order` / `stage_chain`, F4's chain line with swap buttons, #299) through framework ping-pong targets that live in `PostProcessResources` (rebuilt on resize). Scenes: `[post_process.styles.<name>]` (Kuwahara keeps its `kuwahara_*` fields; numbers are `params`, strings are `options`, e.g. a font or ramp); F4 → Styles (and the player's F3 Styles panel, made when a scene uses a style) is one `flint_debug_ui::StylesEditor` drawn from descriptors (#268): optional `hints: &[StyleHint]` give labels/help/`.integer()`, `<base>_r/_g/_b` params fold into one colour picker (display values) and are one colour setting (`StyleColor`, #315: `tint = [r, g, b]` or `"#rrggbb"` in a table, `tint=#rrggbb` or `r/g/b` on `--style`, `frame.color("tint")` / `variant_color`, Copy TOML writes the array), and each style gets Defaults / Authored / Copy TOML (`stylize::style_toml`); `ascii` (#265) is the display-stage glyph style, cells in points times `SceneRenderer::pixels_per_point` (hosts set it from the window), fonts found via `scene_dir`; its edges (#267) are two more passes (`ascii_edges.wgsl`: edge pixels at `StyleFrame::depth_size`; `ascii_edge_cells.wgsl`: one texel a glyph cell summing structure tensors) that the glyph pass draws as line glyphs, and a mask boundary where either side is one of the style's ids (plain or per-entity variant) is always an outline; `encode_to` builds newly enabled styles itself. `invert` is the template. Shared edges (#301): a descriptor's `needs: &[StyleNeed::Edges]` gets `StyleFrame::edges` (`stylize/edges.rs`: `Rgba16Float` at the depth's size, r = silhouette / g = crease strength over the shared `[post_process] edge_depth`/`edge_normal` (`--edge-depth`/`--edge-normal`), ba = unit direction, y down; `edges_bind` + `kit::texture_layout`), drawn once a frame before the Hdr stage however many styles read it (`SceneRenderer::style_edge_runs`); `edges_view` shows it, user styles take `inputs = ["edges"]` (`flint_edges`/`flint_edge`). ASCII and pencil still run their own edge passes. A style with frame-to-frame state (ASCII's per-cell history, #277: a cell pass into a ping-ponged `Rgba16Uint` texture via `target_sized`, then the glyph pass) trusts it only when `StyleFrame::frame` follows its last frame and `history_epoch` is unchanged; hosts call `SceneRenderer::reset_style_history()` on a scene change or camera cut. Book: concepts/stylize-passes.md (reference) and guides/stylized-rendering.md (choosing whole-frame vs per-entity, with the decision's captures). The decision: per-entity *selection* on one screen grid, with full screen its "everything" case and `grid = "object"` opt-in only; glyph-like styles at the Display stage (`docs/adr/0111-stylized-rendering.md`)
- **Entity style mask** (#263) -- `flint_render::style_mask`: a `style_mask { style, enabled }` component (descendants inherit; nearest wins) gives each styled entity an id (`StyleIds`, 1.. in name order). After the scene pass (and MSAA depth resolve) its opaque static/box/skinned draws, its terrain chunks (the host names the terrain entity with `set_terrain_entity`, #310) and then its styled transparents (a styled transparent is a lens: its id replaces what is behind it, #310) are redrawn with their cached transform (+ bone) bind groups into an `Rg16Uint` target in `StyleTargets` (id in `.r`, blend in `.g`, #278), depth-tested in the shader against the single-sample depth with a slope tolerance; ids ride one dynamic-offset uniform buffer. Drawn only while a `StyleDescriptor { uses_mask: true }` style is on; styles read `StyleFrame::mask` / `mask_bind` / `style_id`. `--debug-mode style` (`DebugMode::StyleMask`) turns on the `mask_view` display style. Demo: `demo/style_mask.scene.toml`. Veil (#290): just before the mask, unstyled transparents and all particles draw their coverage into an `R8Unorm` "style veil" (read-only scene depth attachment, `fs_veil` entry points in `shader.wgsl`/`skinned_shader.wgsl`/`particle_shader.wgsl`), and the mask writes blend x (1 - coverage): what's in front unmasks by its coverage. A new transparent or particle draw path needs a veil draw too. Demo: `demo/style_veil.scene.toml`
- **Per-entity styles** (#266) -- a descriptor's `component: Some("ascii_style")` makes a style per-entity: an entity with that component (or a `style_mask` naming the style) switches it on for its own pixels with no `[post_process.styles]` table (`is_on(config, &StyleIds)`; `ensure`/`active`/`any_uses_mask` take the ids). Its fields are the descriptor's params/options, falling back on the scene's table then defaults (`StyleVariant::param`; a colour `tint = [r, g, b]` or `"#rrggbb"` fills `tint_r/g/b`, #315); each distinct set is a `StyleVariant` with its own id, named for its settings (`ascii{ramp=blocks}`, `ascii@barrel` for `grid = "object"`, `ascii{off}` for `enabled = false`). Styles read `frame.variants()` / `variant_param` / `whole_frame()` / `project`. The schema gives `ascii_style` fields no defaults on purpose (the loader fills defaults in). ASCII: `scope = "screen"|"entities"`, `grid = "screen"|"object"`, `--ascii-scope`/`--ascii-grid`. Demo: `demo/ascii_entities.scene.toml`. Thin features (#288): `outline` (points) and `min_coverage` are ink floors `variant_pixel` passes to `shade`, from the mask edge and the variant's share of the cell's samples; per-entity rows only, slot 0 untouched (`demo/ascii_silhouette.scene.toml`). Scripts (#269): `set_entity_style` / `set_style_param` / `clear_entity_style` queue `ScriptCommand`s that `flint_player::apply_style_command` turns into `flint_render::entity_style` component edits (set/clear fire `style_changed`); a per-frame param is a variant-table row in the existing uniform, no GPU allocation (`SceneRenderer::kept_post_binds` + hub report test in `tests/stylize.rs`). Demo: `demo/ascii_story.scene.toml`. Blend (#278): `blend` 0..1 on the style component (or `style_mask`) is the root's, inherited, and rides in the mask's `.g` (`resolve_blended`), never in the variant name, so a ramp keeps one id; ASCII mixes per pixel (`transition = "fade"`) or turns whole cells by a fixed patch-noise order (`"dissolve"`). A new mask-reading style should honour `.g`. Demo: `demo/ascii_reveal.scene.toml`; book: stylize-passes.md "Blending in and out" (and how it differs from `--render-mode` tears)
- **Pencil sketch** (#273) -- `stylize/pencil.rs`, a display style in two passes: a prep pass writes tone, contour (depth steps on the near side, creases from depth-rebuilt normals, mask edges, luma edges by `detail`) and a double-angle stroke direction into `pencil.prep`; the draw pass blurs the direction, hatches up to 4 layers in 15° bins (surface = across grad(1/z), so planes hatch straight), wobbles/doubles contours and lays paper grain. All randomness is integer hashes of row/dash/layer/`seed` (byte-identical runs); `boil` steps the seed by post time. Per-entity via `pencil_style` (same variant table as ASCII, `grid = "object"` anchors the hatching); a mask id another registered style owns gets the `FOREIGN` slot and passes through. The `graphite` look uses it
- **User-authored styles** (#274, #275, ADR 0112; experimental, format v1) -- `styles/<name>.shader.toml` (`version = 1`, `name` = stem, `stage`, `shader`, `inputs = ["depth","mask"]`, `[params.<k>] default/min/max/label/help/integer`) + a WGSL `fn style(p: StylePixel) -> vec4<f32>`; `stylize::custom` composes a generated prelude (`flint.*`, `params.*`, `flint_sample/flint_view_depth/flint_world_pos/flint_style_blend`...) and entry points, checks it with naga (native) into `file:line:col` errors, draws a passthrough on failure, polls the WGSL's mtime to rebuild (last good kept). Hosts call `stylize::custom::load_dir(scene_dir)` BEFORE reading `[post_process]` (`flint render`, the player's `scene_loading::load_user_styles` on load and transitions, and the viewer do); the registry is an append-only snapshot of leaked descriptors, so `registry()` is no longer a compile-time slice. `.shader.toml` hot reload (#275): hosts call `custom::reload_changed(scene_dir)` every ~15 frames; a changed description replaces its descriptor at the same index (`replace_runtime`; `StyleSet` rebuilds a slot whose descriptor address changed) and the host re-reads the scene's tables for it (`refresh_settings`). A user style's bind-cache names carry a per-install generation: keep them unique, or a rebuilt pass binds the old pass's buffers. Errors: `custom::errors()`, drawn by `flint_debug_ui::paint_style_errors` (player + viewer) and tagged in F4; `flint bundle` loads each scene's user styles from the packed files and refuses broken ones. Template: `demo/styles/template.*`. Not `*.style.toml`: that suffix is AI style guides and UI class files. `--style`/`--sweep` let unknown names through clap and `StyleArgs::check` validates after loading. Demos: `demo/styles/`, `demo/user_styles.scene.toml`
- **Post-processing** -- HDR pipeline (`Rgba16Float`) with bloom, SSAO, fog, volumetric (god rays), Kuwahara (anisotropic painterly filter), vignette; `[post_process]` TOML block; F4 opens the Rendering & Effects debug menu in the player and viewer (all toggles + parameters, ADR 0053); when active, PBR shaders output linear HDR
- **Quality settings** (FlintKart#55) -- `flint_core::quality::QualitySettings` (render_scale, render_scale_auto, msaa, shadows, shadow_resolution, shadow_cascades, shadow_distance, and post caps post_process/bloom/ssao/ssao_samples/fog/volumetric) reaches `SceneRenderer::set_quality` from `--quality k=v,...` (flint-player and `flint render`), the page's `?quality=`, Android's `dev_profile.toml` `[quality]`, and scripts' `set_quality(map)` / `get_quality()` / `gpu_info()`, applied between frames (`PlayerApp::apply_quality`). Post keys are caps over the authored config: the renderer keeps `post_process_config()` (authored) apart from `effective_post_process_config()` (drawn), so a scene load keeps the tier. An MSAA change rebuilds every scene pipeline against equal bind group layouts (wgpu dedupes them), so cached bind groups stay valid; a new scene pipeline must be added to `set_msaa`. Shadow cascades map split depths through the camera's own near/far (they assumed 0.1/200 before, stretching the last cascade to the 1000 m far plane). Book: concepts/quality-settings.md
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
- Offline capture: `flint-player --frame-step S` advances the clock exactly S seconds per frame (wall time ignored), and with `--screenshot-every S` every frame is written; `--input-script FILE` plays timed `down`/`up`/`tap` keys, `axis` values and gamepad `press`/`release` (on pad slot 9) at game times (format in `player_app/input_script.rs`); a scripted key also goes through the debug keys (`handle_debug_key`), so `tap F2` opens the stats overlay, and `--stats` opens it from the first frame. Together they make deterministic, smooth gameplay footage and scripted regression runs
- Perf switches: `flint-player --exit-after N` prints `[perf] avg ms/frame`, the spread of dt and of present-to-present times (p50/p90/p99/max and frames over 1.5x p50), and the average draw calls, entity draws, culled draws and triangles on exit; `FLINT_VSYNC=0|mailbox` picks the present mode (Fifo can pace at a virtual display's rate, e.g. 7 Hz with Virtual Desktop attached); `FLINT_FRAME_LATENCY=1..3` sets the swapchain's frame latency (default 2); `FLINT_PACING=0` turns off vsync pacing; `FLINT_SHADOWS=0` disables the shadow pass at start; `FLINT_LOD_SCALE` / `FLINT_DRAW_DISTANCE_SCALE` start the distance levers (#56: glTF `extras.flint_lod` bands per primitive, `lod.draw_distance` per entity, Rhai `set_lod_scale` / `set_draw_distance_scale`; `FLINT_LOD_SCALE=1000` holds every model at full detail; book: Rendering); `--render-scale 0.25..1|auto` draws the 3D scene at that fraction of the window on each side (HDR chain + its own depth, upscaled by the composite, depth copied back so world text/lines still test against it) under a full-resolution HUD, `auto` stepping down to hold 60 fps; the web caps the scene at DPR 1.5 by default and takes `?scale=` (#122, `SceneRenderer::set_render_scale`); `--perf` prints a line every 10 s with each frame phase's CPU time, the worst scripts and each pass's GPU time, and the web page's `?perf=1` (`?perf=cpu` without GPU timing) shows the same in a corner with a Copy line button (#120, docs/design/web.md)
- Vsync pacing: under Fifo the player gives `GameClock` the monitor's refresh period (`set_vsync_period`, re-read when the window moves), and `delta_time` comes in whole refresh periods that keep game time on the wall clock. On a hybrid-GPU laptop Fifo lets frames through in bursts (6, 6, 6, then 45 ms at a 60 Hz mean); paced, every frame steps 16.7 ms. Stalls over 250 ms pass through, and frames faster than the refresh on average fall back to raw time. `raw_delta_time` is the unpaced wall time. See the book's Profiling guide
- CI's `wasm32 check` (`.github/workflows/ci.yml`) builds `flint-player` and every crate it links for `wasm32-unknown-unknown` on each PR. Run it locally with `bash scripts/wasm-check.sh` before pushing runtime-crate changes. It also denies `Instant::now`/`SystemTime::now` and their `elapsed()`, which compile on wasm but panic there: runtime code uses the `web-time` crate (a workspace dep: `web-time = { workspace = true }`), which #100 moved every older site to. Desktop-only deps in a runtime crate go under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`, as in flint-player's Cargo.toml (docs/design/web.md)
- Web frame loop (#121): `flint_web.rs` runs winit with `ControlFlow::Wait` and `about_to_wait` requests a redraw, so the player draws once per `requestAnimationFrame` and the main thread sleeps between frames. Never `ControlFlow::Poll` on the web: it chains `scheduler.postTask` tasks back to back, Chrome shrugs but Firefox starved its refresh driver (83 ms frames, multi-second stalls, half the audio buffers late). `?loop=poll` brings the old loop back for comparisons
- Web player (#102): draw the swapchain through `RenderContext::frame_view` (sRGB view of a plain WebGPU canvas), never a default view. Sound and pointer lock need a gesture: the page's Begin calls `flint_player::web_begin()`, and on wasm `set_cursor_captured(true)` is a wish that `sync_pointer_lock` turns into a lock while the page has activation; mouse look flows only while the lock is held. Headless Chrome never grants pointer lock, and headed Chrome only with the window focused (`Page.bringToFront`). Touch (#132): winit's web backend sends `pointerType == "touch"` as `WindowEvent::Touch` scaled by `devicePixelRatio`, and `touch_to_surface` rescales to canvas pixels (they differ under Chrome's device emulation on a scaled desktop). A `(pointer: coarse)` page asks for no pointer lock. Test with CDP `Emulation.setTouchEmulationEnabled` + `Input.dispatchTouchEvent`
- Web audio (#136): the browser mixes in an `AudioWorklet`, on the audio thread, running a second wasm (`crates/flint-audio-worklet`: kira only, no imports, `[profile.worklet]` opt-level 3, built by Trunk.toml's `pre_build` hook and copied beside the page). The pages share no memory with it (no COOP/COEP on Pages), so `AudioEngine` calls become `flint_audio::remote` commands posted once a frame and each sound crosses once as samples; handles are `engine::SoundHandle` / `SpatialTrack`. A new engine call that plays or changes sound needs a `protocol::Command` too. `?mixer=main` (or a scene with a `music_session`, which needs `manager_mut`) keeps the old main-thread cpal stream, which is also the fallback when the worklet fails
- play.chaps.dev (#103): `.github/workflows/play.yml` builds the player with trunk, packs every game in `tools/play-site/games.toml` (each pinned to a commit) with `flint bundle`, and deploys this repo's Pages from Actions. `?game=<id>` plays `games/<id>/<id>.flintpack`; `?embed=1` is the iframe view. `index.html`'s doorway shows the chooser, or the game's film when WebGPU is missing, before any wasm loads: `Trunk.toml`'s loader awaits `window.flintDoorway` and preloads only the JS glue. Keep the two together, or a browser that can't play downloads 7 MB of wasm (docs/design/web.md)
- Web size (#123): the site builds with `trunk build --release --cargo-profile web-release` (workspace `[profile.web-release]`: opt-level z, fat LTO, 1 codegen unit, panic = abort, with `opt-level = 3` package overrides for the hot crates and `+simd128` in `.cargo/config.toml` (#137); 3.6 MB gzipped, down from 5.1). Speed-build a crate only when the phone shows it hot: each override costs bytes, and the 4 MB gzip line holds. The workspace `image` dep is PNG/JPEG/HDR only and `flint-render` restores image's defaults natively, so add a format for desktop there, not in the workspace. rhai's `no_optimize`/`no_custom_syntax`/`only_i64` are on for wasm only (flint-script's Cargo.toml): web scripts see `i64` integers and nothing else. Measure a size change with trunk and record it in web.md's Size table
- `FlintWorld::spawn` ids come from a process-wide counter and scenes load from a `HashMap`, so entity ids differ run to run -- derive anything reproducible (seeds) from entity **names**, never ids

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
- `docs/adr/` -- Architecture decision records from ADR 0111 on (earlier numbers live only in commit messages); `README.md` says how to take a number

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
