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
    // The scene's [camera], [environment], [post_process], [living] and its
    // tables (what ctx.scene_table reads), as flint-player takes them
    player.apply_scene_settings(&scene);
    player.set_schema_paths(dirs);
    player.add_extension(Box::new(MyGame));

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut player)?;
    Ok(())
}
```

`PlayerApp` is a winit `ApplicationHandler`, so it runs directly with no wrapper. Set the same launch options `flint-player`'s `main.rs` sets for the features your scenes use, on `player.launch` (`window_size`, `msaa_sample_count`, `style_specs`, ...), and `capture` for `--screenshot`-style runs. What the scene's file says is `player.scene` (`SceneSettings`), which `apply_scene_settings` fills in. Pass the engine's `schemas/` yourself, ahead of the game's. The game itself, with no window, is `player.core` (a `GameCore`, [ADR 0117](https://github.com/chrischaps/Flint/blob/main/docs/adr/0117-player-subsystems-and-one-game-step.md)): the world, its clock, scripts, physics, animation and particles are `player.core.world`, `player.core.clock`, `player.core.script` and so on, and each frame runs the core's one frame order, the one `flint render --scripts` runs too.

### What the player links

`flint-player`'s optional parts are Cargo features, all on by default, so a game that names the crate with no `features` key gets everything `flint play` has. A game that needs less, the web build of a small game say, turns the defaults off and names what its scenes use:

```toml
[dependencies]
flint-player = { path = "engine/crates/flint-player", default-features = false, features = ["audio", "gamepad", "music"] }
```

| Feature | What it brings | Without it |
|---|---|---|
| `audio` | The sound card (kira's cpal backend) | The game runs silent |
| `gamepad` | Pads and `rumble()` (gilrs) | No pad is seen; keyboard, mouse and touch still work |
| `debug-hud` | The F3/F4 debug panels (`flint-debug-ui`) | No debug surface |
| `music` | A `music_session`'s suite and `music_beat()` (`flint-music`) | The suite doesn't play and the beat reads -1 |
| `rhythm` | A charted suite judged (`flint-rhythm`, the 1 kHz pad capture); turns on `music` | A charted suite plays as music |
| `procgen` | Assets made from `*.procgen.toml` specs, and `register_generators` (`flint-procgen`) | Every model and texture name is a file's |
| `living` | Living props, `[living]`, `[wooze]`, shallow wooze, woozy grass and the `wooze`, `living` and `debug` script modules; turns on `procgen` | The grove stands still |
| `profile` | Tracy's frame profiler (off by default) | The scopes compile to nothing |

A scene that uses a part the build left out prints one line when it's entered, naming what won't run, for example ``[living] this player is built without the `living` feature: the scene's living props, [wooze] stand still``. With every default off the player links neither `flint-music` nor `flint-procgen`.

## The Hooks

| Method | When it runs | Use it for |
|---|---|---|
| `register_script_api(&mut rhai::Engine)` | Once, inside `add_extension` | Native functions and types for scripts |
| `declare_script_api(&mut ScriptApi)` | Once, inside `add_extension`, after `register_script_api` | Functions declared through Flint's API builder: listed with the engine's, integers taken for numbers |
| `register_generators(&mut GeneratorRegistry)` | Once, inside `add_extension` | The game's own [procgen generators](#procgen-generators) ([ADR 0119](https://github.com/chrischaps/Flint/blob/main/docs/adr/0119-game-generators-through-player-extensions.md)) |
| `pre_update(&mut ExtensionCtx)` | Every frame, before the scripts' `on_update` | Writing the world, firing events, uploading meshes |
| `on_window_event(&WindowEvent) -> bool` | Every window event that egui didn't take | Game input the scene's input config can't express |
| `on_key(&KeyInput) -> bool` | Every key egui didn't take, real or from `--input-script` | Keys a capture should be able to press |
| `ui(&egui::Context)` | Every frame, inside the player's egui pass | The game's own panels and windows |
| `fonts(&mut egui::FontDefinitions)` | Whenever the player installs egui's fonts | Fonts the project's `fonts/` folder can't give |
| `debug_panels() -> Vec<String>` | When the player builds its debug panels: each scene load, and at `add_extension` | Naming the game's own [debug panels](debug-panels.md#adding-a-panel-from-a-game) ([ADR 0107](https://github.com/chrischaps/Flint/blob/main/docs/adr/0107-debug-panels-from-the-game.md)) |
| `debug_panel_ui(name, &mut egui::Ui)` | Every frame one of them is open, after `ui` | Drawing that panel's contents from the game's state |

Extensions run in the order they were added.

### Script functions

`register_script_api` gets the player's Rhai engine. Flint's own API is already registered, so a function with the same name and parameter types replaces Flint's. The engine outlives scene transitions, so registrations last the whole run.

The engine uses rhai's `sync` feature, so a registered closure and everything it captures must be `Send + Sync`. Share state between the extension and its functions with `Arc<Mutex<_>>` or atomics.

### Declaring functions

`declare_script_api` declares functions through the builder Flint's own API uses ([ADR 0116](https://github.com/chrischaps/Flint/blob/main/docs/adr/0116-declared-script-api.md)). Each gets a module, a one-line doc and a kind for each parameter, which converts the script's value before your code runs: `Num` takes an int or a float, `Entity` a live entity (given `-1` or a despawned id it warns once and returns the empty value), `Str`, `Map`, `Array`, `Color` (`[r, g, b]` or `"#rrggbb"`), `Any` and more (`flint_script::api::builder::kind`). The body reaches the world through `call.world()` / `call.world_mut()`, which are a script error, not a panic, when there is no world:

```rust
fn declare_script_api(&mut self, api: &mut flint_script::api::ScriptApi) {
    use flint_script::api::builder::kind::{Entity, Num};
    let mut m = api.module("kart");
    m.func("boost", "Push the kart forward by this many m/s.")
        .arg("kart", Entity)
        .arg("speed", Num)
        .run(|call, (kart, speed)| {
            let world = call.world_mut()?;
            // ... write the kart's velocity
            Ok(())
        });
}
```

A declared function is listed with its parameters and doc by `app.core.script.api_listing()`, the data behind `flint script api`; a game binary prints its own listing from it. Functions from `register_script_api` keep working and are listed with their types only.

### Procgen generators

A game can bring generators the engine doesn't ship: a `rock_v1` for a cave game, a `rune_v1` for its signage. Write one as `crates/flint-procgen/CONTRIBUTING.md` describes (a type implementing `flint_procgen::Generator`), then hand it to the player from `register_generators`:

```rust
use flint_procgen::GeneratorRegistry;

