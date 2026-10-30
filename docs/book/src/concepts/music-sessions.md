# Music Sessions

A music session turns a scene into a rhythm-driven space: a fixed set of stems plays sample-locked on the audio clock, a beatmap chart says what the player's controller should be doing on each beat, and the world's audio and visuals come apart or re-gather as one thing depending on how well the two agree. The machinery lives in two engine crates, `flint-music` and `flint-input-capture`, and is switched on per scene by a `music_session` component. It grew out of Starchild but nothing in it is game-specific: any scene with a suite manifest and a chart can run one.

The design rule throughout is "linear composition, adaptive playback": the music is authored once as a suite, and the engine adapts *how it is played back*, never what it is.

## The Crates

**`flint-music`** is the data-contract and judgment layer. It parses suite manifests and charts, validates them against each other and against the stem files, keeps musical time (tempo map, conductor), judges input against the chart, integrates the result into a single coherence value, drives the six-bus stem mixer through the disintegration ladder, sequences reintegration after a full fail, records and replays sessions, and renders a scripted session offline to WAV. It never touches a gamepad.

**`flint-input-capture`** owns the gamepad on a dedicated thread polling at 1 kHz (default), far above frame rate, so pulse timing resolves at millisecond granularity instead of once per frame. Every event is stamped with a *compensated suite sample*: the bridged audio-clock sample minus the total judgment offset (measured output latency plus tap calibration), so the event carries the musical moment the player was responding to. On Windows the crate uses gilrs on the XInput backend rather than the default (ADR 0011, gilrs XInput backend), because the default backend delivers nothing to console applications.

Neither crate is optional at build time. The player links both; without an audio device or a gamepad the session simply degrades, as described below.

## Verb Space

Charts never see buttons. The capture thread maps physical controls onto a small **verb space**, and the chart is written in those verbs:

| Verb | Kind | Range | Meaning |
|------|------|-------|---------|
| `lean` | continuous, vec2 | [-1, 1] | Left stick. The primary tracking channel. |
| `sway` | continuous, vec2 | [-1, 1] | Right stick. |
| `pressure_l` / `pressure_r` | continuous, scalar | [0, 1] | Trigger depth. |
| `pulse` | discrete | | The one plain beat hit (South button). |
| `press` | discrete | | Rising onset of a trigger squeeze; depth is judged from the pressure stream, never carried on the event. |
| `flick` | discrete, with direction | | Right stick goes from quiet to hard deflection within a beat-scale instant. |

Which of these the pad produces is a **verb map** (ADR 0030, full verb map capture):

- `prototype` (default): left stick is `lean`; South button *or* right trigger is `pulse`. Byte-identical to the earliest builds.
- `full`: left stick is `lean`, South is `pulse`, triggers become `pressure_l`/`pressure_r` streams with `press` onsets, right stick becomes `sway` plus the flick detector. The right trigger no longer emits a plain pulse.

Select it with `input_map` on the component or `--input-map` on the CLI harnesses.

## Suite Manifest and Chart

A suite is two TOML files, both carrying `schema_version = 0`.

The **manifest** (`*.suite.toml`) describes the music:

```toml
schema_version = 0

[suite]
id = "prologue"
title = "Prologue"

[audio]
sample_rate = 48000

[[tempo]]
sample = 0
bpm = 96.0
time_signature = [4, 4]

[[sections]]
name = "intro"
start_sample = 0
pulse_window_ms = 90.0

[reintegration]
re_entry_sections = ["intro", "verse"]
lead_bus = "home_theme"
reassembly_bars = 4

[buses.foundation]
file = "stems/foundation.wav"
[buses.harmony]
file = "stems/harmony.wav"
[buses.texture]
silent = true
```

The bus set is fixed at six: `foundation`, `harmony`, `world_voice`, `home_theme`, `child_motif`, `texture`. `home_theme` and `child_motif` are the *motif* buses and must stay isolated (never share a file with another bus). Optional `[[degraded_alternates]]` entries name a pre-composed degraded take for a bus over a sample range (ADR 0032, degraded alternate playback).

The **chart** (`*.chart.toml`) says what the player should do, in beats:

