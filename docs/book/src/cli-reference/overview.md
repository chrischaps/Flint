# CLI Reference

Flint's CLI is the primary interface for all engine operations. Below is a reference of available commands.

## Commands

| Command | Description |
|---------|-------------|
| `flint init <name>` | Initialize a new project |
| `flint entity create` | Create an entity in a scene |
| `flint entity delete` | Delete an entity from a scene |
| `flint scene create` | Create a new scene file |
| `flint scene list` | List scene files |
| `flint scene info` | Show scene metadata and entity count |
| `flint query "<query>"` | Query entities with the Flint query language |
| `flint schema <name>` | Inspect a component or archetype schema |
| `flint validate <scene>` | Validate a scene against constraints |
| `flint asset import` | Import a file into the asset store |
| `flint asset list` | List assets in the catalog |
| `flint asset info` | Show details for a specific asset |
| `flint asset resolve` | Check asset references in a scene |
| `flint asset generate` | Generate an asset using AI providers |
| `flint asset validate` | Validate a generated model against style constraints |
| `flint asset manifest` | Generate a build manifest of all generated assets |
| `flint asset regenerate` | Regenerate an existing asset with new parameters |
| `flint asset job status` | Check status of an async generation job |
| `flint asset job list` | List all generation jobs |
| `flint edit <file>` | Unified interactive editor (auto-detects file type) |
| `flint play <scene>` | Play a scene with first-person controls and physics |
| `flint render <scene>` | Render a scene to PNG (headless) |
| `flint gen <spec>` | Run a procedural generation spec to produce meshes or textures |
| `flint prefab view <template>` | Preview a prefab template in the viewer |
| `flint validate-suite <manifest>` | Validate a musical suite manifest and optional chart ([music commands](music.md)) |
| `flint play-suite <manifest>` | Play a suite's stems sample-locked, no judgment |
| `flint calibrate <manifest>` | Tap-to-beat latency calibration, written to `logs/latency/` |
| `flint play-chart <manifest>` | Play a suite against its chart with live gamepad capture |
| `flint replay-chart <manifest>` | Replay a recorded or synthetic session through judgment, headless |
| `flint render-suite <manifest>` | Render a scripted suite session to WAV, offline and deterministic |
| `flint spike-rumble` | Time the gamepad rumble command paths |

The seven music commands are documented on their own page: [Music Commands](music.md).

## The `play` Command

Launch a scene as an interactive first-person experience with physics:

```bash
flint play demo/phase4_runtime.scene.toml
flint play levels/tavern.scene.toml --schemas schemas --fullscreen
```

| Flag | Description |
|------|-------------|
| `--schemas <path>` | Path to schemas directory (repeatable; later paths override earlier). Default: `schemas` |
| `--fullscreen` | Launch in fullscreen mode |
| `--input-config <path>` | Input config overlay path (highest priority, overrides all other layers) |
| `--music-volume <f32>` | Initial gain for the `music` mixer bus |
| `--sfx-volume <f32>` | Initial gain for the `sfx` mixer bus |