impl PlayerExtension for CaveGame {
    fn register_generators(&mut self, registry: &mut GeneratorRegistry) {
        registry.register(Box::new(rock::RockGenerator));
    }
}
```

The registry already holds the built-ins, and it's the one the player resolves every scene's `*.procgen.toml` specs through. A spec in the game's `specs/` with `generator = "rock_v1"` then resolves like a tree: generated as the scene loads (or queued, under `--procgen-defer`), cached in memory and on the disk, and uploaded before the model loader looks for a file of that name. A generator registered under a built-in's name replaces the built-in.

Add the extension before the event loop starts. The first scene's specs are resolved as it loads, and a spec resolved before its generator was registered has already fallen back to a file or a placeholder. Only the game's binary knows these generators. The stock `flint-player` shows a placeholder for such a spec, and `flint gen`, `flint edit` and `flint render` build their registries from the built-ins alone.

A complete game binary with one is in the engine: `cargo run -p flint-player --example cave_game` (`crates/flint-player/examples/cave_game/`).

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

### Reading components

Components are TOML tables, so reading one by hand is a chain of `get`, a type test and a default, and each link fails in silence: `as_float` reads the integer `8` as nothing, and a misspelled option falls through to whatever the `match` ends with. Read through a **view** instead ([ADR 0115](https://github.com/chrischaps/Flint/blob/main/docs/adr/0115-scene-diagnostics-and-complete-schemas.md)), as the engine's physics and audio do:

```rust
use flint_core::components as comp;

let c = ctx.world.view(id, comp::COLLIDER);
let friction = c.f32("friction", 0.5);                           // 0.5, 1 and 1.0 are all numbers
let shape = c.choice("shape", &["box", "sphere", "capsule"], "box");
let offset = c.vec3("offset", [0.0; 3]);
let label = c.opt_str("label");                                  // None when not written

