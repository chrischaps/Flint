# flint-cli: `flint render`, `flint edit`, `flint gen`, goldens, export (moved out of the root CLAUDE.md by #755)

The root file keeps the short edit -> validate -> play loop; this is the rest of it. The styles the flags switch on: `crates/flint-render/src/stylize/CLAUDE.md`. `flint play` and `flint-player` flags: `crates/flint-player/CLAUDE.md`. `flint bundle` and the vfs: `crates/flint-core/CLAUDE.md`.

## `flint render` flags

   ```bash
   # Debug/post-processing flags:
   #   --debug-mode wireframe|normals|depth|uv|unlit|metalrough|style|edges
   #                          (style = the entity style mask, #263: needs post-processing;
   #                          prints and draws a legend of each id's hue, #309; runs mask_view
   #                          alone, the scene's other styles suspended, #334)
   #                          (edges = ASCII's edge passes, #349: pixels by source, cells by
   #                          decision + length bar; ascii alone, on at defaults if absent;
   #                          ascii_edge_view.wgsl; StyleFrame::debug_mode carries the mode)
   #   --wireframe-overlay    --show-normals    --no-tonemapping
   #   --select <entity>      (#602: the editor's selection outline round it and its subtree,
   #                          drawn over the finished image; flint_render::selection_outline)
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
   #     (#599: shorthands for --style contour:width=1.25,... ; contour is a registered HDR style)
   #   --kuwahara-boil 0.5  --contour-boil 1  --boil-on 2  --boil-time 0.5
   #     (the boils are kuwahara:boil / contour:boil; boil_on stays a [post_process] key)
   #     (shading boil, #214: boil needs the scene's [wooze]; --boil-time picks the drawing)
   #   --oren-nayar 0.7          --sheen-strength 0.15  --sheen-color 1,0.9,0.8
   #   --grade-lift 0.03,0.02,0.015  --grade-gamma 1,1,1  --grade-gain 1.04,1,0.94
   #   --film-grain 0.03         --grain-time 0  --fxaa
   #   --particle-time 2.0    (step emitters/effects deterministically before
   #                           capture; default none = no particles. ADR 0068)
   #   --msaa 4               (1|4; default 1 keeps pixel gates single-sample.
   #                           Also on flint-player. ADR 0058)
   #   --render-scale 0.5     (scene at a fraction of the image, as the player's)
   #   --gpu-time 240         (draw N frames with GPU timing, print ms per pass)
   #   --mesh tree_a=out/oaks [--mesh tree_b='out/w*.glb'] [--sheet | -o x.gif --fps 4 | -o f-%03d.png]
   #                          (#222: a frame per GLB on that entity, one renderer; natural order;
   #                           multi-node GLBs baked flat, mesh_seq.rs; examples/mesh_sequence.scene.toml)
   #   --frames 181 --frame-dt 0.0166667 --orbit 6.8755 | --camera-path cam.toml
   #                          (#345: N frames on one renderer as out_0000.png...; temporal
   #                           styles see consecutive frames; --flip-stats / --flip-json f.json
   #                           count ASCII glyph flips and A-B-A reversals from its own grid,
   #                           --no-images skips the PNGs; byte-identical run to run)
   #   --ascii                (full-screen ASCII style, #265 = [post_process.styles.ascii])
   #   --ascii-cell 8 --ascii-font builtin --ascii-ramp classic|blocks|minimal|detailed|quadrants|braille
   #   --ascii-color source|tint|raw|mono|palette --ascii-tint 0.35,1,0.45 --ascii-background 0.15
   #   --ascii-palette cga|cga-warm|cga16|phosphor|amber|mono|<palettes/name.palette.toml> (#276)
   #                          (nearest inks match in OKLab; --style ascii:chroma=2,palette_dither=1
   #                           pushes saturation and Bayer-mixes the two nearest inks, #320)
   #   --ascii-ground dim|black|inverse  --ascii-glow 0.6
   #   --ascii-text frame.txt  --ascii-text-ansi frame.ans
   #                          (#317: the glyph grid as text, a line per cell row, cell for
   #                           cell with the PNG; read a render instead of looking at it)
   #   --ascii-scope screen|entities --ascii-grid screen|object
   #                          (#266; a scene's ascii_style entities need no flag)
   #   --ascii-brightness 0 --ascii-contrast 1 --ascii-ink 1 --ascii-dither 0 (#277)
   #   --ascii-edges 1 --ascii-edge-threshold 0.5 --ascii-edge-max 3
   #                          (#267 line glyphs on depth/crease/mask edges; the editor grid is a
   #                           depth-free overlay since #346, never traced, but --no-grid keeps tone clean)
   #                          (#348 corners: a drawn cell whose tensor coherence is under
   #                           ascii:edge_corner=0.9 takes ┌ ├ ┼... from its neighbours' ─ │
   #                           (ARM_GLYPHS, ascii_edge_thin.wgsl); a set's [edges] corners, else lines;
   #                           #637: an unmarked ─/│ that ends beside a perpendicular one turns too,
   #                           and a sub-threshold gap cell between a ─ and a │ ending at it draws
   #                           their corner; staircases refused; edge_corner=0 turns all of it off)
   #                          (#351 their ink: --style ascii:edge_ink=cell|color|hue,
   #                           edge_color=#rrggbb,edge_tint=0..1; hue = the near side's own colour)
   #   --pixels-per-point 2   (high-DPI scale factor; ASCII cells are in points)
   #   --style ascii:cell=12,ramp=blocks   (#271: any registered style, keys checked
   #                          against its descriptor; repeatable; `--style help`;
   #                          also on `flint edit <model|effect> --render`)
   #   --style pencil:spacing=5,direction=screen   (#273: hatching, contours, paper;
   #                          direction surface|screen|tensor, seed, boil; examples/pencil_sketch)
   #   --style-time 1.5       (post time animated styles read; default 0)
   #                          (#328: --style/--style-time also on flint play, flint-player and
   #                           flint edit scene.toml, kept through transitions and hot reloads;
   #                           the web's ?style=ascii:cell=12, repeatable)
   #   --edge-depth 0.6 --edge-normal 2.5   (#301: the shared geometry edges' thresholds;
   #                          `--style edges_view` shows them)
   #   --list-styles [--json] (#303: every registered style, its stage, table, and each
   #                          param's default/range/help; with a scene, its user styles too)
   #   --look ascii_noir      (#279: a styles/<name>.look.toml preset merged over the scene's
   #                          [post_process]; built-ins ascii_noir|matrix|ink_wash|graphite|watercolor|
   #                          coloured_pencil|blueprint|charcoal (#391);
   #                          `--look help` lists them with their tune sweeps. #356: a scene's
   #                          `[post_process] look = "<name>"` goes UNDER its keys (loader records
   #                          them in PostProcessDef::authored; flint_render::look::resolve, vfs reads);
   #                          F4 -> Look and Rhai set_look/clear_look re-resolve; examples/looks.scene.toml)
   #   --sweep ascii.cell=6,8,12   (one labelled contact sheet, a column per value; keys:
   #                          <style>.<key>, post.<field> (`|` splits vectors), look=a,b,
   #                          wooze.<knob> (#216; one Sweep type in commands/sweep.rs,
   #                          one sheet in style_sheet.rs, #353; a render with clocks
   #                          -- --time, wooze.*, --scripts, --report json, a living or
   #                          [wooze] scene -- draws a Stage a cell, #487; it takes grids,
   #                          crops, --reference, --report FILE, --ascii-text, --gpu-time, --frames
   #                          and --ai-refine too, measures at one moment only, #596);
   #                          cells 640x360 unless --width/--height. Cells are in points:
   #                          --sweep-crop X,Y,W,H draws each at full size (1920x1080) and
   #                          keeps a 1:1 region, --sweep-scale full shrinks the whole frame;
   #                          two --sweep flags make a grid, first across, second down, #358)
   #   --variant "label: --flag v ..." (repeatable) | --variants v.toml   [--zoom X0,Y0,X1,Y1@N]
   #                          (#291: any flags side by side: each variant = this command line + its
   #                           flags (args_override_self; --style adds), one labelled sheet at -o,
   #                           <out>-<label>.png each, nearest crops <out>-zoom.png; one process and
   #                           GPU (RenderHost, imports memoized); commands/variants.rs; examples/variants/.
   #                           main.rs / render_goldens fill RenderArgs::command_line for it)
   #   --report m.json --reference ref.png   (#357: per column luma, near-black/white, edge
   #                          density, ASCII ramp-step shares, #183's distance; `look_measure.rs`)
   #   --look x --reference ref.png --ai-refine [--ai-provider anthropic|mock --max-cost 0.5
   #                          --refine-rounds 4 --refine-candidates 4 --refine-out f.look.toml]
   #                          (agent proposes sweep-key edits, each rendered + measured, best kept
   #                          while the distance falls; writes <look>_refined.look.toml + .json log
   #                          + rounds sheet; calls via CostMeter; mock walks the look's tune)
   #   --ascii-levels 4 --ascii-footprint 2 --ascii-no-edges --ascii-edge-depth 0.6
   #   --ascii-edge-normal 2.5   (tone bands, tone footprint, edge sensitivity)
   #   --time 3.5             (living props, wooze field and other clocks at that game time;
   #                           byte-identical, and the same pixels as flint-player --frame-step
   #                           at that time. No physics. --frame-step S names the step)
   #   --scripts              (run the scene's scripts headless, a tick a step in the player's frame order:
   #                           seeded random(), no timestamp(), name-ordered; what can't be drawn is
   #                           warned. flint_player::HeadlessScripts, #8; tune_living's boil, step_living,
   #                           load_living_preset and script particles too, #579: boil tunes queue and
   #                           land on the config at capture via living_tuning::tune_boil, the player's;
   #                           particles made at load step on the script clock, Stage::step_particles_to)
   #   --expression-timeline f.json   (expression() fed on the render's clock; implies --scripts)
   #   --time 0..4 --fps 12 -o x.gif   (a span as a GIF sharing one palette; a still pixel never flickers;
   #                           frames wait on disk as deflated deltas, x.gif.frames~, so memory stays flat, #588;
   #                           -o frames/f-%05d.png writes a PNG a frame instead; clocks run on to the span's end)
   #   --time 2,5,9.5 -o x.png   (#614: moments of one run, simulated once, a still each as
   #                           x_0000.png...; each byte-identical to --time <moment> alone on the step)
   #   --sweep wooze.glide=0,0.25,0.5,1  (labelled columns; also .step .amplitude .jitter .preset;
   #                           a preset is a wooze/<name>.wooze.toml first, species tables and all, #249)
   #   --report json          (stdout: each living prop's lineage seed, pose indices, wooze values,
   #                           and every species value with the layer that set it, #246)
   # Note: --shadow-resolution defaults to 2048 (the renderer's construction
   # default) and is a real control since the texel-size upload (ADR 0049).
   #   --render-mode 1 --mode-mix 1.0 --mode-params 3,0,6,0
   #     (modes: 1=matrix 2=blood 3=drunk 4=tron [reality tears; params =
   #      mask scale, mask style 0 fbm/1 iris, rate, spare] 5=underwater
   #      [params = signed eye depth m, sea energy 0-1, daylight 0-1,
   #      biolum 0-1; masks by per-pixel waterline, not the tear mask])
   ```

