# ADR 0108: GPU time per phase, from timestamp queries read back without waiting

Status: accepted, 2026-10-21 ([#83](https://github.com/chrischaps/Flint/pull/83)).
Backfilled 2026-11-12 (#374) from PR #83, whose body holds the ADR text for
0107 to 0109, and commit `35afb2a`. Amended in practice by #146 and #126
(see Consequences).

## Context

F2 showed the CPU's frame time and draw counts, but nothing said where the
GPU's time went.

## Decision

`flint_render::GpuTimer`, owned by `SceneRenderer` while
`set_gpu_timing(device, queue, true)` is on (off by default).

- A scope is two timestamps. Each is written by an empty compute pass
  carrying `beginning_of_pass_write_index`, so only
  `Features::TIMESTAMP_QUERY` is needed and no existing pass descriptor
  changes. `begin`/`end` scope within an encoder; `begin_submit`/`end_submit`
  put the timestamps in submissions of their own.
- `finish_frame` resolves the queries into one of three read-back buffers and
  maps it. Results are picked up at a later frame without waiting, as the
  grass count is ([ADR 0092](0092-grass-culled-to-view-drawn-indirect.md)).
  A frame that finds all three in flight goes untimed. At most 32 scopes a
  frame.
- The renderer scopes `Grass`, `Shadows`, `Scene`, `Post-process`, the pass
  that writes the target (`Composite`, or `FXAA`) and `World lines`; the
  player adds `UI`.
- The pass that writes the target was timed inside its own submission, so a
  vsync wait for the swapchain image fell between scopes.
- Times are smoothed (EMA, 0.1) into `RenderStats::gpu_passes` and
  `gpu_frame_ms`, the scopes' total: the GPU's busy time, not first to last.
- The window and headless contexts request `TIMESTAMP_QUERY` where the
  adapter has it. The player turns timing on while F2 is open; `--exit-after`
  with `--stats` prints `[perf] avg GPU … ms/frame`.

## Alternatives considered

- **`TIMESTAMP_QUERY_INSIDE_ENCODERS`.** Not needed: empty compute passes
  carry the timestamps.
- **A scope across the swapchain wait.** It counted the vsync wait:
  Post-process read 15.3 ms at 60 fps for 0.55 ms of work.

## Consequences

- Off: no timer, no queries, no extra passes or submissions. On, as decided:
  four extra submissions a frame, a resolve and a 512-byte copy.
- A multi-submission scope counts any idle time between its submissions.
- Since then: the frame goes to the GPU in one submission (#146, for #139).
  Every scope is in the frame's one encoder, `encode_gpu_timings` records the
  resolve into it, and `frame_submitted` starts the read-back.
  `finish_gpu_timings` remains for a host that submits on its own.
- Since then: the post chain has a scope per effect (SSAO, Bloom,
  Volumetric, Kuwahara) instead of one `Post-process` (#126, for #120),
  and the style mask has its own.
- `flint render --gpu-time <frames>` prints the same scopes.

## Measured

The Capitol, offline, 1280×720, `--stats`: vsync on, 60 fps, GPU 1.16 ms
(Shadows 0.36, Scene 0.23, Post-process 0.37, Composite 0.14). Vsync off,
192 fps, GPU 2.57 ms.

## Sources

- [#83](https://github.com/chrischaps/Flint/pull/83) / commit
  [`35afb2a`](https://github.com/chrischaps/Flint/commit/35afb2a254c411881cf581bc4e2f5be134cbbef6)
  "[#374] Flint: game debug panels, GPU timing, a frame profiler (ADRs [0107](0107-debug-panels-from-the-game.md)-[0109](0109-tracy-frame-profiler.md))"
- [#146](https://github.com/chrischaps/Flint/pull/146) / commit
  [`7b1d2b0`](https://github.com/chrischaps/Flint/commit/7b1d2b00f331271b1f676e974bb9c95dc5b4a058)
  "Less render CPU on the web: one submit, cached bind groups, batched uploads (#139)"
- [#126](https://github.com/chrischaps/Flint/pull/126) / commit
  [`ea0cfff`](https://github.com/chrischaps/Flint/commit/ea0cfff2c65ff5a13fecf434a9f2854ccfa4e81a)
  "Web perf readout: ?perf=1 frame, phase, script and pass times (#120)"
- Code now: `crates/flint-render/src/gpu_timer.rs` (`GpuTimer`),
  `crates/flint-render/src/scene_renderer/mod.rs` (`set_gpu_timing`,
  `encode_gpu_timings`, `frame_submitted`),
  `crates/flint-render/src/render_stats.rs`, `crates/flint-render/tests/gpu_timer.rs`
- Book: [Profiling](../book/src/guides/profiling.md)
