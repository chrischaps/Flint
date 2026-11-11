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

### What a loop loops over

A looping `audio_source` (`loop = true`) plays the whole decoded sound, first sample to last, then starts again with nothing between: no crossfade, no silence, nothing trimmed. So a loop has to be authored to close on its own length. Its last sample should lead into its first the way any two neighbouring samples do. A loop that only closes after a fade, or that carries a tail of silence or reverb, clicks or gaps at every wrap.

What "the whole sound" means is exactly the samples the encoder was given. A Vorbis encoder ends a stream on a whole block, so its last packet decodes up to a few hundred samples past the source. Flint decodes gaplessly (#632): it reads the stream's last granule position and drops that excess, as ffmpeg does, so an OGG loop is as long as the WAV it was encoded from. Before #632 that padding played (64 to 832 samples, up to 19 ms, on plain `ffmpeg -c:a libvorbis` encodes), and since ffmpeg's own decode trims it, a check made with ffmpeg didn't show the gap. WAV and FLAC have no padding. MP3 encoders pad both ends, and how much of that can be trimmed depends on the file's headers; prefer OGG or WAV for loops.

## Preloading

At scene load the player loads every file named by an `audio_source` component, then preloads every audio file it finds under `audio/` (searching the scene directory, then its parent, the game root) so that `play_sound("audio/name.ogg")` from a script never stalls on decode. Each file is cached under its path from there, extension included (`audio/whoosh.ogg`); subfolders of `audio/` are not searched. See [Sound names](#sound-names). A scene with a large stem library pays for that in load time. Opt out per scene:

```toml
[scene]
name = "Silent Corridor"
preload_audio = false
```

With `preload_audio = false` only `audio_source` files load, and [music sessions](music-sessions.md) resolve their stems through their own path. The default is `true` (ADR 0066, scene audio preload opt-out).

In the browser, and whenever a late bundle of audio arrives, nothing is decoded in one go: each sound joins a decode queue that the player works through 2 ms a frame, one packet at a time, autoplay sources above silence first and then the rest smallest first. An `audio_source` starts once its sound is decoded. A script's `play_sound` of a sound still queued decodes it on the spot if the file is 64 KB or less; a bigger one moves to the front of the queue and that play is skipped with a warning. The desktop decodes at scene load, as before.

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
| `file` | string | | The sound's file, as a path from the scene's directory or from any directory above it up to the project root (`audio/fire.ogg`); see [where the engine looks](assets.md#where-the-engine-looks-for-a-file) |
| `volume` | f32 | 1.0 | Playback volume (0.0--2.0) |
| `pitch` | f32 | 1.0 | Playback speed/pitch (0.1--4.0) |
| `loop` | bool | false | Loop the sound continuously |
| `spatial` | bool | true | 3D positioned (uses entity transform) |
| `min_distance` | f32 | 1.0 | Distance at full volume |
| `max_distance` | f32 | 25.0 | Distance at silence |
| `autoplay` | bool | true | Start playing on scene load |
| `bus` | string | `"sfx"` | Mixer bus: `"sfx"` or `"music"`. Any other name plays on `sfx` and is reported once ([Diagnostics while the game runs](schemas.md#diagnostics-while-the-game-runs)) |

> **Looping component sources must keep `autoplay = true`.** Components have no
> play/stop API — scripts can only fade their volume. If you need a sound that
> starts on cue, use a one-shot instead.

**The listener is the camera.** The player moves it to the camera's position and facing every frame, so there is no listener component. (An `audio_listener` component used to be declared and nothing read it; #792 removed it, and a scene that still writes one gets an `unknown-component` warning. Delete the table.)

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

Supported audio formats: OGG, WAV, MP3, FLAC (decoded with symphonia, gapless; see [What a loop loops over](#what-a-loop-loops-over)).

## Scripting Integration

Audio can be controlled from [Rhai scripts](scripting.md) using deferred commands. The script API produces `ScriptCommand` values that the player processes after the script update phase:

| Function | Description |
|----------|-------------|
| `play_sound(name)` | Play a non-spatial sound at default volume |
| `play_sound(name, volume)` | Play a non-spatial sound at the given volume (0.0--1.0) |
| `play_sound(name, volume, pitch)` | As above, with a pitch multiplier |
| `play_sound_at(name, x, y, z, volume)` | Play a spatial sound at a 3D position |
| `play_sound_at(name, x, y, z, volume, pitch)` | As above, with a pitch multiplier |
| `play_sound_at(name, x, y, z, opts)` | Play a spatial sound with options: `volume`, `pitch`, `min_distance`, `max_distance`, `rolloff`, each optional |
| `stop_sound(name)` | Stop a playing sound |
| `audio_position(entity)` | Seconds into the sound the entity's `audio_source` is playing, wrapped for a loop, on the mixer's clock; `-1.0` before it has started and after it ends. See [Keeping time with the music](#keeping-time-with-the-music) |
| `audio_output_latency()` | How long after it is mixed a sound is heard, seconds, where the platform says (the browser); `-1.0` where it doesn't (the desktop) |

```rust
// In a Rhai script:
fn on_interact() {
    play_sound("audio/door_open.ogg");                // Non-spatial
    play_sound_at("glass_clink", 5.0, 1.0, 3.0, 0.8); // Spatial; the bare stem finds audio/glass_clink.ogg
    play_sound_at("bell", 0.0, 30.0, 0.0,             // Heard across town
        #{ volume: 0.6, min_distance: 20.0, max_distance: 600.0, rolloff: 0.7 });
}
```

### Sound names

A sound is cached under its file's path from the folder that holds its `audio/`, extension included: `audio/door_open.ogg`. The preload reads every `audio/` the scene can see ([beside the scene, beside each directory above it up to the project root, then the engine's](assets.md#where-the-engine-looks-for-a-file)), and a nearer file hides a farther one of the same file name. That full name always works. A name that isn't cached as written tries `audio/<name>`, then `audio/<name>.ogg`, `.wav`, `.flac` and `.mp3`, in that order, so `"door_open"`, `"door_open.ogg"` and `"audio/door_open.ogg"` all play the same file. A sound cached under exactly the name given wins over the search (one an extension loaded as `"blip"` is `"blip"`), and two files that share a stem want the full path, since the search takes the first extension it finds.

A name that finds nothing plays nothing. The warning names the nearest cached sound, by file stem first:

```text
WARN script: play_sound error: AudioError("Sound not cached: whosh (nearest: \"audio/whoosh.ogg\")")
```

Extensions' `ctx.audio.play` resolves names the same way.

A placed sound is at full volume within `min_distance` and silent past `max_distance`. Between them it is `-60·r^rolloff` dB, where `r` is how far the listener is along the way from one to the other. The default `rolloff` of 2 holds a sound up most of the way and drops it near `max_distance`, which suits a small room. A sound heard across open ground wants a rolloff below 1: it falls away soon after `min_distance` and then tails off slowly, close to a real sound's inverse-distance fade. For example, `rolloff: 0.6` between 32 m and 1250 m stays within 1.5 dB of `(32/d)^1.2` out to 300 m.

Sound names match files in the `audio/` directory. All `.ogg`, `.wav`, `.mp3`, and `.flac` files are automatically loaded at startup.

## Keeping time with the music

A picture that moves with its music needs the music's own clock. Adding up `delta_time()` is not it: the sum is clamped at 250 ms a frame and paced to the display while the sound runs on the device's clock, so a hitch separates them for good, and on the web an `audio_source` whose sound is in the late bundle starts at a moment no script can see. `audio_position(entity)` asks the mixer instead: seconds into the sound that entity's `audio_source` is playing, wrapped for a loop.

```rust
let music = -1;

fn on_init() {
    music = get_entity("music");
}

fn on_draw_ui() {                      // runs paused or not
    let at = audio_position(music);    // -1.0 until the sound has started
    if at < 0.0 { return; }
    let latency = audio_output_latency();
    let heard = if latency > 0.0 { at - latency } else { at };
    let sixteenth = ((heard / 0.125).floor().to_int() + 16) % 16;  // 120 bpm
    // ... light what this sixteenth holds
}
```

It is `-1.0` until the sound has been mixed: on the web, that is once the late bundle's sound is in and the worklet has played it, so a game needs no platform check. It holds still while the sound is paused, comes back to `-1.0` when a one-shot source ends, and is read every frame whether or not the game is paused (a loop plays on under a pause menu), so `on_draw_ui` sees it move.

Where the number comes from:

- **Desktop:** kira's handle, which moves once per device callback (its position is where the callback's last 128-frame chunk began). A small clock in `flint-audio` (`clock::PlayClock`) runs it on at the sound's pitch between callbacks and eases toward each new value, so it moves every frame and never steps backwards by a jitter. After a stall it snaps back to the sound. In a simulated ten minutes it stays within one callback plus 20 ms of the sound.
- **Web:** the worklet's mixer reports each watched voice every 1024 frames (about 21 ms), stamped with the `AudioContext`'s `currentTime` at the start of the quantum it describes. The page's own `currentTime` trails those stamps, so it keeps the audio clock itself: the offset from `performance.now()` to the report that crossed fastest, following the clocks' slow drift and taken afresh only when several reports in a row say it jumped (the context slept). A busy main thread delays reports but doesn't move that clock. A report arrives only for a voice that has been mixed.

`audio_output_latency()` is the time from mixing to hearing: the browser's `baseLatency + outputLatency`. Show `audio_position(e) - audio_output_latency()` to light what is heard rather than what is being mixed. kira's desktop stream doesn't say what its device adds, so there it is `-1.0` and a game keeps its own estimate.

`examples/audio_clock.scene.toml` lights four pads from a looping click track: Space stalls the frame for 500 ms and E pauses, and the pads stay on the clicks while the summed `delta_time()` shown above them falls behind.

## From Rust: Player Extensions

A game that wraps the player in its own binary reaches the same engine from Rust through `ExtensionCtx::audio` (ADR 0084). It plays sounds with no script in between, chooses each placed sound's falloff, sets the bus volumes, and loads sounds it makes in memory (samples, or encoded bytes) rather than from files under `audio/`. A sound loaded there can be played by scripts and `audio_source`s by its name. See [Player Extensions: Sound](../guides/player-extensions.md#sound).

## Further Reading

- [Scripting](scripting.md) --- full scripting API including audio functions
- [Animation](animation.md) --- animation system that can trigger audio events
- [Physics and Runtime](physics-and-runtime.md) --- the game loop and event bus that drives audio triggers
- [Schemas](schemas.md) --- component and archetype definitions
