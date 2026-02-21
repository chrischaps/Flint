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
| `flint serve <scene> --watch` | Interactive viewer with hot-reload |
| `flint play <scene>` | First-person gameplay with physics + scripting |
| `flint render <scene> -o out.png` | Headless render to PNG |
| `flint asset generate <type>` | AI asset generation (texture, model, audio) |
| `flint asset import <file>` | Import file into asset catalog |
| `flint edit <scene>` | Interactive spline/track editor |
| `flint prefab view <template>` | Preview a prefab template in the viewer |

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
| F1 | Cycle debug mode (PBR → Wireframe → Normals → Depth → UV → Unlit → Metal/Rough) |
| F4 | Toggle shadows |
| F5 | Toggle bloom |
| F6 | Toggle post-processing pipeline |
| F11 | Toggle fullscreen |
| Escape | Release cursor / Exit |

### Viewer (`flint serve`)

| Key | Action |
|-----|--------|
| Left-click | Select entity / pick gizmo axis |
| Left-drag | Orbit camera (or drag gizmo) |
| Right-drag | Pan camera |
| Scroll | Zoom |
| Ctrl+S | Save scene |
| Ctrl+Z | Undo position change |
| Ctrl+Shift+Z | Redo position change |
| F1 | Cycle debug mode |
| F2 | Toggle wireframe overlay |
| F3 | Toggle normal arrows |
| F4 | Toggle shadows |

### Editor (`flint edit`)

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
| `play_clip(id, clip)` | --- | Play an animation clip |
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
```
