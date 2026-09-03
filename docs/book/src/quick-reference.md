# Quick Reference

A scannable cheat sheet for daily Flint development.

## CLI Commands

| Command | Description |
|---------|-------------|
| `flint init <name>` | Initialize a new project |
| `flint scene create <path>` | Create a new scene file |
| `flint scene list` | List scene files |
| `flint scene info` | Show scene metadata |
| `flint entity create` | Create an entity in a scene |
| `flint entity delete` | Delete an entity from a scene |
| `flint query "<expr>"` | Query entities (e.g., `"entities where archetype == 'door'"`) |
| `flint schema <name>` | Inspect a component or archetype schema |
| `flint validate <scene>` | Validate scene against constraints (`--fix` to auto-fix) |
| `flint edit <file>` | Unified interactive editor (auto-detects file type) |
| `flint play <scene>` | First-person gameplay with physics + scripting |
| `flint render <scene> -o out.png` | Headless render to PNG |
| `flint gen <spec> -o out.glb` | Run procgen spec to produce mesh/texture |
| `flint asset generate <type>` | AI asset generation (texture, model, audio) |
| `flint asset import <file>` | Import file into asset catalog |
| `flint prefab view <template>` | Preview a prefab template in the viewer |
| `flint validate-suite <manifest>` | Validate a music suite manifest (`--chart` to cross-check) |
| `flint play-suite <manifest>` | Play a suite's stems, no judgment |
| `flint calibrate <manifest>` | Tap-to-beat latency calibration |
| `flint play-chart <manifest> --chart <chart>` | Play a chart with live gamepad capture (`--record`, `--window`) |
| `flint replay-chart <manifest> --chart <chart>` | Replay a recorded or synthetic session headless |
| `flint render-suite <manifest> -o out.wav` | Render a scripted suite session to WAV |
| `flint spike-rumble` | Time the gamepad rumble paths |

Music commands are detailed in [Music Commands](cli-reference/music.md).

## Keyboard Shortcuts

### Player (`flint play`)

| Key | Action |
|-----|--------|
| WASD | Move |
| Mouse | Look around |
| Space | Jump |
| Shift | Sprint |
| E | Interact |
| Left Click | Fire |
| R | Reload |
| 1 / 2 | Weapon slots |
| F2 | Toggle render stats overlay |
| F3 | Toggle scene debug panels (ocean, day/time, camera, grass, weather… — only those the scene uses; never the F4 menu) |
| F4 | Toggle the **Rendering & Effects** menu (all render/post toggles and parameters, debug shading mode, shadows, lighting levers) |
| F9 | Force a music-session full-fail (debug builds, session running) |
| `` ` `` / `\` | Music Guide overlay / Manifest Map strip (debug builds, session running) |
| F11 | Toggle fullscreen |

The old per-effect keys (F1 debug mode, F4 shadows, F5 bloom, F6 post) are gone; everything is in the F4 menu.
| Escape | Release cursor / Exit |

### Scene Viewer (`flint edit <scene.toml>`)

| Key | Action |
|-----|--------|
| Left-click | Select entity / pick gizmo axis |
| Left-drag | Orbit camera (or drag gizmo) |
| Right-drag | Pan camera |
| Scroll | Zoom |
| W/A/S/D, Q/E | Orbit, zoom out/in (no entity selected) |
| W / E / R | Gizmo mode: translate / rotate / scale (entity selected) |
| Space | Return to the scene's authored `[camera]` framing |
| O, `[` / `]` | Toggle auto-orbit, slower / faster |
| Ctrl+S | Save scene |
| Ctrl+Z | Undo position change |
| Ctrl+Shift+Z | Redo position change |
| F2 | Toggle render stats |
| F3 | Toggle normal arrows |
| F4 | Toggle the **Rendering & Effects** menu |

### Spline Editor (`flint edit <scene.toml> --spline`)

| Key | Action |
|-----|--------|
| Left-click | Select control point |
| Left-drag | Move control point |
| Alt+drag | Move vertically (Y) |
| Middle-drag | Orbit |
| Right-drag | Pan |
| Tab / Shift+Tab | Cycle control points |
| I | Insert point |
| Delete | Remove point |
| Ctrl+S | Save spline |
| Ctrl+Z | Undo |

### Rendering & Effects Menu (F4, player and viewer)

One panel for everything the old F-keys flipped. Sections: **Post chain** (enable, exposure, vignette, chromatic aberration, radial blur, desaturate; player-only "freeze script post overrides"), **SSAO** (radius, intensity, bias, samples), **Depth of field** (strength, focus distance, range; viewer adds DoF-follow-selection), **Fog** and height fog, **Bloom** with film grain and FXAA, **Kuwahara**, **Render mode** (none / Matrix / blood / drunk / Tron / underwater, mix, params), **Dither / Volumetric**, **Shadows** (enable, resolution — rebuilds the shadow pass), **Lighting** (ambient sky/ground, diffuse wrap, Oren-Nayar, sheen, reset), **Camera** (vertical FOV), **Shading** (debug mode combo). The viewer also has an authored-vs-viewer-default post switch.

### File Type Auto-Detection (`flint edit`)

| Extension | Opens |
|-----------|-------|
| `.scene.toml`, `.chunk.toml` | Scene viewer |
| `.procgen.toml` | Procgen previewer (or texture pipeline editor) |
| `.terrain.toml` | Terrain editor |
| `.glb`, `.gltf` | Model previewer (orbit camera) |

## Common TOML Snippets

### Minimal Entity

```toml
[entities.my_thing]
archetype = "furniture"