- `[[curves]]` keys: `channel`, `beat`, `value` (one or two numbers), `interp` in `linear` | `hold` | `smooth`.
- `[[pulses]]`: `beat`, `kind` in `pulse` | `press` | `flick`, optional `window_ms`, `strength`, `direction`.
- `[[cues]]`: `beat`, `cue` name, optional `params` table. Cues reach scripts through `conducted_cues()` (ADR 0033, cue params and conducted cues).
- `[[intensity]]` keys: `beat`, `value`.

Both parsers are shape-tolerant on purpose. Unknown bus names, channels or kinds parse fine and are reported by `flint validate-suite` as coded issues; only structurally unreadable input fails to parse.

## The `music_session` Component

Add the component to any entity in the scene. Its schema file lives in the *game's* `schemas/components/` directory, not in the engine's, so the engine reads it by name and validates the fields itself:

```toml
[entities.conductor.music_session]
manifest = "music/prologue.suite.toml"
chart = "music/prologue.chart.toml"
lean_mode = "arrival"
input_map = "full"
ladder_config = "config/ladder.toml"
bars = 64
quit_on_finish = false
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `manifest` | string | required | Suite manifest, relative to the game root |
| `chart` | string | required | Beatmap chart |
| `lean_mode` | string | `"arrival"` | `arrival` judges gross motion toward beat-anchored targets (ADR 0013, arrival lean mode); `track` judges the stick against the curve on a fine grid |
| `input_map` | string | `"prototype"` | Verb map, `prototype` or `full` |
| `coherence_config` | string | `config/coherence.toml` if present | Explicit path must load; the default is optional |
| `ladder_config` | string | `config/ladder.toml` if present | Same contract |
| `gradient_config` | string | `config/gradient.toml` if present | Same contract; absent means inert |
| `haptics_config` | string | `config/haptics.toml` if present | Same contract; absent never emits an event |
| `bars` | integer | play the suite out | Stop after this many bars |
| `quit_on_finish` | bool | `false` | Exit the player when the session finishes (ADR 0036) |
| `tuning_config` | string | | Parsed and logged, not yet read |
| `bindings` | string | | Declarative only. The script named here must be loaded through an ordinary `script` component; the session warns if the file does not exist |

Paths are resolved against the game root (the scene's `base_dir`), the same place `scripts/` and `audio/` live.

## Lifecycle

1. **Shared audio manager.** The session opens on the player's existing Kira manager rather than its own (ADR 0017, shared audio manager). Stems and `audio_source` sounds share one device, one clock. If there is no audio device the session is skipped with a console notice and everything else in the scene runs.
2. **Timing offsets.** Measured output latency and tap calibration are read from the newest files under `logs/latency/` in the game root. Missing values are announced loudly on the console: run the latency harness for the former and `flint calibrate` for the latter.
3. **Gamepad handoff.** The capture thread takes the pad for the session's duration (ADR 0018, gilrs handoff and InputState downsample). The same event stream is down-sampled into the ordinary `InputState` so `is_action_pressed` and friends keep working frame-quantised, with a pulse press released on the *next* tick so edge detection sees a full down/up pair.
4. **Teardown** fades the stems over 50 ms and restores the scene's authored post-processing values.

> **While a session is active only `lean` and `pulse` reach `InputState`.** Every other pad control is dropped inside the capture crate. Keyboard input stays on winit throughout. This is an accepted gap of ADR 0018; design your session scenes so that nothing else needs the pad.

The session ticks once per frame from the player's frame loop. Each tick drains capture events, judges them, advances coherence, observes the ladder, runs the reintegration sequencer, applies the resolved mixer state, and publishes a `ConductedSnapshot` for scripts.

## Coherence

Everything downstream sees one number in [0, 1] (ADR 0010, coherence model). It is a leaky integrator with asymmetric, bar-denominated time constants: the continuous tracking signal sets a per-step target and the value eases toward it, rising with `rise_bars` and falling with `fall_bars`. Judged pulses enter as bounded impulses: a hit nudges up by how clean it was, a miss down by `miss_penalty`, a spurious pulse by `spurious_penalty` (default 0, because this is flow, not evaluation). The `sway` and pressure channels have weights that default to 0, so an unmodified config produces bit-identical values on any chart. All of it is plain `f64` arithmetic in a fixed order: same records in, same value out. Every knob is in `config/coherence.toml` and reloadable mid-session.

## The Ladder

The disintegration ladder (ADR 0015, disintegration ladder config) is ordered rungs, each with a coherence threshold and a full description of the degraded state. It is data, `config/ladder.toml`, hot-reloadable:

```toml
schema_version = 0
arm_above = 0.8

