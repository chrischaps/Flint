# Player Extensions

Most Flint games are scenes, schemas and Rhai scripts that the stock `flint-player` runs. Some games need native Rust in the loop. A game might hold a network connection and apply what arrives before scripts read the world, or build meshes from data, or draw panel-heavy UI with egui widgets. Such a game builds its own binary around `PlayerApp` and installs a **player extension**.

An extension is a type that implements `flint_player::PlayerExtension`. Every method has a no-op default, so you implement only what you need. A player with no extensions behaves exactly as `flint-player` does.

## A Game Binary

The game crate depends on the engine crates by path, usually through a subtree at `engine/` (see [Building a Game Project](building-a-game-project.md)). Any third-party crate that crosses the engine boundary (`egui`, `winit`, `wgpu`, `rhai`, `glam`) must use the **same version as `engine/Cargo.toml`**. Otherwise two copies get linked and an `egui::Context` from one isn't the other's type. `rhai` needs its `sync` feature, as the engine uses it.

```rust
use flint_player::{ExtensionCtx, PlayerApp, PlayerExtension};
use winit::event_loop::{ControlFlow, EventLoop};

struct MyGame;

impl PlayerExtension for MyGame {
    fn register_script_api(&mut self, engine: &mut rhai::Engine) {
        engine.register_fn("server_name", || "localhost".to_string());
    }
    fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
        // apply network state to ctx.world before scripts run
    }
    fn ui(&mut self, ctx: &egui::Context) {
        egui::Window::new("Inventory").show(ctx, |ui| { ui.label("empty"); });
    }
}

fn main() -> anyhow::Result<()> {
    let dirs = vec!["engine/schemas".to_string(), "schemas".to_string()];
    let refs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let registry = flint_schema::SchemaRegistry::load_from_directories(&refs)?;
    let (world, scene) = flint_scene::load_scene("scenes/main.scene.toml", &registry)?;

    let mut player = PlayerApp::new(
        world,
        "scenes/main.scene.toml".into(),
        false,
        None,
        scene.scene.input_config.clone(),
    );
    player.scene_camera = scene.camera.clone();
    player.scene_post_process = scene.post_process.clone();
    player.set_schema_paths(dirs);
    player.add_extension(Box::new(MyGame));

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut player)?;
    Ok(())
}
```

`PlayerApp` is a winit `ApplicationHandler`, so it runs directly with no wrapper. Set the same public fields `flint-player`'s `main.rs` sets for the features your scenes use: window size, environment and skybox, and `capture` for `--screenshot`-style runs. Pass the engine's `schemas/` yourself, ahead of the game's.

## The Hooks

| Method | When it runs | Use it for |
|---|---|---|
| `register_script_api(&mut rhai::Engine)` | Once, inside `add_extension` | Native functions and types for scripts |
| `pre_update(&mut ExtensionCtx)` | Every frame, before the scripts' `on_update` | Writing the world, firing events, uploading meshes |
| `on_window_event(&WindowEvent) -> bool` | Every window event that egui didn't take | Game input the scene's input config can't express |
| `on_key(&KeyInput) -> bool` | Every key egui didn't take, real or from `--input-script` | Keys a capture should be able to press |
| `ui(&egui::Context)` | Every frame, inside the player's egui pass | The game's own panels and windows |
| `fonts(&mut egui::FontDefinitions)` | Whenever the player installs egui's fonts | Fonts the project's `fonts/` folder can't give |

Extensions run in the order they were added.

### Script functions

`register_script_api` gets the player's Rhai engine. Flint's own API is already registered, so a function with the same name and parameter types replaces Flint's. The engine outlives scene transitions, so registrations last the whole run.

The engine uses rhai's `sync` feature, so a registered closure and everything it captures must be `Send + Sync`. Share state between the extension and its functions with `Arc<Mutex<_>>` or atomics.

### The frame

Each frame the player ticks the clock, applies any input script, advances a scene transition, then calls every extension's `pre_update`. Game events are collected next, and then scripts' `on_update` runs. Whatever an extension writes to the world is visible to scripts **in the same frame**.