[entities.my_thing.transform]
position = [0, 1, 0]
rotation = [0, 45, 0]
scale = [1, 1, 1]
```

### PBR Material

```toml
[entities.my_thing.material]
base_color = [0.8, 0.2, 0.1]
roughness = 0.6
metallic = 0.0
emissive = [1.0, 0.4, 0.1]
emissive_strength = 2.0
```

### Physics Body

```toml
[entities.wall.collider]
shape = "box"
size = [10.0, 4.0, 0.5]

[entities.wall.rigidbody]
body_type = "static"
```

### Particle Emitter (Fire)

```toml
[entities.fire.particle_emitter]
emission_rate = 40.0
max_particles = 200
lifetime_min = 0.3
lifetime_max = 0.8
speed_min = 1.5
speed_max = 3.0
direction = [0, 1, 0]
gravity = [0, 2.0, 0]
size_start = 0.15
size_end = 0.02
color_start = [1.0, 0.7, 0.1, 0.9]
color_end = [1.0, 0.1, 0.0, 0.0]
blend_mode = "additive"
shape = "sphere"
shape_radius = 0.15
autoplay = true
```

### Post-Processing

```toml
[post_process]
bloom_enabled = true
bloom_intensity = 0.04
bloom_threshold = 1.0
vignette_enabled = true
vignette_intensity = 0.3
exposure = 1.0
ssao_samples = 16               # 1-64; 16 is ~4x cheaper than the default 64
desaturate = 0.0                # 0 = full colour, 1 = ash-grey
dof_strength = 0.0              # 0 = sharp
dof_focus_distance = 10.0       # view metres
dof_focus_range = 5.0
kuwahara_enabled = false        # painterly pre-pass
film_grain = 0.0                # 0.02-0.05 is subtle
grade_lift = [0.0, 0.0, 0.0]    # after ACES; neutral 0,0,0
grade_gamma = [1.0, 1.0, 1.0]
grade_gain = [1.0, 1.0, 1.0]
fxaa = false                    # off by default so pixel gates stay single-path
```

### Lighting Levers

```toml
[environment]
ambient_sky = [0.35, 0.40, 0.50]
ambient_ground = [0.18, 0.14, 0.10]
diffuse_wrap = 0.3      # 0 = legacy sharp terminator
oren_nayar = 0.7        # 0 = Lambert
sheen_color = [1.0, 0.9, 0.8]
sheen_strength = 0.15   # keep <= ~0.3
```

### Authored Camera

```toml
[camera]
position = [0, 4, 12]
target = [0, 1, 0]
fov = 60.0
```

### UI Layout

```toml
# ui/hud.ui.toml
[ui]
name = "HUD"
style = "ui/hud.style.toml"

[elements.score_panel]
type = "panel"
anchor = "top-right"
class = "hud-panel"

[elements.score_text]
type = "text"
parent = "score_panel"
class = "score-value"
text = "0"
```

### UI Style

```toml
# ui/hud.style.toml
[styles.hud-panel]
width = 160
height = 50
bg_color = [0.0, 0.0, 0.0, 0.6]
rounding = 6
padding = [10, 8, 10, 8]
x = -10
y = 10

[styles.score-value]
font_size = 28
color = [1.0, 1.0, 1.0, 1.0]
text_align = "center"
width_pct = 100
```

### Script Attachment

```toml
[entities.npc.script]
source = "npc_behavior.rhai"
enabled = true