`--music-volume 0` is the quickest way to audition a scene's sound design
without its score. See [Audio: Mixer Buses](../concepts/audio.md#mixer-buses).

The standalone `flint-player` binary takes the same flags plus `--msaa <1|4>`
(default `1`) for multisample anti-aliasing of the scene passes; `flint play`
does not expose `--msaa`.

### Player Controls (Defaults)

These are the built-in defaults. Games can override any binding via input config files (see [Physics and Runtime: Input System](../concepts/physics-and-runtime.md#input-system)).

| Input | Action |
|-------|--------|
| WASD | Move |
| Mouse | Look around |
| Left Click | Fire (weapon) |
| Space | Jump |
| Shift | Sprint |
| E | Interact with nearby object |
| R | Reload |
| 1 / 2 | Select weapon slot |
| Escape | Release cursor / Exit |
| F2 | Toggle the render stats overlay (FPS, frame time, draw stats) |
| F3 | Toggle the scene debug panels (see below); leaves the Rendering & Effects panel alone |
| F4 | Toggle the **Rendering & Effects** menu: every render and post-processing control, including debug shading mode, shadows, and the lighting levers |
| F9 | Force a music-session full-fail (debug builds only, needs a running session) |
| `` ` `` | Toggle the Music Guide overlay (debug builds only) |
| `\` | Toggle the Manifest Map timeline strip (debug builds only) |
| F11 | Toggle fullscreen |

The per-effect F-keys of earlier releases (F1 debug mode, F4 shadows, F5 bloom, F6 post-processing and so on) are gone; all of those toggles and their parameters live in the F4 menu (see [Post-Processing: The Rendering & Effects menu](../concepts/post-processing.md#the-rendering--effects-menu-f4)). F9, `` ` `` and `\` exist only when the player is built with the default `debug-hud` feature.

Gamepad controllers are also supported when connected. Bindings for gamepad buttons and axes can be configured in input config TOML files.

### Debug Panels (F3)

`F3` opens live tuning panels for whichever systems the current scene actually
uses. Each panel is created **only when its driving component is present**, so a
scene with no ocean never sees an ocean panel and a scene with no panels at all
logs a note instead. Panels are fold-open headers distributed across up to three
size-balanced columns.

Built-in panels:

| Panel | Component | Controls |
|-------|-----------|----------|
| Ocean Debug | `ocean` | Wave spectrum, colors, foam, contact foam, cel band edges, clarity/turbidity, grid, CPU/GPU parity probe |
| Day / Time | `time_of_day` | Clock readout, 0–24 h scrub slider, preset hours, natural-advance toggle, day counter, day length (with the effective ramped length), sun path tilt |
| Camera | `camera_tuning` | Vertical FOV |
| Grass Debug | `grass` | Density, height, wind, LOD distances |
| Weather | `weather` | Read-only state/wind/sea, `forced_state` override, one-shot snap / lightning-strike buttons |
| Reality | `reality` | Read-only active render-mode tear and mix, trigger-mode and end-now buttons, mix pin |
| Visitor | `raft_visitor` | Read-only phase/day plus a trigger-visit button |
| Dead Calm | `dead_calm` | Read-only phase/calm plus trigger and end-now buttons |
| Rendering & Effects | (always, when a renderer is active) | Post chain, SSAO, depth of field, fog, bloom, grade/grain/FXAA, Kuwahara, render mode, dither/volumetric, shadows and resolution, lighting levers, camera FOV, debug shading mode. Owned by **F4**, not F3 |
| Music Guide | `music_session` | Upcoming pulse/press/flick windows with countdown, per-channel targets beside the live stick and trigger state (`` ` ``) |
| Manifest Map | `music_session` | Full-width bottom strip: sections, bar ruler, tempo/meter changes, re-entry points, playhead and this run's judged pulses and seams (`\`) |

The full roster and the `DebugPanel` trait are described in [Debug Panels](../guides/debug-panels.md).

Edits apply live through the world's components. **Commit to File** writes the
current values back into the scene TOML, so a tuning session ends as a diff
rather than as notes. Values a game script owns each frame (a day counter, a
published factor) are deliberately never committed.

Games can add their own panels for their own components; unknown panels simply
take a default size weight in the column layout.

`Escape` releases the mouse so panels can be clicked; clicking the world
recaptures it.

The `play` command requires the scene to have a `player` archetype entity with a `character_controller` component. Physics colliders on other entities define the walkable geometry.

### Game Project Pattern

Games that define their own schemas, scripts, and assets use multiple `--schemas` paths. Game projects typically live in their own repositories with the engine included as a git subtree at `engine/`. The engine schemas come first, then the game-specific schemas overlay on top:

```bash
# From a game project root (engine at engine/)
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/level_1.scene.toml \
  --schemas engine/schemas \
  --schemas schemas
```

This loads the engine's built-in components (transform, material, rigidbody, etc.) from `engine/schemas/`, then adds game-specific components (health, weapon, enemy AI) from the game's own `schemas/`. See [Schemas: Game Project Schemas](../concepts/schemas.md#game-project-schemas) for directory structure details and [Building a Game Project](../guides/building-a-game-project.md) for the full workflow.

### Standalone Player Binary

The player is also available as a standalone binary for distribution:

```bash
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas

# With game project schemas (from a game repo with engine subtree)
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/level_1.scene.toml --schemas engine/schemas --schemas schemas
```

## The `render` Command

Render a scene to a PNG image without opening a window:

```bash
flint render demo/phase3_showcase.scene.toml --output hero.png --schemas schemas
flint render scene.toml -o shot.png --distance 20 --pitch 30 --yaw 45 --target 0,1,0 --no-grid
```

| Flag | Default | Description |
|------|---------|-------------|
| `--output <path>` / `-o` | `render.png` | Output file path |
| `--width <px>` | `1920` | Image width |
| `--height <px>` | `1080` | Image height |
| `--distance <f32>` | (auto) | Camera distance from target |
| `--yaw <deg>` | (auto) | Horizontal camera angle |
| `--pitch <deg>` | (auto) | Vertical camera angle |
| `--target <x,y,z>` | (auto) | Camera look-at point |
| `--fov <deg>` | (auto) | Field of view |
| `--no-grid` | `false` | Disable ground grid |
| `--debug-mode <mode>` | (none) | `wireframe`, `normals`, `depth`, `uv`, `unlit`, `metalrough` |
| `--wireframe-overlay` | `false` | Wireframe edges on solid geometry |
| `--show-normals` | `false` | Normal direction arrows |
| `--no-tonemapping` | `false` | Raw linear output |
| `--no-shadows` | `false` | Disable shadow mapping |
| `--shadow-resolution <px>` | `2048` | Shadow map resolution per cascade. A real control since the per-resolution texel upload (ADR 0049); earlier builds silently filtered as if 2048 |
| `--msaa <n>` | `1` | MSAA sample count for the scene passes: `1` (off) or `4`. Default 1 keeps headless pixel-diff gates single-sample (ADR 0058) |
| `--no-postprocess` | `false` | Disable post-processing |
| `--bloom-intensity <f32>` | `0.04` | Bloom strength |
| `--bloom-threshold <f32>` | `1.0` | Bloom brightness threshold |
| `--exposure <f32>` | `1.0` | Exposure multiplier |
| `--ssao-radius <f32>` | `0.5` | SSAO sample radius |
| `--ssao-intensity <f32>` | `1.0` | SSAO intensity (0 = disabled) |
| `--ssao-samples <n>` | `64` | SSAO hemisphere samples per pixel, 1–64; the kernel is strided so lower counts keep full radius coverage. `16` is ~4x cheaper |
| `--fog-density <f32>` | `0.02` | Fog density (0 = disabled) |
| `--fog-color <r,g,b>` | `0.7,0.75,0.82` | Fog color |
| `--fog-height-falloff <f32>` | `0.1` | Fog height falloff (enables height fog) |
| `--dither-intensity <f32>` | `0.03` | Ordered dither strength (enables dither; 0 = disabled) |
| `--volumetric-density <f32>` | `1.0` | Volumetric light density (enables god rays) |
| `--volumetric-samples <n>` | `32` | Volumetric ray-march sample count |
| `--desaturate <f32>` | `0` | Drain colour toward ash-grey (0 = full colour, 1 = fully drained) |
| `--dof <f32>` | `0` | Depth-of-field strength (0 = sharp, 1 = full defocus) |
| `--dof-focus <f32>` | `10.0` | Focus plane distance in world units |
| `--dof-range <f32>` | `5.0` | Focus half-width in world units |
| `--kuwahara-radius <n>` | `4` | Kuwahara filter radius in pixels (enables Kuwahara) |
| `--kuwahara-sharpness <f32>` | `8.0` | Kuwahara sector sharpness |
| `--kuwahara-hardness <f32>` | `8.0` | Kuwahara sector hardness |
| `--kuwahara-anisotropy <f32>` | `1.0` | Kuwahara anisotropy (0 = isotropic, 1 = full) |
| `--film-grain <f32>` | `0` | Animated film grain (0 = off; 0.02–0.05 is subtle) |
| `--grain-time <s>` | `0.0` | Post time for grain and render-mode animation. Deterministic: two renders at the same value are identical |
| `--particle-time <s>` | (none) | Simulate particle emitters and effects for this long at a fixed 1/60 s step before capturing. Deterministic; without it no particles are drawn |
| `--grade-lift <r,g,b>` | `0,0,0` | Colour-grade lift (per-channel add after ACES) |
| `--grade-gamma <r,g,b>` | `1,1,1` | Colour-grade gamma (per-channel curve) |
| `--grade-gain <r,g,b>` | `1,1,1` | Colour-grade gain (per-channel multiply) |
| `--fxaa` | `false` | Enable the FXAA anti-aliasing pass |
| `--oren-nayar <f32>` | `0` | Oren-Nayar diffuse blend (0 = Lambert, 1 = full); see [Lighting](../concepts/lighting.md) |
| `--sheen-strength <f32>` | `0` | Charlie-sheen rim strength (keep at or below about 0.3) |
| `--sheen-color <r,g,b>` | `1,1,1` | Charlie-sheen rim tint |
| `--render-mode <n>` | `0` | Stylized render mode: 1 Matrix, 2 blood, 3 drunk, 4 Tron, 5 underwater |
| `--mode-mix <f32>` | `0.0` | Render mode blend strength, 0--1 |
| `--mode-params <x,y,z,w>` | `0,0,0,0` | Per-mode parameters (see [Render Modes](../concepts/post-processing.md#render-modes)) |
| `--schemas <path>` | `schemas` | Schemas directory (repeatable) |

When no camera flags are given, the render starts from the scene's `[camera]` block if it has one. The camera and post flags override the scene's authored `[camera]` and `[post_process]` values.

> **`flint render` runs no scripts.** Anything your game drives from a script —
> a render mode, a time of day, a floating hull, an animated character — will
> not happen. The render-mode flags exist precisely so a stylized frame can be
> captured without one. For script-driven world state, bake a fixture scene
> with the values you want; for animation, note that skinned meshes render at
> **bind pose** headlessly. To capture a posed frame of a rigged model, use the
> model previewer instead: `flint edit model.glb --render out.png --anim-time 1.5`
> (optionally with `--clip`, `--layer` or `--sequence`). Particles are the
> exception: `--particle-time 2` steps every emitter before the capture.

## The `edit` Command

Unified interactive editor that auto-detects file type and opens the appropriate tool:

```bash
flint edit levels/demo.scene.toml              # Scene viewer (hot-reload)
flint edit levels/demo.scene.toml --spline     # Spline/track editor
flint edit models/character.glb                # Model previewer (orbit camera)
flint edit models/character.glb --watch        # Model previewer with file watching
flint edit specs/oak_tree.procgen.toml         # Procgen previewer (mesh/texture)
flint edit specs/stone_wall.procgen.toml       # Texture pipeline editor (if pipeline pattern)
flint edit terrain.terrain.toml                # Terrain editor
flint edit fx/fire.particles.toml              # Particle effect editor (created from --preset if missing)
```

### File Type Detection

| Extension | Tool | Description |
|-----------|------|-------------|
| `.scene.toml`, `.chunk.toml` | Scene viewer | Hot-reload, egui inspector, gizmos |
| `.procgen.toml` (pipeline pattern) | Texture pipeline editor | Node graph for texture specs |
| `.procgen.toml` (other) | Procgen previewer | Live preview of generated mesh/texture |
| `.terrain.toml` | Terrain editor | Heightmap terrain editing |
| `.particles.toml` | Particle effect editor | Emitters, curves, forces, bursts; scrub timeline; headless `--render` |
| `.glb`, `.gltf` | Model previewer | Orbit camera, animation playback |

### Common Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--schemas <path>` | `schemas` | Schemas directory (repeatable) |
| `--width <px>` | (auto) | Window width |
| `--height <px>` | (auto) | Window height |
| `--no-grid` | `false` | Disable ground grid |
| `--watch` | `false` | Watch for file changes |
| `--seed <u64>` | (auto) | Override seed (procgen) |
| `--no-inspector` | `false` | Hide egui inspector (scene) |
| `--spline` | `false` | Open the spline/track editor instead of the viewer (scene) |
| `--auto-orbit` | `false` | Start with the turntable auto-orbit on (scene/model/procgen); toggle with **O**, speed with `[` / `]` |

### Model Previewer Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--distance <f32>` | (auto) | Camera orbit distance |
| `--yaw <deg>` | (auto) | Horizontal camera angle |
| `--pitch <deg>` | (auto) | Vertical camera angle |
| `--target <x,y,z>` | (auto) | Camera look-at point |
| `--fov <deg>` | (auto) | Field of view |
| `--no-animate` | `false` | Disable animation playback |
| `--clip <name>` | (none) | Start with a specific animation clip |
| `--anim-speed <f32>` | `1.0` | Animation playback speed multiplier |
| `--layer <clip[:weight[:mask[:mode]]]>` | (none) | Add an animation layer; repeatable. `mode` is `additive` or `override`, `mask` a root joint name |
| `--sequence <file.sequence.toml>` | (none) | Play a sequence of timestamped animator events |
| `--sequence-loop` | `false` | Loop the sequence regardless of its `loop` setting |
| `--anim-time <s>` | (none) | With `--render`: sample the animation (or replay the sequence) at this time |
| `--render <path>` | (none) | Render to PNG instead of opening a window |

### Particle Editor Flags

The camera flags above (`--distance`, `--yaw`, `--pitch`, `--target`, `--fov`), `--render` and `--anim-time` apply to `.particles.toml` files too. In addition:

| Flag | Default | Description |
|------|---------|-------------|
| `--preset <name>` | `sparks` | Preset written when the file does not exist yet: `fire`, `smoke`, `sparks`, `rain` |
| `--anim-time <s>` | `1.0` | With `--render`: simulation time of the snapshot (fixed 1/120 s steps; deterministic) |

### Particle Editor Controls

| Input | Action |
|-------|--------|
| Space / R | Play-pause / restart |
| Home / End, ← / → | Seek start / end, step (Shift: 0.1 s) |
| L, `[` / `]` | Loop, halve / double speed |
| O, G, X, B, H | Auto-orbit, grid, shape gizmos, backdrop, hide UI |
| Ctrl+S, Ctrl+Z / Ctrl+Y, Ctrl+D, Delete, Ctrl+R | Save, undo / redo, duplicate emitter, delete emitter, reload |
| Curve widgets | Drag keys; double-click adds; right-click removes; Shift-drag locks t |

See the [Particle Editor guide](../guides/particle-editor.md).

### Scene Viewer Controls

| Input | Action |
|-------|--------|
| Left-click | Select entity / pick gizmo axis |
| Left-drag | Orbit camera (or drag gizmo if axis selected) |
| Right-drag | Pan camera |
| Scroll | Zoom |
| W / A / S / D | Orbit camera (when no entity is selected) |
| Q / E | Zoom out / in (when no entity is selected) |
| W / E / R | Switch gizmo mode: translate / rotate / scale (while an entity is selected) |
| Space | Return to the scene's authored `[camera]` framing (viewer default if the scene has none) |
| O | Toggle auto-orbit |
| `[` / `]` | Slow down / speed up auto-orbit |
| Ctrl+S | Save scene to disk |
| Ctrl+Z | Undo position change |
| Ctrl+Shift+Z | Redo position change |
| Escape | Cancel gizmo drag / exit |
| F2 | Toggle the render stats overlay |
| F3 | Toggle normal arrows |
| F4 | Toggle the **Rendering & Effects** menu (all render and post toggles and parameters, debug shading mode, shadows, an authored-vs-viewer-default post switch, DoF follow, and the live Particles controls) |

The viewer applies the scene's `[post_process]` block on load; the F4 menu's "authored" switch flips between those values and the viewer defaults. The earlier F1 (debug mode), F2 (wireframe overlay) and F4 (shadows) keys were folded into the F4 menu.

When an entity is selected, a **translate gizmo** appears with colored axis arrows (red = X, green = Y, blue = Z) and plane handles. Click and drag an axis or plane to move the entity. Position changes can be undone/redone and saved back to the scene file.

### Spline Editor Controls (`--spline`)

| Input | Action |
|-------|--------|
| Left-click | Select control point |
| Left-drag | Move control point on constraint plane |
| Alt + drag | Move control point vertically (Y axis) |
| Middle-drag | Orbit camera |
| Right-drag | Pan camera |
| Scroll | Zoom |
| Tab / Shift+Tab | Cycle through control points |
| I | Insert a new control point after selected |
| Delete | Remove selected control point |
| Ctrl+S | Save spline to disk |
| Ctrl+Z | Undo |

> **Legacy aliases:** `flint serve`, `flint preview`, `flint gen-preview`, `flint tex-edit`, `flint terrain-edit`, and `flint spline-edit` still work. They are hidden subcommands that open the same tools `flint edit` dispatches to; they are omitted from `flint --help`.

## The `asset generate` Command

Generate assets using AI providers:

```bash
flint asset generate texture -d "rough stone wall" --style medieval_tavern
flint asset generate model -d "wooden chair" --provider meshy --seed 42
flint asset generate audio -d "tavern ambient noise" --duration 10.0
```

| Flag | Description |
|------|-------------|
| `-d`, `--description` | Generation prompt (required) |
| `--name` | Asset name (derived from description if omitted) |
| `--provider` | Provider to use: `flux`, `meshy`, `elevenlabs`, `mock` |
| `--style` | Style guide name (e.g., `medieval_tavern`) |
| `--width`, `--height` | Image dimensions for textures (default: 1024x1024) |
| `--seed` | Random seed for reproducibility |
| `--tags` | Comma-separated tags |
| `--output` | Output directory (default: `.flint/generated`) |
| `--duration` | Audio duration in seconds (default: 3.0) |

Generated assets are automatically stored in content-addressed storage and registered in the asset catalog with a `.asset.toml` sidecar. Models are validated against style constraints after generation.

## The `gen` Command

Run a procedural generation spec to produce meshes (GLB) or textures (PNG):

```bash
flint gen specs/oak_tree.procgen.toml -o tree.glb
flint gen specs/stone_wall.procgen.toml -o wall.png
flint gen specs/oak_tree.procgen.toml --dry-run
flint gen specs/oak_tree.procgen.toml --seed 42 -o tree.glb
flint gen specs/oak_tree.procgen.toml --batch 10 --seed-start 0
```

| Flag | Default | Description |
|------|---------|-------------|
| `-o, --output <path>` | (derived from spec) | Output file or directory |
| `--seed <u64>` | (from spec) | Override the spec's seed |
| `--dry-run` | `false` | Print estimated cost without generating |
| `--format <fmt>` | (auto) | Force output format: `glb` or `png` |
| `--batch <N>` | (none) | Generate N variants with sequential seeds |
| `--seed-start <u64>` | `0` | Starting seed for batch generation |
| `--register` | `false` | Store output in content store with provenance |
| `--force` | `false` | Regenerate even if cached |
| `--validate` | `false` | Validate output after generation |
| `--strict` | `false` | Treat warnings as failures |
| `--style-guide <path>` | (none) | Style guide TOML for validation constraints |

## The `prefab view` Command

Preview a prefab template in the interactive viewer:

```bash
flint prefab view prefabs/kart.prefab.toml --schemas engine/schemas --schemas schemas
```

| Flag | Default | Description |
|------|---------|-------------|
| `--prefix <string>` | `"preview"` | Prefix for `${PREFIX}` substitution |
| `--schemas <path>` | `schemas` | Schemas directory (repeatable) |

This command loads the `.prefab.toml` template, performs variable substitution, builds a synthetic scene from the expanded entities, and launches the viewer for visual inspection. Useful for verifying prefab structure and appearance without creating a full scene.

## Common Flags

| Flag | Description |
|------|-------------|
| `--scene <path>` | Path to scene file |
| `--schemas <path>` | Path to schemas directory (repeatable for multi-schema layering; default: `schemas`) |
| `--format <fmt>` | Output format: `json`, `toml`, or `text` |
| `--fix` | Apply auto-fixes (with `validate`) |
| `--dry-run` | Preview changes without applying |

## Usage

```bash
# Get help
flint --help
flint <command> --help

# Examples
flint init my-game
flint edit levels/tavern.scene.toml              # Interactive scene viewer
flint edit models/character.glb --watch           # Model previewer
flint play levels/tavern.scene.toml
flint render levels/tavern.scene.toml -o shot.png
flint gen specs/oak_tree.procgen.toml -o tree.glb
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml
```
