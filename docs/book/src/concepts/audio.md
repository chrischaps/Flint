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

- **AudioEngine** --- wraps Kira's `AudioManager`, handles sound file loading, listener positioning, and spatial track creation. Sounds route through spatial tracks for 3D positioning or the main track for ambient playback.
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
