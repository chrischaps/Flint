# Audio

Flint's audio system provides spatial 3D sound via the `flint-audio` crate, built on [Kira](https://github.com/tesselode/kira) 0.11. Sounds can be positioned in 3D space with distance attenuation, played as ambient loops, or triggered by game events like collisions.

## Spatial Audio

Spatial sounds are attached to entities via the `audio_source` component. The sound's volume attenuates with distance from the listener (the player camera):

- **min_distance** --- full volume within this radius
- **max_distance** --- silence beyond this radius
- Volume falls off smoothly between the two

The listener position and orientation are updated each frame to match the first-person camera, so sounds pan and attenuate as you move through the scene.

## Ambient Loops

Non-spatial sounds play on the main audio track at constant volume regardless of listener position. Set `spatial = false` on an `audio_source` to use this mode --- useful for background music, ambient atmosphere, and UI sounds.

## Preloading

At scene load the player loads every file named by an `audio_source` component, then preloads every audio file it finds under `audio/` (searching the scene directory, then its parent, the game root) so that `play_sound("name")` from a script never stalls on decode. A scene with a large stem library pays for that in load time. Opt out per scene:

```toml
[scene]
name = "Silent Corridor"
preload_audio = false
```

With `preload_audio = false` only `audio_source` files load, and [music sessions](music-sessions.md) resolve their stems through their own path. The default is `true` (ADR 0066, scene audio preload opt-out).

## Mixer Buses

Every ordinary sound routes through one of two buses — `music` and `sfx` — both children
of Kira's main track. (A running [music session](music-sessions.md) adds its own six-bus stem mixer on the same device; that mixer is owned by `flint-music`, not by `audio_source`, and is described on its own page.)

```
main  ──┬── music     (audio_source.bus = "music")
        └── sfx       (everything else, including all one-shots)
```

Opt a source into the music bus in the scene:

```toml
[entities.score.audio_source]
file = "audio/theme_night.ogg"
bus = "music"
loop = true
autoplay = true
```

Initial gains come from the CLI:

```bash
flint play scene.toml --music-volume 0.8 --sfx-volume 1.0
flint play scene.toml --music-volume 0     # mute the score, keep the world
```

**Bus gain multiplies underneath per-sound volume.** A script crossfading
between two music beds keeps working at any bus gain, and a player who has
turned the music down does not have their fades overridden.

The master low-pass applies to both buses, so `set_audio_lowpass` muffles the
entire mix — score included — which is what you want when the effect is
"something is between you and the world" rather than "the sfx got quieter".

One-shots (`play_sound`, `play_sound_at`) are always sfx.

## Music Sessions

A scene that carries a `music_session` component turns the audio engine into the host for a rhythm-driven chart: `flint-music` opens a `ChartSession` on the **same** Kira manager the ordinary buses use (ADR 0017, shared audio manager), so the world's sounds and the suite's stems mix together and one master low-pass muffles both. For the length of the session the gamepad is handed to a 1 kHz capture thread that stamps every stick and button event with the audio clock (ADR 0018, gamepad handoff).