[[rungs]]
name = "haze"
enter_below = 0.6
exit_above = 0.7
ramp_ms = 350.0
[rungs.audio]
lpf_hz = 4000.0
thin_db = { texture = -6.0 }
[rungs.visual]
desaturate = 0.3

[[rungs]]
name = "dropout"
enter_below = 0.3
exit_above = 0.45
[rungs.audio]
drop = ["texture"]
warble_depth_semitones = 0.3
warble_rate_hz = 1.5
[rungs.visual]
chromatic = 0.5
blur = 0.4

[full_fail]
enter_below = 0.15
exit_above = 0.25
hold_ms = 1500.0

[seam]
fade_ms = 30.0
rewind_beats = 4.0
rewind_drop_semitones = -30.0
pickup_beats = 2.0
lead_in_beats = 0.0
```

Rules the ladder keeps:

- **Rung parameters are absolute.** A deeper rung states the whole degraded state; it does not stack on the rung above.
- **Hysteresis is in the thresholds.** `enter_below` is lower than `exit_above`, so a noisy value at a boundary never flickers the world.
- **Protected buses.** Only `texture`, `world_voice` and `harmony` can be thinned or dropped. `foundation` and the two motif buses are never touched by gain or dropout. The low-pass applies to every non-motif bus, foundation included: filtering the whole world is the woozy intent, silencing its pulse is not.
- **Arming.** The ladder arms only once coherence first reaches `arm_above`; the world can only come apart after it has first cohered.
- **One writer.** The resolved `LadderParams` for the current rung is the single source of truth. The audio half is applied as idempotent Kira tweens on the mixer; the visual half rides on the frame to the post-processing stack.

## Full Fail and Reintegration

Below `full_fail.enter_below`, held for `hold_ms`, the reintegration sequencer takes over (ADR 0014, reintegration seam mechanism). The state machine is `Playing → Failing → Reassembling → Playing`:

1. **Rewind.** For `seam.rewind_beats` the whole world spins down like a record played backwards, a playback-rate ramp on every stem landing at `rewind_drop_semitones`, mirrored visually. The gesture is measured in beats so it starts on a beat and ends exactly at the seam.
2. **Pickup.** In the last `pickup_beats` before the seam the lead bus plays the re-entry material winding up from the spin-down rate to full speed, arriving on the re-entry downbeat.
3. **Seam.** On the next reachable bar line the old timeline fades out over `fade_ms` (an envelope, not a cut) and every stem re-plays sample-locked from the previous re-entry section. The lead motif bus enters at full level; the rest enter at -60 dB. With `lead_in_beats` above 0 (validated 0..8, default 0) the ensemble re-enters that many beats *before* the checkpoint downbeat, so the player gets a "3, 4, go" of prep time.
4. **Reassembly.** Over the manifest's `reassembly_bars` the entering buses ramp in and the ladder runs in reverse, lerping from the deepest rung back to clean, so the world re-gathers as one thing.

Coherence is not reset. A player still absent after reassembly fails again, which is the designed loop; only the debounce restarts. Input is never interrupted and the judge is rewound at the seam.

## Audio Gradient and Haptics

Two optional drivers sit beside the ladder, both pure evaluators that feed the same single mixer writer:

- **Gradient** (ADR 0024, error-driven audio gradient; `config/gradient.toml`). *Tune*: lean error drives the depth of a zero-mean pitch LFO on one degradable bus, so off the lean the voice wavers and on it the voice settles. *Sink*: at stick-neutral the mix thins by per-bus gain trims. Scripts are read-only toward audio by design; the gradient never goes through Rhai.
- **Haptics** (ADR 0026, haptic entrainment architecture; `config/haptics.toml`). Pre-beat tick, pulse-landing thump, rewind grind, pickup ticks. The driver is event-shaped: it never sees coherence or lean error, because a buzz-when-wrong reads as punishment. Motor writes happen in `flint-input-capture`'s rumble engine over direct XInput (ADR 0025, rumble spike direct XInput), fired early by a feel-tuned lead.

## Post-Processing Integration

The rung's visual half maps onto three [post-processing](post-processing.md) fields (ADR 0021, post-stack desaturation and blur mapping): `blur` becomes radial blur scaled by 0.6, `chromatic` and `desaturate` map 1:1. Each frame the session writes `authored + rung × scale` into any slot a script has *not* overridden, so script overrides win, preroll leaves the world untouched, and a ladder that recovers to clean restores the authored look. The F4 menu's "Freeze script post overrides" switch stops these stamps so panel edits stick while tuning.

## Scripts

A running session publishes a `ConductedSnapshot` every frame and the `conducted_*` family in [Scripting](scripting.md) reads it (ADR 0020, conducted parameters script surface): `conducted_lean()`, `conducted_target()`, `conducted_next_target()`, `conducted_next_pulse()`, `conducted_coherence()`, `conducted_beat_phase()`, `conducted_bar()`, `conducted_section()`, `conducted_pulses()`, `conducted_cues()`, `conducted_desaturate()`, `conducted_blur()`, `conducted_chromatic()`, `conducted_reassembly()`, `conducted_rewind()`, `conducted_no_input()`, `conducted_preroll()`. With no session running the snapshot is neutral (coherence and reassembly 1.0, lookaheads effectively infinite), so a script binding `1 - conducted_coherence()` to fog shows nothing.

Scripts are read-only toward the session. Nothing in Rhai can change a bus gain, a rung, or the chart.

## Recording, Replay and Offline Render

Every judgment is logged, and the input stream can be recorded to `logs/sessions/<name>.session.jsonl`: one JSON header line (suite, chart, sample rate, both offsets, config snapshots) then one event per line, each stamped with its compensated suite sample. Because the stamp is already musical time, `flint replay-chart` feeds the identical judgment and coherence code with no clock, no audio and no gamepad, and produces the same numbers. Synthetic profiles (`perfect`, `late:<ms>`, `neglect`) generate a stream from the chart alone. `flint render-suite` plays a scripted `*.events.toml` (bus gain, low-pass and detune changes at `bar:N`, `beat:F` or `sample:N` times) through the real scheduler and mixer into a WAV, deterministically.

## Debug Surfaces

All of these compile only with the player's `debug-hud` cargo feature (on by default) and never ship in the felt experience:

| Key | Surface |
|-----|---------|
| `` ` `` (backquote) | **Music Guide**: upcoming pulse, press and flick windows with a countdown, per-channel targets beside the live stick and trigger state (ADR 0035, music guide debug panel) |
| `\` (backslash) | **Manifest Map**: a full-width bottom strip of the whole suite, with sections, bar ruler, tempo changes, re-entry points, a playhead and this run's judged pulses and seams (ADR 0037) |
| `F9` | **Force fail**: trigger the full rewind, seam and reassembly without playing down to it. Routed through the ordinary trigger path so it exercises the real failure code |

With no gamepad visible, a debug-hud build arms a **keyboard fallback** (ADR 0064, debug keyboard input fallback): arrow keys are `lean`, Space is `pulse`. Prototype verbs only; it is a plumbing check, not a feel surface.

## Workflow

The CLI harnesses form a pipeline; each is documented in the [Music CLI reference](../cli-reference/music.md):

1. `flint validate-suite` cross-checks manifest, chart and stem files.
2. `flint play-suite` plays the stems sample-locked with a console readout of position and per-bus state.
3. `flint calibrate` records the player's median tap offset to `logs/latency/`.
4. `flint play-chart` runs the full reactive loop with live capture; `--window` adds wordless visual cues, `--record` writes a session file.
5. `flint replay-chart` re-judges a recorded or synthetic session headlessly.
6. `flint render-suite` renders a scripted session to WAV for listening tests and automated evidence.

Once the suite plays well in the harness, the `music_session` component brings the same session into `flint play`.

## Further Reading

- [Audio](audio.md): the two-bus `flint-audio` mixer the session shares a device with
- [Post-Processing](post-processing.md): the fields the ladder drives
- [Scripting](scripting.md): the `conducted_*` surface
- [Debug Panels](../guides/debug-panels.md): how the Music Guide and Manifest Map fit the panel system
