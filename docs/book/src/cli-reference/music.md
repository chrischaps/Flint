# Music Commands

Seven subcommands drive the rhythm system described in [Music Sessions](../concepts/music-sessions.md). They work on a **suite manifest** (`*.suite.toml`), a **chart** (`*.chart.toml`) and the optional tuning files under `config/`; see [File Formats: Music Session Files](../formats/overview.md#music-session-files) for the file grammar.

The usual order is: validate the suite, play it raw to hear the stems, calibrate the player's latency, play the chart live, replay the recording headless to reproduce judgment, and render a scripted session to WAV for listening or CI.

Every command takes `--base-dir <path>`: the directory the manifest's file paths are relative to (default: the current directory). Reports and logs land under that directory's `logs/`.

## `flint validate-suite`

Check a suite manifest, and optionally cross-check a chart against it.

```bash
flint validate-suite music/prototype.suite.toml
flint validate-suite music/prototype.suite.toml --chart music/prototype.chart.toml --no-assets
```

| Flag | Default | Description |
|------|---------|-------------|
| `<manifest>` | (required) | Path to the suite manifest |
| `--chart <path>` | (none) | Chart to cross-check: channels, pulse kinds, interpolation modes, beats inside the suite |
| `--no-assets` | `false` | Skip the asset pass (stem file existence, sample rate, duration) |
| `--base-dir <path>` | cwd | Manifest path root |

Validation is shape-tolerant: unknown channels or pulse kinds parse fine and are reported here rather than rejected at load.

## `flint play-suite`

Play a validated suite's stems on the six fixed buses, sample-locked, with no chart and no judgment. This is the "does the music itself work" check.

```bash
flint play-suite music/prototype.suite.toml --bars 8
```

| Flag | Default | Description |
|------|---------|-------------|
| `<manifest>` | (required) | Path to the suite manifest |
| `--bars <n>` | (to the end) | Stop after this many bars |
| `--base-dir <path>` | cwd | Manifest path root |

## `flint calibrate`

Tap-to-beat latency calibration. Plays the suite's beat grid, collects the player's taps, and writes the median offset to `logs/latency/calibration-*.toml`. Later `play-chart` sessions read the offset so judgment is measured against what the player heard, not what the clock said.

```bash
flint calibrate music/prototype.suite.toml --taps 24
```

| Flag | Default | Description |
|------|---------|-------------|
| `<manifest>` | (required) | Path to the suite manifest |
| `--taps <n>` | `16` | Number of taps to collect |
| `--base-dir <path>` | cwd | Manifest path root |

## `flint play-chart`

Play a suite against its chart with live gamepad capture. This is the development harness for the whole reactive loop: coherence, the disintegration ladder, the seam and reintegration, the audio gradient and haptics. A gamepad is expected; input is captured on a dedicated 1 kHz thread using the XInput backend.

```bash
flint play-chart music/prototype.suite.toml --chart music/prototype.chart.toml
flint play-chart music/prototype.suite.toml --chart music/prototype.chart.toml \
  --window --record take_03 --ladder config/ladder.toml --input-map full
```

| Flag | Default | Description |
|------|---------|-------------|
| `<manifest>` | (required) | Path to the suite manifest |
| `--chart <path>` | (required) | Beatmap chart for the suite |
| `--base-dir <path>` | cwd | Manifest path root |
| `--bars <n>` | (to the end) | Stop after this many bars |
| `--config <path>` | `config/coherence.toml` if present | Coherence config TOML |
| `--lean-mode <mode>` | `arrival` | Lean judgment: `arrival` (be at each target on its beat, roll freely between) or `track` (follow the curve continuously) |
| `--ladder <path>` | `config/ladder.toml` if present | Disintegration ladder TOML |
| `--gradient <path>` | `config/gradient.toml` if present, else inert | Error-driven audio gradient TOML |
| `--haptics <path>` | `config/haptics.toml` if present, else no rumble | Haptics TOML |
| `--input-map <map>` | `prototype` | Physical-to-verb mapping: `prototype` (left stick = lean, South / R2 = pulse) or `full` (adds sway on the right stick, trigger pressure, press onsets and flicks) |
| `--record <name>` | (none) | Record the input session to `logs/sessions/<name>.session.jsonl` |
| `--window` | `false` | Open a bare visual window that absorbs keystrokes from gamepad-to-keyboard mappers and shows wordless cues. Console output continues underneath |
| `--spike-input-secs <n>` | (none) | Run the input-granularity spike for `n` seconds and exit, with no audio |

If the input backend sees no gamepad the command warns loudly at startup rather than recording a silent session. The player's debug keyboard fallback (arrows = lean, Space = pulse) applies only to `flint play` scenes with a `music_session` component, not to this command.

## `flint replay-chart`

Replay a recorded or synthetic session through judgment, fully headless. The same recording replayed twice produces the same judgment log, which is what makes the feel work reviewable.

```bash
# Reproduce a recorded take
flint replay-chart music/prototype.suite.toml --chart music/prototype.chart.toml \
  --session logs/sessions/take_03.session.jsonl

# Synthetic player, 40 ms late on everything, with reactive audio rendered to WAV
flint replay-chart music/prototype.suite.toml --chart music/prototype.chart.toml \
  --synthetic late:40 --ladder config/ladder.toml --render out/late40.wav
```

| Flag | Default | Description |
|------|---------|-------------|
| `<manifest>` | (required) | Path to the suite manifest |
| `--chart <path>` | (required) | Beatmap chart for the suite |
| `--base-dir <path>` | cwd | Manifest path root |
| `--session <path>` | (none) | Session file to replay. Conflicts with `--synthetic` |
| `--synthetic <profile>` | (none) | Synthesize a session instead: `perfect`, `late:<ms>` or `neglect` |
| `--config <path>` | the session's recorded snapshot | Coherence config TOML |
| `--lean-mode <mode>` | `arrival` | `arrival` or `track`; must match the run being reproduced (judgment-log headers record it) |
| `--ladder <path>` | `config/ladder.toml` if present | Disintegration ladder TOML. With `--render`, makes the render reactive (the full fall-and-reintegration loop) |
| `--gradient <path>` | `config/gradient.toml` if present, else inert | Error-gradient TOML, applied inside the reactive render only |
| `--out <path>` | `logs/judgment/replay.jsonl` | Judgment log output path |
| `--save-session <path>` | (none) | Also save the replayed event stream as a session file (useful after `--synthetic`) |
| `--render <path>` | (none) | Also render the suite audio over the replayed span to this WAV |

## `flint render-suite`

Render a scripted suite session to a 32-bit float stereo WAV, offline and deterministic. The event script schedules bus gain, low-pass and detune changes on bars or beats, so a whole arrangement can be auditioned or diffed without playing it.

```bash
flint render-suite music/prototype.suite.toml --script music/intro.events.toml -o out/intro.wav
flint render-suite music/prototype.suite.toml -o out/full.wav --duration-bars 32 --status-every beat
```

| Flag | Default | Description |
|------|---------|-------------|
| `<manifest>` | (required) | Path to the suite manifest |
| `-o`, `--output <path>` | (required) | Output WAV path |
| `--script <path>` | (none) | Event script (`*.events.toml`) of scheduled bus changes and markers |
| `--base-dir <path>` | cwd | Manifest path root |
| `--duration-bars <n>` | length of the longest stem | Render this many bars. Conflicts with `--duration-seconds` |
| `--duration-seconds <s>` | (none) | Render this many seconds |
| `--status-every <unit>` | `bar` | Status line cadence: `bar` or `beat` |
| `--chunk-frames <n>` | `128` | Processing chunk size in frames, which is also the scheduling granularity |

## `flint spike-rumble`

Fire the gamepad's force-feedback motors, time the command paths, and write the report beside the audio-latency and input-granularity spikes in `logs/latency/`. Used to characterise a controller before enabling haptics.

```bash
flint spike-rumble
flint spike-rumble --no-feel
```

| Flag | Default | Description |
|------|---------|-------------|
| `--base-dir <path>` | cwd | Directory whose `logs/latency/` receives the report |
| `--no-feel` | `false` | Skip the operator-felt tick / thump / grind demo; timing only |
