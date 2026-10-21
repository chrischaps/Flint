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

## Further Reading

- [Debug Panels](debug-panels.md): F2, F3 and F4, and a game's own panels
- [Player Extensions](player-extensions.md): reaching the renderer from a game
- [Rendering](../concepts/rendering.md): the passes the scopes cover
