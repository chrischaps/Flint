### `--trace`, `--trace-seconds`, `--trace-on-hitch` and Shift+F3 save the engine's scopes as a Perfetto trace (#1097)
**What changed.** The scope recorder records now. `flint play` and
`flint-player` take `--trace FILE` (the last 5 s of every `flint_core::scope!`,
written when the run ends), `--trace-seconds N` (how far back; 0 keeps the
whole session) and `--trace-on-hitch[=N]` (a trace eight frames after each of
the first N hitches past the start, beside `--trace`'s file or in `traces/` at
the project's root). The new `save_trace` debug key, Shift+F3, saves one
mid-play. The file is Chrome trace-event JSON that Perfetto opens: a track
per thread, plus frames, phases, GPU passes, hitches and frame notes. Each
script callback is a scope of its own while recording
(`scripts/hud.rhai:on_draw_ui`), and a frame capture taken while recording
holds `trace.json`. Every debug build has the recorder; a release build
needs the `trace` feature, and refuses the flags without it. On the web,
`?perf=1` shows a Save trace button in a build with the recorder.
**What to do.** Nothing. Add `traces/` to a game's `.gitignore`: a trace
names its scenes, scripts and assets.