// A game's own component works the same way; the name is any string
let hp = ctx.world.view(id, "health").i64("max", 100);
```

| Reader | Takes | With no default |
|--------|-------|-----------------|
| `f32(field, default)`, `f64` | an integer or a float | `opt_f32`, `opt_f64` |
| `i64(field, default)` | an integer, or a float with no fraction (`3.0`) | `opt_i64` |
| `bool(field, default)` | `true` or `false` | `opt_bool` |
| `str(field, default)` | a string | `opt_str` |
| `choice(field, &allowed, default)` | one of `allowed`; returns the entry from that list | `opt_choice` |
| `vec3(field, default)` | `[x, y, z]`, or a table with any of `x`, `y`, `z` | `opt_vec3` |
| `vec4(field, default)` | `[a, b, c, d]` | `opt_vec4` |
| `color(field, default)` | `[r, g, b]` (alpha 1) or `[r, g, b, a]` | `opt_color` |

A field that isn't written reads as the default, and so does every field of a component the entity doesn't have (`exists()` says which); neither is reported. A value of the wrong type, or a string that is none of a `choice`'s values, reads as the default too, and is **reported once** for that entity, component and field: a warning in the log with the entity's name, what was expected and, for a choice, the allowed values and the nearest one.

```
WARN error[bad-enum-value] entity 'crate' collider.shape: "spere" is not a value of `shape` (did you mean `sphere`? allowed values: box, sphere, capsule; the default "box" is used)
```

A read that finds nothing wrong takes no lock and allocates nothing, so reading every frame is fine. `ctx.world.read_reports().snapshot()` returns what has been reported so far as `Diagnostic`s (a debug panel can list them, and a test can assert on them); `view.report(field, || Diagnostic::warning(..))` adds a finding of your own to the same once-only list; and `raw(field)` hands back the `toml::Value` for a shape no reader covers. For a table that isn't a component, `flint_core::toml_util` (`toml_f32`, `toml_f64`, `toml_vec3`, `toml_color`) coerces integers without reporting.

### Reading the scene's tables

A game gives a scene settings of its own as a top-level table, and reads it here. Declare the table in the game's schema layer, so `flint validate` checks every scene that writes it ([Schemas: A table of your own](../concepts/schemas.md#a-table-of-your-own)):

```toml
# schemas/scene/weather.toml
[table.weather.fields]
rain = { type = "f32", min = 0.0, max = 1.0, default = 0.0 }
wind = "vec3"
```

```toml
# scenes/storm.scene.toml
[weather]
rain = 0.7
wind = [3.0, 0.0, -1.0]
```

`ctx.scene_table(name)` is the current scene's table as its file wrote it, and `ctx.scene_table_as::<T>(name)` reads it into a type of the game's:

```rust
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Weather {
    rain: f32,
    wind: [f32; 3],
}

impl PlayerExtension for MyGame {
    fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
        // None: this scene has no [weather]
        let weather: Weather = match ctx.scene_table_as("weather") {
            Some(Ok(weather)) => weather,
            Some(Err(e)) => { eprintln!("[weather]: {e}"); Weather::default() }
            None => Weather::default(),
        };
        self.rain.set_rate(weather.rain);
    }
}
```

The values are the file's own: a `default` the schema states is not filled in, so give the type its defaults (`#[serde(default)]`), or read a key with a fallback. `ctx.scene_tables` holds every top-level table of the scene, the engine's `[camera]` and `[post_process]` included. A scene transition replaces them, so read each frame, or keep the last scene's and compare, not once at start. A host that builds `PlayerApp` itself gets them from `apply_scene_settings`, or sets `player.scene_tables = scene_file.scene_tables()`.

The table reaches the extension whether or not a schema declares it. Without one it is an `unknown-table` warning on every load, and a typo in it is not caught.

### Saved data

`ctx.persist` is the same `PersistentStore` that scripts reach with `persist_get` and `persist_set`, the one the player keeps in the game's save file (see [Scripting: Persistence](../concepts/scripting.md#persistence) and [Where saves live](../concepts/scripting.md#where-saves-live)). It's loaded before any script's `on_init`, so the first `pre_update` already sees last session's values.