> **While a session is active, only the lean stick and the pulse button reach `InputState`.** Every other gamepad control is consumed by the capture thread; keyboard input still flows through winit as normal. Scripts that need the pad for anything else during a chart should read the [conducted parameters](scripting.md#conducted-parameters-api) instead.

The component, its configuration files, the ladder and reintegration mechanics, and the seven `flint` subcommands that go with it are on the [Music Sessions](music-sessions.md) page.

## Audio Schemas

Three component schemas define audio behavior:

**audio_source** (`audio_source.toml`) --- a sound attached to an entity:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `file` | string | | Path to audio file (relative to scene directory) |
| `volume` | f32 | 1.0 | Playback volume (0.0--2.0) |
| `pitch` | f32 | 1.0 | Playback speed/pitch (0.1--4.0) |
| `loop` | bool | false | Loop the sound continuously |
| `spatial` | bool | true | 3D positioned (uses entity transform) |
| `min_distance` | f32 | 1.0 | Distance at full volume |
| `max_distance` | f32 | 25.0 | Distance at silence |
| `autoplay` | bool | true | Start playing on scene load |
| `bus` | string | `"sfx"` | Mixer bus: `"sfx"` or `"music"` |

> **Looping component sources must keep `autoplay = true`.** Components have no
> play/stop API — scripts can only fade their volume. If you need a sound that
> starts on cue, use a one-shot instead.

**audio_listener** (`audio_listener.toml`) --- marks which entity receives audio:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `active` | bool | true | Whether this listener is active |

**audio_trigger** (`audio_trigger.toml`) --- event-driven sounds:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `on_collision` | string | | Sound to play on collision start |
| `on_interact` | string | | Sound to play on player interaction |
| `on_enter` | string | | Sound when entering a trigger volume |
| `on_exit` | string | | Sound when exiting a trigger volume |

## Dynamic Parameter Sync

Audio source parameters (`volume` and `pitch`) can be changed at runtime via `set_field()` and the engine automatically syncs changes to the playing audio each frame. This enables dynamic audio effects like engine RPM simulation or distance-based volume curves:

```rust
// Adjust engine sound pitch based on speed
let rpm_ratio = speed / max_speed;
set_field(engine_sound, "audio_source", "pitch", 0.5 + rpm_ratio * 1.5);
set_field(engine_sound, "audio_source", "volume", 0.3 + rpm_ratio * 0.7);
```

Changes are applied with a 16ms tween for smooth transitions (no clicks or pops).

## Scene Transition Behavior

When a scene transition occurs (via `load_scene()` or `reload_scene()`), all playing sounds are explicitly stopped with a short fade-out before the old scene is unloaded. This prevents audio bleed between scenes --- sounds from the previous scene won't continue playing into the new one.

## Architecture

The audio system has three main components:

- **AudioEngine** --- wraps Kira's `AudioManager`, handles sound file loading, listener positioning, and spatial track creation. Sounds route through spatial tracks for 3D positioning, or through a mixer bus for non-positional playback.
- **AudioSync** --- bridges TOML `audio_source` components to Kira spatial tracks. Discovers new audio entities each frame and updates spatial positions from entity transforms.
- **AudioTrigger** --- maps game events (collisions, interactions) to `AudioCommand`s that play sounds at specific positions.

The system implements the `RuntimeSystem` trait, ticking in the `update()` phase of the game loop (not `fixed_update()`, since audio doesn't need fixed-timestep processing).

## Graceful Degradation

`AudioManager::new()` can fail on headless machines or CI environments without an audio device. The engine wraps the manager in `Option` and silently skips audio operations when unavailable. This means scenes with audio components work correctly in all environments --- you just won't hear anything.

## Adding Audio to a Scene

```toml
# A crackling fire with spatial falloff
[entities.fireplace]
archetype = "furniture"

[entities.fireplace.transform]
position = [5.0, 0.5, 3.0]

[entities.fireplace.audio_source]
file = "audio/fire_crackle.ogg"
volume = 0.8
loop = true
spatial = true
min_distance = 1.0
max_distance = 15.0

# Background tavern ambience (non-spatial)
[entities.ambience]

[entities.ambience.audio_source]
file = "audio/tavern_ambient.ogg"
volume = 0.3
loop = true
spatial = false
```

Supported audio formats: OGG, WAV, MP3, FLAC (via Kira's symphonia backend).

## Scripting Integration

Audio can be controlled from [Rhai scripts](scripting.md) using deferred commands. The script API produces `ScriptCommand` values that the player processes after the script update phase:

| Function | Description |
|----------|-------------|
| `play_sound(name)` | Play a non-spatial sound at default volume |
| `play_sound(name, volume)` | Play a non-spatial sound at the given volume (0.0--1.0) |
| `play_sound_at(name, x, y, z, volume)` | Play a spatial sound at a 3D position |
| `stop_sound(name)` | Stop a playing sound |

```rust
// In a Rhai script:
fn on_interact() {
    play_sound("door_open");                          // Non-spatial
    play_sound_at("glass_clink", 5.0, 1.0, 3.0, 0.8); // Spatial at position
}
```

Sound names match files in the `audio/` directory. All `.ogg`, `.wav`, `.mp3`, and `.flac` files are automatically loaded at startup.

## Further Reading

- [Scripting](scripting.md) --- full scripting API including audio functions
- [Animation](animation.md) --- animation system that can trigger audio events
- [Physics and Runtime](physics-and-runtime.md) --- the game loop and event bus that drives audio triggers
- [Schemas](schemas.md) --- component and archetype definitions