## `flint render` over time

- **`flint render` over time** (#216) -- `render.rs` loads a `Stage` per sweep column, each a world, renderer and clocks of its own. `flint_player::LivingRun` is the player's living-prop load and step with no window and no budget (`ProcGenResolver::step_living_now`), and only runs in a scene with `living_prop` entities, whose plain renders were empty before; every other scene's plain render stays byte-identical. To match the player it keeps the authored camera position (going through `update_orbit` rounds it, which changed 7,000 pixels at 1280 x 720) and draws no grid. `--screenshot-at` has a tolerance, since 210 steps of 1/60 sum to a hair under 3.5. Stdout is the JSON report's: flint-cli's tracing goes to stderr, and so do the load path's progress lines, so a new `println!` on that path breaks `--report json`. GIFs go through `gif_writer.rs`, one palette and transparent unchanged pixels, because `image`'s encoder shimmers still pixels. Not a new dep: `gif` and `color_quant` were in the tree through `image`

## `flint edit`

   ```bash
   flint edit levels/demo.scene.toml --spline      # Spline/track editor
   flint edit models/character.glb --watch         # Model previewer with file watching
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
   **Spec saves patch, never rewrite** (#194, `commands/spec_save.rs`): the procgen previewer (`spec_text_for_save`) and the texture node editor build the whole document as values, and `patch_document` edits only what differs into the file's `toml_edit` document, keeping comments, order and CRLF. Never write a spec with `toml::to_string_pretty`. The node editor's ops go through `pipeline_ops_for_save` first, which drops the defaults `from_toml` filled in and the `id`/`inputs` the graph added when the file had none. Tests: `spec_save::tests`, `gen_preview::tests`.
   Old commands (`serve`, `preview`, `gen-preview`, `tex-edit`, `terrain-edit`) still work as hidden aliases.

## `flint gen`

   ```bash
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

## Goldens and `flint render-suite`

   **Goldens (#271):** every case loads a frozen scene under `tests/fixtures/scenes/` (#763; its README),
   never `examples/` (test `no_golden_case_loads_an_example`): copy a scene there, with its assets, to pin it.
   `flint render-suite crates/flint-cli/tests/golden/stylize.goldens.toml`
   renders every stylize case and fails past its tolerance (`--bless` rewrites, `--case`,
   `--out-dir` gets renders + `<case>.diff.png` + `report.json` + an `index.html` contact
   sheet of golden | render | diff, failures first, #329; `--case` with an exact name picks that case
   alone, otherwise every name containing it, #388); a pass that isn't byte-identical with its golden
   prints `DRIFT`, is `"drift": true` in the report, and the run ends naming every drifted case
   or saying none; `--bless-drifted` re-blesses only those, for a catch-up commit (#387); `cargo test -p flint-cli
   --test render_goldens` runs it. Cases run in one process on one GPU device with glTF
   decodes and mip chains memoized (`render::RenderHost`, `flint_import::memoize_imports`,
   `flint_render::memoize_mip_chains`; ~2.5x in debug); a panicking case reruns in its own
   process, `--isolate` spawns every case. A new style adds a case. A case's `matches = [flags]` (#417) compares with a second render
   made with those flags instead of a golden, exact by default, and `error = "<text>"` needs a user
   style's error to hold it (`user-style-broken`: `tests/golden/fixtures/styles/broken.*` must draw a
   passthrough, which runs the prelude's sRGB round trip). A case's `text = "<x>.txt"`
   (#317) also pins its ASCII grid exactly (`text_cells` allows some; `<case>.text.diff`).
   CI runs the goldens and `flint-render --test stylize` on WARP (#314, job `GPU tests (WARP)`)
   with `FLINT_REQUIRE_GPU=1`, which turns a GPU test's skip into a failure
   (`gpu_select::skip_without_gpu`; a new GPU test calls it). flint-render's GPU test binaries
   share one device (`tests/shared_gpu`, #598: a device per test deadlocked NVIDIA's Vulkan
   driver against the loader lock as test threads exited, hanging a run for hours): `lease()`
   alongside other tests, `lease_alone()` to count device-wide objects or use error scopes
   (per device in wgpu 23); a watchdog aborts, naming the test, past `FLINT_GPU_TEST_TIMEOUT`
   (300 s). A new GPU test file uses it, not its own `request_device`. A golden (image or text) may have
   a per-adapter variant `<stem>.<warp|lavapipe|vulkan|dx12>.<ext>` (`--bless-variant`); wgpu
   hides WARP on a machine with a GPU, so WARP variants come from the job's `stylize-goldens-warp`
   artifact. Two traps it caught (DX12/FXC only): a `switch` miscompiled, and
   `fract(theta - base)` on a ±ε knife edge. A `.suite.toml` there is still music to WAV.

## `flint init`

- **`flint init <name> [--template walk|blank|2d] [--engine path|subtree|none] [--engine-path dir] [--force]`** (#707, `commands/init.rs`) writes a game project from the files under `crates/flint-cli/templates/`, compiled in with `include_str!` (the `SHARED` table and `Template::files`; a new file is a row there). `shared/` is every project's: `flint.toml`, `guide.md` + `agents.md` (written as `CLAUDE.md` and `AGENTS.md`; named otherwise here so no agent working in the engine loads a template as its instructions), `bundle.toml`, `config/input.toml`, `gitignore` (written as `.gitignore`), plus the per-mode Markdown sections (`engine-*.md`, `schemas-*.md`, `scripts-*.md`) the guide is assembled from. `walk/`, `blank/` and `2d/` hold each template's `scenes/main.scene.toml` and `scripts/`. `{{key}}` is filled by `fill` (keys: `name`, `id`, `schemas`, `engine`, `template`, `template_blurb`, `scripts_note`, `controls`, `bundle_engine`, and the three sections; a unit test fails on an unknown key). The game's `schemas/components` and `schemas/archetypes` are written empty (`.gitkeep`): **no engine schema is copied**, the manifest names the engine's.
- **`--engine`**: `path` (default) names `built_from()` (`CARGO_MANIFEST_DIR/../..`, as `flint bundle`'s `engine_schemas` does) or `--engine-path`, absolute with forward slashes, in `flint.toml`'s `schemas` and `bundle.toml`'s `[mount] "engine/schemas"`; it is refused unless the directory has `schemas/components/transform.toml`. `subtree` names `engine/schemas` and prints the `git init` ... `git subtree add` lines: init never runs git or touches the network, and until the subtree exists every command fails with `schema directory not found` (the output says so). `none` lists only `schemas`: validate and render work (a component is a table with or without a schema), archetypes resolve to nothing, so `flint play` is not printed. There is no way to get engine schemas into a project with neither a path nor an `engine/`; `none` does not pretend otherwise.
- **Never deletes or overwrites**: a directory with files is refused without `--force`; with it, a file already there is kept and listed as kept.
- **The printed "Next steps" and the generated `CLAUDE.md` are tested as written** (`tests/init_cli.rs`): every printed `flint` line is run in a fresh project of each template (render: luma mean and spread, so a black or flat frame fails; then `--scripts --time 0.5` with no script error), every inline-code path in the guide must exist in the project (or, for `engine/...` in a subtree project, in this repo), every `flint <word>` must take `--help`, and every `flint` line in a code block is run. So a path or command added to a template must exist, and prose there says "Flint", not "flint", unless it names a command. `flint play` opens a window: the test resolves its default scene and the window is checked by hand (`flint-player --exit-after 2 --no-activate --screenshot x.png` in a fresh project draws the HUD too, which a headless render does not).
- A scene edit in a template needs a look: `flint init x --template <t>` somewhere scratch, then the printed render. `testGame/` (a committed copy of the old scaffold) is gone; the test generates its projects.

## `flint examples`

- `flint examples [--topic t] [--json] [--check] [--dir d] [--schemas s]` (#763, `commands/examples.rs`): lists `examples/index.toml` (`[[example]] scene/topic/purpose`, unknown keys refused) grouped by topic in index order; `--dir` defaults to `examples/`, else `engine/examples/`. `--check` fails (exit 1, a `FAIL` line each) on a scene on disk with no entry (`dist/` skipped), an entry with no file, an empty topic or purpose, a duplicate, or a scene failing `validate::check` -- everything `flint validate` checks, but the scene is loaded even with no constraints, and style warnings count as failures. CI's headless job runs it. Test: `tests/examples_cli.rs`

## `flint export web`

- `flint export web <game> [--out dir]` (#106, `commands/export.rs`): copies a trunk build of the player (`--player`, or builds one from the CLI's own tree so packer and player share `bundle::FORMAT_VERSION`), packs `games/<id>/` with the game's `bundle.toml` rules, copies a local `video`/`poster`, and merges this game's entry into `games.json` (id kept in place); one listed game plays at the bare address (the doorway sets `?game=`). Writes only into an empty folder or a past export. `bundle.toml` gained `[web]` (the games.json keys) and `[mount]` (`"engine/schemas" = "../schemas"`, as `examples/bundle.toml` does); a game with no `engine/schemas` packs the engine's own

## `--schemas` and the project manifest

- **One schema resolver** (ADR 0114 step 2, #706) -- every command that loads schemas calls `commands::common_args::load_schemas(scene, &flags)` (`flint_schema::SchemaRegistry::resolve` -> `flint_core::project::schema_dirs`): `--schemas` flags if any (the whole list; nothing discovered is added), else `[project] schemas` in the scene's project's `flint.toml`, else `schemas/` in the working directory, beside the scene and one level up. `--schemas` is `Vec<String>` with NO clap default on every command (`play`, `render`, `validate`, `query`, `entity *`, `schema`, `edit`, `serve`, `spline-edit`, `prefab view`), so an empty vec means "not given"; a default would read as a flag and hide the manifest. A named directory that isn't there is an error naming it and who named it (`schema directory not found: nope (named by --schemas)`), printed bare, on stderr. A game repo with `schemas = ["engine/schemas", "schemas"]` needs no flags; in the engine checkout a `examples/` scene resolves by discovery to `./schemas`, as it always did. `flint edit <scene>` passes every flag to `flint_viewer::app::load_schemas` (#704 was `.first()` in `edit_router.rs`; its test parses the real command line on a 16 MB thread). `flint bundle` takes the manifest's `schemas` when `bundle.toml` names none. Test: `tests/project_schemas_cli.rs` (a fixture project, every windowless command plus a render). Book: guides/building-a-game-project.md, concepts/schemas.md. **The manifest's other keys** (#706, the last step): `flint play` with no scene opens `Project::current()?.default_scene()`; `flint bundle` / `flint export web` take name and start scene from `bundle::read_project(root)` (`[project] id` / `scene`) when neither a flag nor `bundle.toml` says, and the bundle's reference check (`bundle::candidates`) walks from the naming file's folder to the root, then `engine/`. `tests/project_done_when_cli.rs` is #706's done-when in a fixture game: every `AssetKind::ALL` needs a row in its `ASSETS`
- **`flint asset where <kind> <name> [--scene x] [--json]`** (ADR 0114 step 3, #706; `commands/asset_where.rs`, a variant of `AssetCommands`, not of `Commands`) -- prints `Project::explain`: the file the engine would load, the paths tried before it and any order-change note; with nothing found it lists every path and exits 1 (`--json`: one document on stdout, then `process::exit(1)`). The project is the scene's (`Project::for_path`), or the working directory's with no `--scene`, when the scene directory is `.`. The kind argument parses through `AssetKind::from_label`, and both `--help` (`after_help`) and a bad kind print `kinds_help()`, built from `AssetKind::ALL`, so a new kind shows up with no change here. Paths print through `project::shown`. A palette, look or glyph set the engine has built in and no file shadows is not a miss: `loads    the built-in palette 'amber'`, `"built_in": true`, exit 0 (`built_in()`, which asks flint-render's lists). Test: `tests/asset_where_cli.rs`. `flint render` finds the scene's project once (`AssetScope::for_scene`) and gives it to the user-style load, `--look`, the scene's own look, the model loader and the renderer (`set_asset_scope`); `--list-styles` and `flint style list` list through `catalog_in`

## Linux

- Linux (#220, checked on a GPU-less Ubuntu 24.04): building flint-cli needs `build-essential pkg-config libasound2-dev libudev-dev` (alsa-sys via kira/cpal, libudev-sys via gilrs; each build script fails without its package). Rendering with no GPU needs `mesa-vulkan-drivers` (lavapipe; `flint render --list-gpus` then shows `llvmpipe ... [Cpu, Vulkan]`). With no adapter, `RenderError::AdapterNotFound` and an empty `--list-gpus` print `gpu_select::no_adapter_hint()`; render-suite matches the message's leading "Failed to get adapter", so keep those words first. Book: getting-started/installation.md
- **`flint validate` and scene diagnostics** (ADR 0115 step 2, #683; `commands/validate.rs`) -- `run` asks `flint_scene::diagnose` first (`scene_diagnostics`; `--strict` -> `diagnostic::promote_strict`, the manifest's `strict` is applied inside `diagnose`). A scene that can't load (`toml-parse`, `file-unreadable`, `bad-prefab`) prints its diagnostics and exits 1 there. Otherwise the scene is loaded with `load_scene_diagnosed` (quiet: the list is already in hand) **even when no schema directory has constraints**, which used to return early with "No constraints found" and exit 0. Text: `diagnostics_text` (count line, each diagnostic errors first, `hint:` on the line below), then the constraint report; JSON: one document, `diagnostics` beside `violations`, with `valid` / `errors` / `warnings` counting both. Exit 1 when either list has an error; `--fix` / `--dry-run` print the diagnostics after the fix report and fail the same way. `check()` (what `flint examples --check` calls) returns the diagnostics as a third value, and `examples.rs` fails an example on any **error** diagnostic (a warning, such as an undeclared field, passes): `examples/` is kept free of them, with the components only its scripts read (`film`, `story`, `tour`, `orbit`) and the user style's `hologram_style` declared as open schemas in `schemas/examples/components/`. The frozen `tests/fixtures/scenes/` still carry stale `directional_light` / `point_light` tables and are not checked by it. Test: `tests/validate_diagnostics_cli.rs` (its own schemas and scenes in a temp project, so the engine's schema files can change under it). `bundle`'s reference checks and `flint check` are #687's

## `flint entity create/delete`

- **`flint schema`** (#684; `commands/schema.rs`, `commands/schema_export.rs`) -- `Commands::Schema(SchemaArgs)` (an `Args` struct, so the enum did not grow): `[name] [--list] [--format text|json|json-schema] [--json] [--output file] [--schemas dir]...`. A name that is neither a component nor an archetype is `bail!` (exit 1, nearest name suggested), checked before any output for every format. `schema_export` builds from a `SchemaRegistry`: `list_json` (`--list --json`), `registry_json` / `component_json` / `archetype_json` (`--format json`; `schema_dirs`, `types`, `old_type_names`, `components`, `archetypes`), `scene_json_schema` (draft-07: `definitions.entity` with `archetype` as an enum of the loaded ones, `parent`, a `$ref` per component and `additionalProperties: false`; `definitions.components.<name>` with typed fields and `additionalProperties` left open, since an unknown field is only a warning; `required` only for a field with no default that no archetype supplies; `[scene]` and `[prefabs]` described, other top-level tables allowed and undescribed until #686) and `component_schema` (`<name> --format json-schema`). Directories are named from the working directory where they are under it, so a committed schema carries no machine's paths. `--output` exists because a PowerShell redirect can add a byte-order mark. `tests/schema_cli.rs` compiles the scene schema with the `boon` crate (a dev-dependency) and validates every `examples/**/*.scene.toml` against it, then checks it refuses seven one-mistake scenes; it has not been tried in an editor. `flint validate` puts `registry.game_lints()` ahead of the scene's diagnostics and prefixes a diagnostic about another file with that file
- **Edit, don't rewrite** (#678, `commands/entity.rs`): both go through `flint_scene::SceneDocument` (`insert_entity`, `remove_entity`, `children_of`, `descendants_of`), so one table goes in after the last entity or comes out with the comment above it, and every other byte stays: never write back the `SceneFile` from `load_scene`, which has prefabs expanded, the wooze preset filled in and every `[post_process]` default set. The edited text is loaded with `flint_scene::load_scene_text(text, path, registry)` before writing, and each `--props` component is checked with `validate_component_data` (an error here, a warning at load); a component with no schema is written with a warning. `create` writes archetype, parent and props only. `delete` refuses a parent unless `--recursive`, and names the `[prefabs.<instance>]` for a prefab-made entity. Test: `tests/entity_cli.rs`