[entities.npc.interactable]
prompt_text = "Talk"
range = 3.0
interaction_type = "talk"
```

### Audio Source

```toml
[entities.campfire.audio_source]
file = "audio/fire_crackle.ogg"
volume = 0.8
loop = true
spatial = true
min_distance = 1.0
max_distance = 15.0
```

### Prefab Instance

```toml
[prefabs.player]
template = "kart"
prefix = "player"

[prefabs.player.overrides.kart.transform]
position = [0, 0, 0]
```

## Top Scripting Functions

| Function | Returns | Description |
|----------|---------|-------------|
| `self_entity()` | `i64` | ID of the entity this script is attached to |
| `get_entity(name)` | `i64` | Look up entity by name (`-1` if not found) |
| `get_field(id, comp, field)` | `Dynamic` | Read a component field |
| `set_field(id, comp, field, val)` | --- | Write a component field |
| `get_position(id)` | `#{x,y,z}` | Entity position |
| `set_position(id, x, y, z)` | --- | Set entity position |
| `distance(a, b)` | `f64` | Distance between two entities |
| `is_action_pressed(action)` | `bool` | Check if action is held |
| `is_action_just_pressed(action)` | `bool` | Check if action pressed this frame |
| `delta_time()` | `f64` | Seconds since last frame |
| `play_sound(name)` | --- | Play a sound effect |
| `set_dof(strength)` / `set_dof_focus(dist, range)` | --- | Drive depth of field from a script |
| `set_desaturation(amount)` | --- | Drain colour toward ash-grey (0–1) |
| `set_camera_roll(radians)` | --- | Roll the camera about its view axis |
| `conducted_lean()` / `conducted_coherence()` | `#{x,y}` / `f64` | Read the running music session (neutral values when none) |
| `play_clip(id, clip)` | --- | Play an animation clip |
| `set_anim_layer(id, idx, clip, w)` | --- | Play a clip on an animation layer |
| `set_anim_layer_weight(id, idx, w)` | --- | Set a layer's weight instantly |
| `fade_anim_layer(id, idx, w, secs)` | --- | Ramp a layer's weight over `secs` |
| `play_sequence(id, name)` / `stop_sequence(id)` | --- | Drive the animator from `animations/*.sequence.toml` (cues → `on_sequence_cue`) |
| `raycast(ox,oy,oz, dx,dy,dz, dist)` | `Map`/`()` | Cast a ray, get hit info |
| `move_character(id, dx, dy, dz)` | `#{x,y,z,grounded}` | Collision-corrected movement |
| `spawn_entity(name)` | `i64` | Create a new entity |
| `load_scene(path)` | --- | Transition to a new scene |
| `push_state("paused")` | --- | Push a game state (e.g., pause) |
| `pop_state()` | --- | Pop to previous game state |
| `persist_set(key, val)` | --- | Store data across scene transitions |
| `load_ui(path)` | `i64` | Load a `.ui.toml` document (returns handle) |
| `ui_set_text(id, text)` | --- | Set element text content |
| `ui_show(id)` / `ui_hide(id)` | --- | Toggle element visibility |
| `ui_set_style(id, prop, val)` | --- | Override a style property at runtime |

## Render Command Quick Examples

```bash
# Basic screenshot
flint render scene.toml -o shot.png --schemas schemas

# Framed hero shot
flint render scene.toml -o hero.png --distance 20 --pitch 30 --yaw 45 --target 0,1,0 --no-grid

# Debug views
flint render scene.toml -o wireframe.png --debug-mode wireframe
flint render scene.toml -o normals.png --debug-mode normals
flint render scene.toml -o depth.png --debug-mode depth

# Post-processing control
flint render scene.toml -o bloom.png --bloom-intensity 0.08
flint render scene.toml -o raw.png --no-postprocess
flint render scene.toml -o dof.png --dof 0.6 --dof-focus 8 --dof-range 3
flint render scene.toml -o graded.png --grade-lift 0.03,0.02,0.015 --grade-gain 1.04,1,0.94 --film-grain 0.03
flint render scene.toml -o drained.png --desaturate 0.85

# Anti-aliasing and lighting levers (both default off; ADR 0058 / 0048)
flint render scene.toml -o smooth.png --msaa 4 --fxaa
flint render scene.toml -o clay.png --oren-nayar 0.7 --sheen-strength 0.15 --sheen-color 1,0.9,0.8

# Cheaper SSAO for quick iteration
flint render scene.toml -o fast.png --ssao-samples 16
```