`ExtensionCtx` gives `pre_update`:

- `world`: the ECS world, mutable.
- `clock`: already ticked for this frame.
- `input`: this frame's input state, before scripts read it.
- `camera`: **last** frame's camera. Scripts and physics may move it later this frame.
- `gpu`: `Option<ExtensionGpu>`, with the renderer, device and queue. It's `None` until the window and renderer exist, and in headless runs, so check it every frame rather than once.
- `persist`: the persistent store scripts use, mutable (see [Saved data](#saved-data)).
- `audio`: play sounds, load sounds made in memory, and set the mixer buses (see [Sound](#sound)).

### Talking to scripts

`ctx.fire_event(name, data)` fires a custom game event. Scripts receive it in `on_event(name, data)` this same frame, before their `on_update` (see [Scripting](../concepts/scripting.md)). Audio triggers see it too.

```rust
let mut data = toml::Table::new();
data.insert("count".into(), 3.into());
ctx.fire_event("inventory_changed", data);
```

A table arrives in scripts as a map, and anything else arrives as `#{ value: data }`. The payload is TOML, which has no null. If you forward JSON from a server, drop `null` fields or leave the keys out.

In the other direction, scripts call the native functions the extension registered.

### Saved data

`ctx.persist` is the same `PersistentStore` that scripts reach with `persist_get` and `persist_set`, the one the player keeps in `save/persist.toml` (see [Scripting: Persistence](../concepts/scripting.md#persistence)). It's loaded before any script's `on_init`, so the first `pre_update` already sees last session's values.

```rust
let volume = ctx.persist.get("music_volume").and_then(|v| v.as_float()).unwrap_or(0.8);
ctx.persist.set("last_level", "harbour".into());
```

A write in `pre_update` is visible to scripts in that frame's `on_update`, and a script's write is visible to extensions the next frame. Extension writes join the player's own save: one second after a burst of changes, on every scene transition, and on exit. `ctx.persist.request_flush()` saves at the end of this frame instead, as a script's `persist_save()` does.

Every script can read every key, so keep secrets such as session tokens in a file of the game's own. The store is only reachable from `pre_update`. If the game changes a setting in `ui`, keep the change and write it in the next `pre_update`.

### Sound

`ctx.audio` is an `ExtensionAudio`, a handle on the player's audio engine. Sounds are cached by name, in the same cache that scripts' `play_sound` and `audio_source.file` read. A sound an extension loads can be played by a script, and a file the scene preloaded (`audio/<file>`) can be played from Rust.

```rust
use flint_player::{Bus, SoundOptions};

// Once: a sound made in code (one channel, or two for left and right).
ctx.audio.load_samples("blip", 44_100, &[&samples])?;
// Or an encoded one held in memory: WAV, OGG, FLAC or MP3.
ctx.audio.load_bytes("door", include_bytes!("door.ogg").to_vec())?;

// Any frame:
ctx.audio.play("blip", &SoundOptions::new().volume(0.5).pitch(1.2))?;
ctx.audio.play("door", &SoundOptions::new().at(pos).distances(2.0, 60.0))?;
ctx.audio.play("bell", &SoundOptions::new().at(pos).distances(20.0, 600.0).rolloff(0.7))?;
ctx.audio.set_bus_volume(Bus::Music, 0.4);
```

- **`play`** plays a sound once. Without `at`, it plays flat, from nowhere. With `at`, it pans and fades with distance from the listener: full volume within `distances`' first value and silent past its second (by default 1 and 25, the same as a script's `play_sound_at`). `rolloff` sets the curve between them: the default 2 holds a sound up and drops it near the far distance, and a value below 1 fades it like a sound in the open (see [Audio](../concepts/audio.md#scripting-integration)). `bus` routes it through `Bus::Sfx` (the default) or `Bus::Music`. An unknown name is an error.
- **`load_samples`** and **`load_bytes`** replace any sound already cached under the name. A sound that's already playing carries on with the old data. A looping `audio_source` whose sound isn't cached yet stays silent, so load the sound before spawning the entity that plays it.
- **`set_bus_volume`** scales everything on the bus, including scripts' sounds and `audio_source`s, on top of each sound's own volume. It holds across scene transitions. A game's volume settings belong here rather than in each sound's volume.

Loading needs no audio device. Without one, `play` does nothing and returns `Ok`, so a game doesn't need to check `is_available()` first. Like the rest of `ExtensionCtx`, `audio` is only reachable from `pre_update`. A sound triggered in `ui` (a button's click) is queued and played in the next `pre_update`.

### Meshes at runtime

`ExtensionGpu::upload_mesh(name, vertices, indices, material)` creates or replaces a named mesh. `remove_mesh(name)` removes it. `ctx.attach_mesh(entity, name)` makes an entity draw it. A mesh can be rebuilt as often as its data changes, and the old GPU buffers are freed once the frame lets go of them. Vertex colours, culling and the `runtime = true` scene flag are covered in [Rendering: Runtime meshes](../concepts/rendering.md#runtime-meshes).

## Game UI with egui

`ui()` draws into the same egui pass as the script HUD and the debug panels. Back to front, the order is:

1. script UI (`on_draw_ui`)
2. each extension's `ui()`
3. script UI drawn between `push_over_ui()` and `pop_over_ui()` ([Scripting: Over the UI](../concepts/scripting.md#over-the-ui))
4. the [debug panels](debug-panels.md)

Egui popups and tooltips draw over the first three.

Use `egui::Window`, `egui::Area`, or side and top panels. Don't use a `CentralPanel`: it covers the whole window and takes every click away from the game.

### Who gets an input event

Once any extension is installed, or any debug panel is open, every window event goes to egui first. An event egui wants is consumed, and extensions, the player and scripts never see it.

- **Keyboard and IME** are consumed while an egui widget has keyboard focus, such as a text field being typed in. Tab isn't consumed when nothing is focused, so a game can still bind it.
- **Pointer** buttons and the wheel are consumed over an egui area. Cursor motion is consumed only while egui owns a drag. Clicks on empty space reach the game.
- **A release follows its press.** If egui took a key's press, it takes the release too. If the game saw the press, it gets the release. Moving focus mid-press never leaves a key stuck down on either side.
- **Escape** in a focused text field unfocuses the field instead of quitting.

Next, `on_window_event` offers each remaining event to the extensions in order. Returning `true` consumes it, and later extensions and the player don't see it. Only input events can be consumed. Lifecycle events such as `Resized`, `CloseRequested` and `RedrawRequested` always reach every extension and the player, whatever an extension returns.

A key then goes to the same extension's `on_key`, unless its `on_window_event` took it. `KeyInput` holds the key's `KeyCode`, whether it went down or up, whether the OS is repeating it, the character it stands for (winit's `logical_key`, on the release too), the modifiers held, and whether `--input-script` pressed it. `pressed()` is a first press. A scripted key comes **only** to `on_key` (ADR 0103): winit's `KeyEvent` can't be built outside winit, so there is no `KeyboardInput` to show `on_window_event`. Read keys in `on_key`, then, and a capture can press them; handle each key in one hook or the other, not both. Keys winit can't name, and the synthetic presses it sends for keys held as the window gains focus, come only to `on_window_event`.

```rust
fn on_key(&mut self, key: &KeyInput) -> bool {
    if key.character == Some("?") {
        if key.pressed() {
            self.help_open = !self.help_open;
        }
        return true; // the release too, so the game sees neither
    }
    false
}
```

egui's verdicts come from the previous frame. A key pressed in the same frame as the click that focuses a field still reaches the game.

### Fonts

The player owns egui's fonts. It installs them from the project's `fonts/` folder at start and again on a scene change into another project root, and each install replaces every font egui has. So don't call `Context::set_fonts` from `ui()`: the next install drops what it set.

Usually the folder is enough. Name your egui UI's faces in `fonts/fonts.toml` (ADR 0081):

```toml
[[font]]
name = "body"
file = "SourceSans3-Regular.ttf"

[[font]]
name = "strong"
file = "SourceSans3-Semibold.ttf"

[egui]
proportional = "body"     # egui's default text, so every label and button
# monospace = "..."       # likewise for egui's monospace
```

Then `ui()` draws in `body` by default and in `strong` with `FontId::new(size, FontFamily::Name("strong".into()))`. egui's own fonts stay behind each face, for the glyphs it lacks. The families exist from the first frame, so there's no frame to skip.

For fonts that don't come from the folder (embedded with `include_bytes!`, or chosen at run time), implement `fonts()`. It gets the definitions just before each install, project fonts already in, and what it adds survives every reload. A named family it adds is one scripts can draw in too.

## Picking the Ground

`Camera::screen_to_ground(x, y, width, height, plane_y)` returns the `[x, z]` point on a horizontal plane under a screen point, or `None` for the sky. Scripts have the same thing as `screen_to_ground(mouse_x(), mouse_y(), plane_y)`. See [Rendering: Picking the ground](../concepts/rendering.md#picking-the-ground).

## Projecting from Native Functions

A native function that places things on the screen needs the camera scripts project with, and `ExtensionCtx::camera` is a frame old by the time `on_draw_ui` runs. `PlayerApp::projector()` returns a `Projector`, a cloneable handle on the scripts' camera (ADR 0087). Take it before `add_extension`, hand it to the extension, and clone it into the closures it registers:

```rust
let projector = player.projector();
player.add_extension(Box::new(MyGame { projector, .. }));

// in register_script_api:
let projector = self.projector.clone();
engine.register_fn("my_labels", move || -> rhai::Array {
    let view = projector.view(); // one lock, then plain maths
    let size = view.screen_size();
    anchors().filter_map(|p| view.world_to_screen(p)) /* ... */
});
```

`View::world_to_screen` returns a `ScreenPoint { x, y, depth }` in logical points, or `None` where Rhai's `world_to_screen` says `visible: false`. The two agree exactly: called from a script, the view is the camera this frame renders with. `View::screen_to_ground` matches the Rhai function too. Outside a script call (in `pre_update`, say), the view is the previous frame's camera. `view()` takes the script context's lock, so don't call it from a callback the script system runs while holding that lock, such as a text measurer.

### Picking a ground that isn't flat

`screen_to_ground` cuts a plane, which is wrong on a hill. A game that builds its own ground registers it with the projector as a `GroundField` (ADR 0095): a `Heightmap` placed in the world, with its first sample at `origin` and its last at `origin + size`, and world y `sample × height_scale + base_y`. Both types are re-exported by `flint-player`.

```rust
let heights: Vec<f32> = /* cols × rows samples, row-major, in metres */;
let ground = GroundField::new(
    Heightmap::from_raw(heights, cols, rows),
    [0.0, 0.0],          // world x, z of the first sample
    [12000.0, 12000.0],  // to the last
    1.0,                 // height_scale
    0.0,                 // base_y
);
projector.set_ground(Some(Arc::new(ground)));
```

Then `Projector::screen_to_heightfield(x, y)` returns the world point where the ray through a screen point first comes down onto the ground, and `Projector::ground_height(x, z)` reads it; scripts have the same as `screen_to_heightfield(x, y)` and `ground_height(x, z)`. `View::screen_ray` and `View::screen_to_heightfield(x, y, &ground)` do it on a snapshot, and `GroundField::ray(origin, dir)` takes any ray, a camera rig's line to its target say.

The ray is clipped to the field's box (its x/z extent and the range of its heights), then stepped one texel at a time over the ground and the crossing step bisected, so it can start anywhere and a hit lies on the ground. A ray that comes in through the field's side below the ground meets it at the edge. One that starts under the ground is reported where it next comes down onto it from above. A ridge thinner than a texel can be stepped over. `set_ground` again replaces the field (one built off the main thread, say), and `set_ground(None)` clears it. It survives scene changes: it's the game's, not the scene's. The editor's `ray_heightmap_intersect` uses the same march.