```rust
// toml_f64 takes `music_volume = 1` as well as `1.0`; `as_float` would not
let volume = ctx
    .persist
    .get("music_volume")
    .and_then(flint_core::toml_util::toml_f64)
    .unwrap_or(0.8);
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

`upload_mesh_with(name, vertices, indices, material, MeshOptions { ray_target: true })` uploads a mesh that rays meet too ([ADR 0105](https://github.com/chrischaps/Flint/blob/main/docs/adr/0105-rays-against-drawn-meshes.md)): wherever an entity draws it, `ctx.rays` and scripts' `raycast_meshes` and `screen_to_mesh` find its triangles. `ctx.rays` is the scene as the last frame drew it:

```rust
gpu.upload_mesh_with("houses", &vertices, &indices, material, MeshOptions { ray_target: true });
// ... a frame later, or in a script:
if let Some(hit) = ctx.rays.segment(head, camera_wants) {
    // a wall `hit.distance` from the head; come in to it
}
```

An upload without the option, or `remove_mesh`, stops it being a target. The triangles are kept on the CPU under a bounding volume hierarchy, built at each upload, so make targets of what rays need to meet. Native functions reach the same scene through `Projector::ray_scene()`, and `Projector::screen_to_mesh(x, y)` picks under a screen point.

`set_ground_fog(&GroundFog { origin, size, width, depth, texels })` lays a fog on floors of the game's own ([ADR 0106](https://github.com/chrischaps/Flint/blob/main/docs/adr/0106-ground-fog-on-own-floors.md)): each texel a `[floor, amount]`, so a mist can fill valleys at many heights. Its look is the post-process config's `ground_fog_*`, so set the map when the ground changes and the density and colour by the hour. See [Post-processing: Ground fog](../concepts/post-processing.md#ground-fog).

`renderer.set_gpu_timing(device, queue, true)` times the frame's phases on the GPU ([ADR 0108](https://github.com/chrischaps/Flint/blob/main/docs/adr/0108-gpu-timing-from-timestamp-queries.md)), and `renderer.collect_stats()` then carries each phase's milliseconds in `gpu_passes` and the frame's in `gpu_frame_ms`, for a game's own frame graph. See [Profiling](profiling.md).

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

A key then goes to the same extension's `on_key`, unless its `on_window_event` took it. `KeyInput` holds the key's `KeyCode`, whether it went down or up, whether the OS is repeating it, the character it stands for (winit's `logical_key`, on the release too), the modifiers held, and whether `--input-script` pressed it. `pressed()` is a first press. A scripted key comes **only** to `on_key` ([ADR 0103](https://github.com/chrischaps/Flint/blob/main/docs/adr/0103-extensions-see-scripted-keys.md)): winit's `KeyEvent` can't be built outside winit, so there is no `KeyboardInput` to show `on_window_event`. Read keys in `on_key`, then, and a capture can press them; handle each key in one hook or the other, not both. Keys winit can't name, and the synthetic presses it sends for keys held as the window gains focus, come only to `on_window_event`.

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

Usually the folder is enough. Name your egui UI's faces in `fonts/fonts.toml` ([ADR 0081](https://github.com/chrischaps/Flint/blob/main/docs/adr/0081-project-fonts-name-egui-defaults.md)):

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

A native function that places things on the screen needs the camera scripts project with, and `ExtensionCtx::camera` is a frame old by the time `on_draw_ui` runs. `PlayerApp::projector()` returns a `Projector`, a cloneable handle on the scripts' camera ([ADR 0087](https://github.com/chrischaps/Flint/blob/main/docs/adr/0087-native-code-projects-with-frame-camera.md)). Take it before `add_extension`, hand it to the extension, and clone it into the closures it registers:

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

`screen_to_ground` cuts a plane, which is wrong on a hill. A game that builds its own ground registers it with the projector as a `GroundField` ([ADR 0095](https://github.com/chrischaps/Flint/blob/main/docs/adr/0095-ray-against-placed-height-field.md)): a `Heightmap` placed in the world, with its first sample at `origin` and its last at `origin + size`, and world y `sample × height_scale + base_y`. Both types are re-exported by `flint-player`.

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
