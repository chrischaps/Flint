# Profiling

Two tools say where a frame's time goes. **GPU timing** shows how long the GPU spends on each phase of the frame, in the F2 overlay and in `RenderStats`. The **`profile` feature** sends the frame loop's CPU time to [Tracy](https://github.com/wolfpld/tracy), scope by scope. Both are off until asked for, and a build without the feature carries none of the profiler.

## GPU timing

Open the F2 stats overlay and, on a device that can time, it gains a GPU section:

```
GPU: 2.57ms
  Grass          0.02
  Shadows        1.10
  Scene          0.26
  Post-process   0.43
  Composite      0.12
  World lines    0.04
  UI             0.01
```

Each line is a **scope**, a stretch of the frame between two timestamps the GPU writes as it reaches them (ADR 0108). The renderer scopes its own phases:
- `Grass`: the compute that scatters blades, when grass is on.
- `Shadows`: every cascade.
- `Scene`: the sky, the opaque and transparent draws, and the water.
- `Post-process`: the chain up to its last pass.
- `Composite`: that last pass, or `FXAA` when FXAA is on, since then FXAA is what writes the window.
- `World lines`.

The player adds `UI`, the HUD pass. The top line is the scopes' total, the GPU's busy time in the frame. The figures are smoothed over recent frames and come back a frame or two late, because the player never waits for them.

A scope covers the GPU's time on the work inside it, plus any time the GPU sat idle inside it waiting for the CPU's next submission. `Shadows` and `Post-process` are several submissions each, so on a CPU-bound frame they read a little high. Under vsync the GPU also waits for the window's next image before the first pass that draws into it. That pass is timed inside its own submission, so the wait falls between scopes and is in none of them: with vsync on or off, the figures are the GPU's own.

Timing needs `wgpu::Features::TIMESTAMP_QUERY`. Flint's window and headless contexts ask for it wherever the adapter has it, as most desktop GPUs do. Without it the GPU section just doesn't appear.

With `--exit-after` and `--stats`, the player prints the average GPU frame time beside its other `[perf]` lines, so a change can be compared without watching the overlay:

```
[perf] avg GPU 2.10 ms/frame over 412 timed frames
```

### From a game

Timing belongs to the renderer, which an extension reaches through `ExtensionCtx::gpu`:

```rust
fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
    if let Some(gpu) = &mut ctx.gpu {
        if self.overlay_open && !gpu.renderer.gpu_timing() {
            gpu.renderer.set_gpu_timing(gpu.device, gpu.queue, true);
        }
        let stats = gpu.renderer.collect_stats();
        self.graph.push(stats.gpu_frame_ms);
    }
}
```

`set_gpu_timing(device, queue, on)` returns whether timing is on, false where the device can't time. While F2 is open the player times the frame itself, and it stops again when F2 closes, unless the game had already turned it on.

A host that drives `SceneRenderer` itself can time its own work in the same frame. It scopes it with the timer and ends each frame once:

```rust
let timer = renderer.gpu_timer_mut().unwrap();
let scope = timer.begin(&mut encoder, "My pass");
// ... passes on `encoder` ...
timer.end(&mut encoder, scope);
queue.submit([encoder.finish()]);

renderer.finish_gpu_timings(&device, &queue); // after the frame's last timed submission
```

`begin_submit` and `end_submit` put the timestamps in submissions of their own, for work that makes its own encoders. A frame holds up to 32 scopes; past that, scopes time nothing.

## The `profile` feature

Build the player with `--features profile` (a game binary passes it on: `profile = ["flint-player/profile"]`) and run [Tracy's viewer](https://github.com/wolfpld/tracy/releases) beside it. The viewer finds the game on this machine and shows each frame as a stack of scopes on a timeline (ADR 0109):

- the frame loop: `tick` and its phases (`run_extension_pre_updates`, `run_scripts`, `tick_fixed_physics`, `tick_av_systems`, `sync_renderer`, ...), then `render` and `render_hud`;
- the renderer: `update_from_world`, `render_to`, and inside it `render_shadow_pass`, `render_main_pass` and `render_postprocess`;
- wgpu's own scopes, from the same `profiling` crate.

Each `RedrawRequested` ends a frame for Tracy, so its frame graph is the player's.

The client listens on loopback only, so there's no firewall prompt, and it keeps nothing until a viewer connects, so a profile build run without one doesn't grow. Without the feature, the `#[profiling::function]` attributes and `profiling::scope!` calls compile to nothing.

To scope a game's own code, depend on `profiling` (the version in Flint's `Cargo.toml`) and use the same attribute or macro. Under the player's `profile` feature they reach Tracy with the engine's:

```rust
#[profiling::function]
fn rebuild_houses(&mut self) { /* ... */ }

fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
    profiling::scope!("net");
    self.session.poll();
}
```

## Frame pacing

A mean frame time hides uneven frames. `--exit-after N` prints two spreads beside the mean, both taken after the first 2 s:

```
[perf] 480 frames over 8.00 s after t=2: avg 16.67 ms/frame (60.0 fps)
[perf] dt: p50 16.7 p90 16.7 p99 16.7 max 16.7 ms, 0 of 480 over 1.5x p50
[perf] present to present: p50 6.2 p90 29.6 p99 51.7 max 52.1 ms, 226 of 479 over 1.5x p50
```

`dt` is what the game steps by. `present to present` is the wall time between one `present()` returning and the next, which is when the swapchain let the CPU through.

With vsync (Fifo), every shown frame stays on screen for a whole number of refreshes. But the CPU can be let through in bursts: on a hybrid-GPU laptop (the dGPU renders and the iGPU drives the panel), frames came 6, 6, 6, then 45 ms apart at a 60 Hz mean. Stepping by those makes each shown frame move 6, 6, 6, then 45 ms of game time, which is judder. So under Fifo the player paces the clock: it passes the window's monitor refresh to `GameClock::set_vsync_period`, and each frame's `delta_time` is a whole number of refresh periods, as many as keep game time on the wall clock and at least one. Game time can run up to 100 ms ahead of the wall clock inside a burst. A stall over 250 ms passes through as one clamped step, as it would unpaced. Frames that come faster on average than that refresh (the wrong monitor, or not really vsynced) fall back to the raw time. `GameClock::raw_delta_time` always has the unpaced wall time.

Switches: `FLINT_PACING=0` turns pacing off, `FLINT_FRAME_LATENCY=1..3` sets the swapchain's frame latency (default 2), and `FLINT_VSYNC=0|mailbox` picks the present mode. A game that samples its own clock (interpolating snapshots at `Instant::now()`, say) should advance that clock by `delta_time` too, or it will still step in bursts.

## Further Reading

- [Debug Panels](debug-panels.md): F2, F3 and F4, and a game's own panels
- [Player Extensions](player-extensions.md): reaching the renderer from a game
- [Rendering](../concepts/rendering.md): the passes the scopes cover
