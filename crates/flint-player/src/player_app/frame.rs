//! Frame loop for `PlayerApp` — code-motion sibling of `mod.rs`
//! (player_app decomposition; see the decomposition ADR). Owns per-frame
//! tick (scripts, subsystems, transitions), render, the egui HUD pass,
//! and the stats overlay.

use super::camera_cut;
use super::hud_render::render_draw_commands;
use super::subsystem::{EncodeCtx, FrameCtx};
use super::PlayerApp;
use super::TransitionPhase;
use flint_render::{FrameInputs, GrassEntityPosition, View2d};
use flint_runtime::state::AUDIO;
use flint_runtime::{RuntimeSystem, StateConfig};
use std::collections::HashSet;

/// Frame time given to decoding sounds while a late audio bundle settles in
/// (#124). A song is 85 to 190 ms of decoding in Chrome; at 2 ms a frame it
/// spreads over a second or two, and Float's 10 ms frames stay inside one
/// 16.7 ms vsync (at 4 ms they slipped to 33 ms now and then).
const AUDIO_DECODE_BUDGET: std::time::Duration = std::time::Duration::from_millis(2);

impl PlayerApp {
    /// The screen in points as scripts see it (`screen_width()` and
    /// `screen_height()`, and the UI documents' layout): egui's screen, once
    /// it agrees with the window. For egui's first passes its `screen_rect()`
    /// is a 10,000-point placeholder (15,000 once a `--pixels-per-point`
    /// zoom lands on a 1.5x display), and a script sizing its HUD by the
    /// screen asked for lettering 14 to 21 times too big, whose glow wanted
    /// a texture past the GPU's limit (#657). Until egui agrees, it's the
    /// window's size at the scale egui is about to use.
    pub(super) fn script_screen_size(&self) -> (f32, f32) {
        let rect = self.hud.egui_ctx.content_rect();
        let egui_size = (rect.width(), rect.height());
        let Some(window) = &self.window else {
            return self.launch.headless_screen.unwrap_or(egui_size);
        };
        let size = window.inner_size();
        let ppp = self
            .launch
            .pixels_per_point
            .unwrap_or(window.scale_factor() as f32)
            .max(0.01);
        let window_size = (size.width as f32 / ppp, size.height as f32 / ppp);
        let agrees = (egui_size.0 - window_size.0).abs() <= 1.0
            && (egui_size.1 - window_size.1).abs() <= 1.0;
        if agrees || size.width == 0 || size.height == 0 {
            egui_size
        } else {
            window_size
        }
    }

    pub(super) fn render(&mut self) {
        flint_core::scope!("render");
        // On Android, window may be None between suspended() and resumed()
        if self.window.is_none() {
            return;
        }
        self.update_render_scale();
        if let Some((look, seconds)) = self.style.pending_look.take() {
            if let Err(e) = self.fade_to_look(look.as_deref(), seconds) {
                tracing::warn!("look: {e}");
            }
        }
        // A look's cross-fade (#496) eases what the two looks differ in, on
        // game time, under the scripts' overrides stamped below
        self.step_look_fade(self.core.clock.delta_time);
        // The presented frame as a PNG (#583): a capture line on the
        // expression socket, `--record`. Nothing at all while neither asks
        #[cfg(not(target_arch = "wasm32"))]
        self.take_capture_asks();
        #[cfg(not(target_arch = "wasm32"))]
        let grab = (!self.frame_grabs.idle())
            .then(|| self.frame_grabs.has_asks().then(|| self.capture_line_dir()));
        let Some(context) = &self.render_context else {
            return;
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };

        // The wait for the image is the swapchain's, not the renderer's
        // (#1123): a compositor stall reads as `elsewhere`. A field borrow,
        // as below: `context` is held.
        let acquired = {
            flint_core::scope!("acquire");
            self.stats.perf.acquire(&context.surface)
        };
        let output = match acquired {
            Ok(output) => output,
            Err(flint_render::SurfaceError::Lost | flint_render::SurfaceError::Outdated) => {
                return;
            }
            Err(e) => {
                tracing::warn!("Surface error: {:?}", e);
                return;
            }
        };

        let view = context.frame_view(&output.texture);

        // Apply script-driven post-processing overrides before rendering.
        // The Rendering & Effects panel's freeze switch (ADR 0053) skips the
        // stamp entirely so panel edits to contended fields stick; the
        // drained overrides are plain Options re-filled each tick, so nothing
        // piles up and un-freezing resumes on the next script write.
        #[cfg(feature = "debug-hud")]
        let pp_stamp_frozen = self.post.debug_freeze;
        #[cfg(not(feature = "debug-hud"))]
        let pp_stamp_frozen = false;
        if !pp_stamp_frozen && self.post.any() {
            let mut config = renderer.post_process_config().clone();
            self.post.stamp(&mut config);
            renderer.set_post_process_config(config);
        }

        // Every panel applies its edits and reads back what it shows (#744):
        // one loop, no panel asked its name. What they tell the player comes
        // back through `PanelHost`.
        #[cfg(feature = "debug-hud")]
        {
            flint_core::scope!("sync_debug_panels", self.debug_panels.len());
            let mut host = flint_debug_ui::PanelHost {
                particles_render: self.particles_render_enabled,
                ..Default::default()
            };
            let mut ctx = flint_debug_ui::PanelCtx {
                world: &mut self.core.world,
                renderer: &mut *renderer,
                device: &context.device,
                camera: &mut self.camera,
                particles: &mut self.core.particles.sync,
                host: &mut host,
            };
            for panel in &mut self.debug_panels {
                panel.sync(&mut ctx);
            }
            if let Some(freeze) = host.freeze_scripts {
                self.post.debug_freeze = freeze;
            }
            if let Some(look) = host.look {
                self.style.pending_look = Some((look, host.look_seconds));
            }
            self.particles_render_enabled = host.particles_render;
            // A style the F4 or F3 editor just wrote into the scene file
            // (#364): the scene's authored settings after it, for the other
            // editor too
            if let Some(authored) = host.committed_styles {
                super::debug_panels::set_panels_authored_styles(&mut self.debug_panels, &authored);
            }
        }

        // F2's stats show the GPU's time too (ADR 0108), as does the perf
        // readout unless it was asked not to (#120): time the frame while
        // either is up. Timing the game turned on stays on after.
        if self.stats.view == super::StatsView::Full
            || (self.stats.perf.readout && self.stats.perf.gpu_wanted)
            || self.stats.perf.trace.is_some()
            || self.stats.report.is_some()
        {
            if !renderer.gpu_timing() {
                self.stats.gpu_timing =
                    renderer.set_gpu_timing(&context.device, &context.queue, true);
            }
        } else if self.stats.gpu_timing {
            renderer.set_gpu_timing(&context.device, &context.queue, false);
            self.stats.gpu_timing = false;
        }

        // The whole frame (scene, HUD, the GPU timer's resolve) goes into
        // one encoder and one submission (#139): on the web each submission
        // crosses into JS.
        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Encoder"),
            });
        // `--pixels-per-point` (#435) pins egui's scale with the renderer's
        // (`renderer_pixels_per_point`), so a capture's glyphs and panels
        // are the same size on any display
        if let Some(window) = &self.window {
            let native = window.scale_factor() as f32;
            if let Some(ppp) = self.launch.pixels_per_point {
                let zoom = ppp / native.max(0.01);
                if (self.hud.egui_ctx.zoom_factor() - zoom).abs() > 1e-4 {
                    self.hud.egui_ctx.set_zoom_factor(zoom);
                }
            }
        }
        // A frame capture (#1086): the renderer describes this frame and
        // copies each pass's targets as it goes
        #[cfg(not(target_arch = "wasm32"))]
        let capture_adapter = self
            .frame_captures
            .armed()
            .then(|| context.adapter.get_info());
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(info) = &capture_adapter {
            let targets = super::frame_capture::Targets::of(context, &output.texture, info);
            self.frame_captures.arm(renderer, &targets);
        }
        // A scene of comic panels draws each panel's camera, submitting all
        // but the last itself (#880, ADR 0120); one view is one submission
        renderer.encode_frame_to(
            &context.device,
            &context.queue,
            &mut encoder,
            // With its id, so bind groups over it are kept (#379)
            context.depth_target(),
            &self.camera,
            &view,
            Some(&self.core.world),
        );
        // A second look drawn over part of the frame, before the HUD (the
        // Living Props A/B split, #238)
        if !self.core.subsystems.is_empty() {
            let mut ctx = EncodeCtx {
                encoder: &mut encoder,
                device: &context.device,
                queue: &context.queue,
                output: &output.texture,
                depth_view: &context.depth_view,
                formats: (context.view_format, context.hud_format),
                renderer: &mut *renderer,
                world: &self.core.world,
                camera: &self.camera,
                #[cfg(feature = "living")]
                living: &mut self.core.living_props,
            };
            self.core.subsystems.post_encode(&mut ctx);
        }
        // A field borrow, not `perf_mark`: `context` is still held.
        self.stats.perf.mark(super::perf::Phase::Render);

        // Render egui HUD overlay on top of the 3D scene
        // HUD draws into a non-sRGB view of the same texture (see
        // RenderContext::hud_view) so egui blends in display space.
        let hud_view = context.hud_view(&output.texture);
        self.render_hud(&hud_view, &mut encoder);
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            // A captured frame's HUD and bone palettes, in its encoder (#1086)
            #[cfg(not(target_arch = "wasm32"))]
            if capture_adapter.is_some() {
                let device = &context.device;
                self.frame_captures
                    .host_pass(renderer, device, &mut encoder, &output.texture);
                self.frame_captures
                    .record_replay(renderer, device, &mut encoder, &self.core.world);
            }
            // The recorder's GPU track (#1097): this frame's scopes carry
            // its index, to be laid beside it when they come back
            let tracing = flint_core::profile::COMPILED_IN && self.stats.perf.trace.is_some();
            if let (true, Some(timer)) = (tracing, renderer.gpu_timer_mut()) {
                timer.trace(true);
                timer.set_frame_tag(flint_core::profile::frame() as u64);
            }
            // The frame as shown, scene and HUD, copied for a capture line
            // or the recording (#583), in the same encoder
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(dir) = &grab {
                flint_core::scope!("frame_grab");
                let at = super::frame_grab::GrabClock {
                    game_time: self.core.clock.total_time,
                    stepped: self.core.clock.frame_step.is_some(),
                };
                let place = super::frame_grab::GrabPlace {
                    capture_dir: dir.as_deref(),
                    scene_path: &self.core.scene_path,
                };
                self.frame_grabs.before_submit(
                    &context.device,
                    &mut encoder,
                    &output.texture,
                    context.surface_copyable,
                    at,
                    &place,
                );
            }
            // Its GPU time comes back under its number in the history, to
            // be laid on its own record a frame or two on (#1104)
            if let Some(timer) = renderer.gpu_timer_mut() {
                timer.set_frame_index(self.stats.perf.gpu_frame_index());
            }
            renderer.encode_gpu_timings(&context.device, &mut encoder);
            {
                flint_core::scope!("submit");
                context.queue.submit(std::iter::once(encoder.finish()));
            }
            renderer.frame_submitted();
            // Its map asked for, and earlier ones polled, never waited on
            #[cfg(not(target_arch = "wasm32"))]
            if grab.is_some() {
                flint_core::scope!("frame_grab_poll");
                self.frame_grabs.submitted();
                self.frame_grabs.poll(&context.device);
            }
            if let (true, Some(trace), Some(timer)) = (
                tracing,
                self.stats.perf.trace.as_deref_mut(),
                renderer.gpu_timer_mut(),
            ) {
                trace.tracks.gpu(timer.take_traced(), timer.timestamps());
            }
            // The GPU times read back this frame, each onto the frame it
            // timed in the history
            for time in renderer.take_gpu_frame_times() {
                self.stats.perf.gpu_time(time.index, time.ms);
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                if let Some(info) = &capture_adapter {
                    let targets = super::frame_capture::Targets::of(context, &output.texture, info);
                    let facts = super::frame_capture::FrameFacts {
                        world: &self.core.world,
                        camera: &self.camera,
                        particles: self
                            .particles_render_enabled
                            .then_some(&self.core.particles.sync),
                        scene_path: &self.core.scene_path,
                        project: &self.core.project,
                        index: self
                            .stats
                            .perf
                            .recording()
                            .then(|| self.stats.perf.under_way_index()),
                        game_time: self.core.clock.total_time,
                        dt: self.core.clock.delta_time,
                    };
                    if let Some(line) = self.frame_captures.submitted(renderer, &targets, &facts) {
                        self.hud.key_note.say("capture", &line);
                    }
                    self.stats.perf.capturing();
                }
                // A capture on its way back: polled, never waited on
                let timed = self.core.script.timing();
                if self.frame_captures.poll(
                    renderer,
                    &context.device,
                    &context.queue,
                    &self.stats.perf,
                    self.stats.hitches.hitches(),
                    timed,
                ) {
                    self.stats.perf.capturing();
                }
                // A capture handed to its writer gets the trace beside it
                self.traces_into_captures();
            }
        }
        self.say_frame_captures();
        self.perf_mark(super::perf::Phase::Hud);

        // Capture the composited frame (scene + HUD) before it is presented.
        self.capture.last_frame_time = Some(self.core.clock.total_time);
        if let Some(path) = self.capture_path_due() {
            flint_core::scope!("screenshot");
            // The read-back waits on the GPU and the PNG is written here, on
            // the frame: noted, so the hitch it makes names it rather than
            // reading as time elsewhere (#1129)
            let _shot = flint_core::frame_notes::begin(
                flint_core::frame_notes::NoteKind::Screenshot,
                || flint_core::project::shown(&path),
            );
            match self.write_screenshot(&output.texture, &path) {
                Ok(()) => self.capture.written += 1,
                Err(reason) => {
                    tracing::warn!("screenshot: {reason}");
                    self.capture.note_failure(reason);
                }
            }
        }

        if let Some(context) = &self.render_context {
            flint_core::scope!("present");
            context.queue.present(output);
        }
        let presented = web_time::Instant::now();
        // A load's first frame is on screen: its table (#1092)
        self.frame_presented_for_loads();
        self.stats.perf.presented_at(presented);
        self.perf_mark(super::perf::Phase::Present);
        #[cfg(target_arch = "wasm32")]
        self.show_perf();
        #[cfg(not(target_arch = "wasm32"))]
        self.print_perf();

        if let Some(exit_after) = self.capture.exit_after {
            if self.core.clock.total_time >= super::PERF_SKIP_SECS {
                // `--perf-report`'s GPU passes: every frame from here (#1093)
                if self.stats.report.is_some() {
                    if let Some(timer) =
                        self.scene_renderer.as_mut().and_then(|r| r.gpu_timer_mut())
                    {
                        if !timer.tallying() {
                            timer.tally(true);
                        }
                    }
                }
                self.capture.perf_frames += 1;
                self.capture.perf_time += self.core.clock.delta_time;
                self.capture
                    .perf_pacing
                    .record(self.core.clock.delta_time, presented);
                if let Some(renderer) = &self.scene_renderer {
                    let stats = renderer.collect_stats();
                    self.capture.perf_draws += stats.draw_calls as u64;
                    self.capture.perf_entity_draws += stats.entity_draws as u64;
                    self.capture.perf_culled += stats.culled_draws as u64;
                    self.capture.perf_lod_culled += stats.lod_culled_draws as u64;
                    self.capture.perf_shadow_triangles += stats.shadow_triangles as u64;
                    self.capture.perf_triangles += stats.triangles as u64;
                    if stats.gpu_frame_ms > 0.0 {
                        self.capture.perf_gpu_ms += stats.gpu_frame_ms as f64;
                        self.capture.perf_gpu_frames += 1;
                    }
                    self.capture.perf_gpu_calls += stats.gpu_calls;
                    if stats.gpu_calls.created() > 0 {
                        self.capture.perf_gpu_creating += 1;
                    }
                }
            }
            // A shot that's due but failed waits for its retries (#639).
            let now = self.core.clock.total_time;
            // A script's `quit_game()` this frame has already said it (#659).
            if now >= exit_after && !self.capture.shot_pending(now) && !self.outcome.exit_requested
            {
                self.outcome.exit_requested = true;
                self.print_exit_report();
            }
        }
    }

    /// `--exit-after`'s report, at its time or at a script's `quit_game()`
    /// before it (#659).
    pub(super) fn print_exit_report(&mut self) {
        // Headless perf figure: average frame time over the run, so a
        // scene change can be A/B'd without watching the overlay.
        // The lines and `--perf-report`'s file both come from these (#1093)
        let numbers = self.exit_numbers();
        if let Some(n) = &numbers {
            let (frames, avg_ms) = (n.frames, n.mean_ms());
            // The run's figures as fields too, for `--log-format json` (#716)
            tracing::info!(
                target: "report",
                frames,
                avg_ms = (avg_ms * 100.0).round() / 100.0,
                fps = (10_000.0 / avg_ms).round() / 10.0,
                "[perf] {frames} frames over {:.2} s after t={:.0}: avg {avg_ms:.2} ms/frame ({:.1} fps)",
                n.secs,
                super::PERF_SKIP_SECS,
                1000.0 / avg_ms
            );
            // How even they were: a mean hides bursts (#406); the F2
            // overlay's main figures, averaged, so a capture can compare
            // draw counts and culling headlessly (H9); the GPU's time when
            // the frames were timed (--stats, ADR 0108), which under vsync
            // reads high on an idle GPU's clocks (#380)
            let present = self
                .render_context
                .as_ref()
                .map(|c| c.present_mode_in_use());
            let note = n.gpu.map_or(String::new(), |g| {
                super::perf::gpu_vsync_note(present, g.mean_ms as f32, avg_ms as f32)
            });
            for line in n.lines(&note) {
                tracing::info!(target: "report", "{line}");
            }
            // The GPU memory the renderer holds as the run ends (#1091)
            if let (Some(r), Some(ctx)) = (&self.scene_renderer, &self.render_context) {
                let device = flint_render::gpu_counts::device_memory(&ctx.device);
                tracing::info!(target: "report",
                    "[perf] GPU memory at exit: {}",
                    super::perf::gpu_memory_part(&r.gpu_memory(), device)
                        .trim_start_matches("gpu mem ")
                );
            }
            // That GPU total leaves out the scopes that read zero (#1316)
            if n.gpu.is_some() {
                if let Some(why) = self.gpu_timestamps_unusable() {
                    tracing::warn!(target: "report",
                        "[perf] {}: {why}, so the GPU total counts only the scopes that read",
                        flint_render::TIMESTAMPS_UNAVAILABLE
                    );
                }
            }
        }
        // The run's hitches (#1079): a mean hides the one long frame. The
        // log's last lines are written now, with the frames after them
        // that there are.
        if self.stats.perf.recording() {
            self.stats.watch.finish();
            for line in self.stats.watch.report() {
                tracing::info!(target: "report", "{line}");
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.write_perf_report(numbers.as_ref());
        for line in self.subsystem_report() {
            tracing::info!(target: "report", "{line}");
        }
        // Every script error of the run, each once with its count (#693)
        for line in self.core.script.errors().summary() {
            tracing::info!(target: "report", "{line}");
        }
        // The input script's expect and wait lines (#714): what's left is
        // not reached, then a line each that failed
        let now = self.core.clock.total_time;
        if let Some(script) = &mut self.capture.input_script {
            if script.has_checks() {
                script.finish(now);
                for line in super::input_checks::summary(script.outcomes()) {
                    tracing::info!(target: "report", "{line}");
                }
            }
        }
    }

    /// Decide whether this frame should be written to disk, and to where.
    ///
    /// Single-shot: the first frame at or after `capture.at`. Interval mode:
    /// the first frame at or after `capture.at`, then every `every` seconds,
    /// numbered `<stem>_0001.<ext>`, `<stem>_0002.<ext>`, ...
    fn capture_path_due(&mut self) -> Option<std::path::PathBuf> {
        let base = self.capture.path.clone()?;
        let now = self.core.clock.total_time;
        // Tolerance so `--screenshot-at` means the frame at that time: 210 steps of
        // 1/60 add up to a hair under 3.5, and the next frame is a step too late.
        let reached = now + 1e-6 >= self.capture.at;
        match self.capture.every {
            None => {
                if self.capture.taken || !reached {
                    return None;
                }
                self.capture.taken = true;
                Some(base)
            }
            Some(every) => {
                if !self.capture.taken {
                    if !reached {
                        return None;
                    }
                    self.capture.taken = true;
                    self.capture.next_due = now;
                }
                // Tolerance so `--screenshot-every` equal to `--frame-step`
                // writes every frame instead of losing some to rounding.
                if now + 1e-6 < self.capture.next_due {
                    return None;
                }
                self.capture.sequence += 1;
                self.capture.next_due = now + every.max(1.0 / 240.0);
                Some(numbered_capture_path(&base, self.capture.sequence))
            }
        }
    }

    /// Read the swapchain texture back and save it as a PNG; the reason
    /// it couldn't, if it couldn't (#639).
    fn write_screenshot(
        &self,
        texture: &wgpu::Texture,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let Some(context) = &self.render_context else {
            return Err("no render context".into());
        };
        if !context.surface_copyable {
            return Err(format!(
                "skipped {}: the surface has no COPY_SRC usage on this adapter",
                path.display()
            ));
        }
        let pixels = context
            .read_surface_rgba(texture)
            .map_err(|e| format!("read-back of {} failed: {e}", path.display()))?;
        let img = image::RgbaImage::from_raw(texture.width(), texture.height(), pixels)
            .ok_or_else(|| format!("pixel buffer size mismatch for {}", path.display()))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
        }
        img.save(path)
            .map_err(|e| format!("failed to save {}: {e}", path.display()))?;
        tracing::info!(
            "screenshot written: {} ({}x{} at t={:.2}s)",
            path.display(),
            texture.width(),
            texture.height(),
            self.core.clock.total_time
        );
        Ok(())
    }

    pub(super) fn tick(&mut self) {
        flint_core::scope!("tick");
        // A frame begins: the frame history's clock (#120, #1076).
        self.stats.perf.begin_frame();
        use super::perf::Phase;

        // The page's Begin click and the browser's pointer lock (#102).
        #[cfg(target_arch = "wasm32")]
        self.sync_pointer_lock();

        {
            flint_core::scope!("poll_devices");
            // Pads are held while a text field has focus (ADR 0110), before
            // they're polled.
            self.gate_gamepads();

            // The touch controls' layout follows the window and page (#133).
            self.sync_touch_controls();

            // Before the clock ticks: a subsystem that reads the pads its own
            // way does it here (the rhythm session's capture drain, ADR 0018),
            // and the player polls them only while none holds them.
            super::game_core::run_stage(self, super::subsystem::Stage::Clock, None);
            if !self.pads_lent {
                self.poll_gamepad_events();
            }
        }

        // Advance game clock. Its tick measured the frame just closed: that
        // is the history's newest frame, which is judged now (#1076).
        self.core.clock.tick();
        let mut found = 0;
        if self.stats.perf.recording() {
            flint_core::scope!("frame_history");
            self.stats
                .perf
                .clock_ticked(super::frame_history::ClockSample::of(&self.core.clock));
            self.stats.perf.game_time(self.core.clock.total_time);
            found = self.stats.hitches.observe(&self.stats.perf);
            // `--perf-report`'s phase and script totals (#1093)
            if let Some(report) = &mut self.stats.report {
                report
                    .totals
                    .add_closed(&self.stats.perf, super::PERF_SKIP_SECS);
            }
            // The hitch log, the exit report's counts and the gate (#1079)
            self.stats
                .watch
                .observe(found, &self.stats.hitches, &self.stats.perf);
        }
        // Frame captures (#1086): a hitch just found, a time reached
        self.frame_capture_triggers(found);
        // `--trace-on-hitch` (#1097), a few frames after it
        self.trace_triggers(found);
        if self
            .launch
            .hold_at
            .is_some_and(|t| self.core.clock.total_time >= t)
            && self.core.clock.frame_step != Some(0.0)
        {
            self.core.clock.frame_step = Some(0.0);
            tracing::info!("game time held at {:.3} s", self.core.clock.total_time);
        }

        // `--stats`: the overlay starts open, and F2 can still close it.
        if std::mem::take(&mut self.capture.show_stats) {
            let view = match self.capture.stats_view {
                super::StatsView::Off => super::StatsView::Full,
                view => view,
            };
            self.set_stats_view(view);
        }

        // Scripted input due by now (`--input-script`), before anything reads it.
        // Keys and the pointer go the way a real keyboard's and mouse's do:
        // egui first (ADR 0082, 0097), then the debug keys (H9) and InputState.
        // Its `expect` and `wait` lines (#714) ask the game as it stands
        // before this frame's step.
        if let Some(script) = &mut self.capture.input_script {
            flint_core::scope!("input_script");
            if self.core.event_log.is_none() && script.has_checks() {
                self.core.event_log = Some(Vec::new());
            }
            let probe = super::input_probe::CoreProbe(&self.core);
            let applied = script.apply_probed(self.core.clock.total_time, &mut self.input, &probe);
            for event in applied.events {
                self.scripted_event(event);
            }
        }

        // The game's frame (ADR 0117 decision 4): the stages, with this
        // window's part in them (`impl FrameHost for PlayerApp` below)
        super::game_core::step(self);

        // Script audio/cursor frame overrides, then per-frame input clear
        {
            flint_core::scope!("finish_frame");
            self.finish_frame();
        }
        self.stats.perf.fixed_steps(self.core.fixed_steps);
        self.perf_mark(Phase::Sync);
    }

    fn run_extension_pre_updates(&mut self) {
        flint_core::scope!("run_extension_pre_updates");
        self.each_extension(|ext, ctx| {
            ext.pre_update(ctx);
            false
        });
    }

    /// `f` for each extension in the order they were added, with an
    /// `ExtensionCtx` over the player's fields, until one returns `true`
    /// (which this returns). Events they fire join the frame's bus. With no
    /// extension it returns at once.
    pub(super) fn each_extension(
        &mut self,
        mut f: impl FnMut(
            &mut dyn crate::extension::PlayerExtension,
            &mut crate::extension::ExtensionCtx,
        ) -> bool,
    ) -> bool {
        if self.extensions.is_empty() {
            return false;
        }
        let mut fired_events = Vec::new();
        let rays = self
            .scene_renderer
            .as_ref()
            .map(|r| r.ray_scene())
            .unwrap_or_default();
        let mut taken = false;
        for ext in &mut self.extensions {
            let gpu = match (&mut self.scene_renderer, &self.render_context) {
                (Some(renderer), Some(context)) => Some(crate::extension::ExtensionGpu {
                    renderer,
                    device: &context.device,
                    queue: &context.queue,
                }),
                _ => None,
            };
            let mut ctx = crate::extension::ExtensionCtx {
                world: &mut self.core.world,
                clock: &self.core.clock,
                input: &self.input,
                camera: &self.camera,
                gpu,
                persist: &mut self.core.store,
                audio: crate::extension::ExtensionAudio::new(&mut self.audio),
                rays: rays.clone(),
                scene_tables: &self.scene.tables,
                fired_events: &mut fired_events,
                cursor_captured: self.focus.cursor_captured,
                script: &mut self.core.script,
            };
            if f(ext.as_mut(), &mut ctx) {
                taken = true;
                break;
            }
        }
        // Onto the frame's bus, so collect_game_events hands them to scripts
        // and audio this frame (or the next, from a later stage).
        for event in fired_events {
            self.core.events.push(event);
        }
        taken
    }

    fn finish_frame(&mut self) {
        // Apply audio low-pass filter override from scripts
        if let Some(cutoff) = self.core.script_requests.audio_lowpass.take() {
            self.audio.set_filter_cutoff(cutoff);
        }

        // A menu put the touch controls away, or play brought them back.
        if let Some(enabled) = self.core.script_requests.touch_controls.take() {
            self.input.touch_controls_mut().set_enabled(enabled);
        }

        // A quality setting moved the distance levers (#56).
        if let Some(renderer) = &mut self.scene_renderer {
            let lod = self.core.script_requests.lod_scale.take();
            let draw = self.core.script_requests.draw_distance_scale.take();
            let mut s = renderer.lod_settings();
            if lod.is_some() || draw.is_some() {
                s.lod_scale = lod.unwrap_or(s.lod_scale);
                s.draw_distance_scale = draw.unwrap_or(s.draw_distance_scale);
                renderer.set_lod_settings(s);
                s = renderer.lod_settings();
            }
            self.core
                .script
                .set_lod_state(s.lod_scale, s.draw_distance_scale);
        }

        // Apply cursor capture request from scripts (skipped while a debug
        // panel is open — the panel needs the mouse).
        if let Some(capture) = self.core.script_requests.cursor_captured.take() {
            #[cfg(feature = "debug-hud")]
            let panel_open = self.debug_panels.iter().any(|p| p.is_open());
            #[cfg(not(feature = "debug-hud"))]
            let panel_open = false;
            if capture && !panel_open && !self.focus.cursor_captured {
                self.capture_cursor();
            } else if !capture && self.focus.cursor_captured {
                self.release_cursor();
            }
        }

        // Debounced save of the persistent store (1 s after the last change,
        // or now if a script called persist_save()).
        self.flush_persistent_store(false, "debounce");

        // Clear per-frame input state
        self.input.end_frame();
    }

    /// The frame-budgeted procgen queue, as the frame runs it (tests).
    #[cfg(all(test, feature = "living"))]
    pub(super) fn tick_procgen(&mut self) {
        use super::game_core::FrameHost;
        let (core, lent) = self.lend();
        core.tick_procgen(lent);
    }

    /// Hot reload of user styles' descriptions (#275, natively): a few
    /// times a second, look for edited or new `styles/*.shader.toml`, and
    /// read the scene's settings again for each style that changed, since
    /// its tables were checked against the old parameters. The WGSL files
    /// are polled by the styles themselves.
    fn poll_user_styles(&mut self) {
        flint_core::scope!("poll_user_styles");
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::sync::atomic::{AtomicU32, Ordering};
            static FRAMES: AtomicU32 = AtomicU32::new(0);
            if FRAMES.fetch_add(1, Ordering::Relaxed) % 15 != 0 {
                return;
            }
            let scope = self.kept_asset_scope();
            let reload = self.core.styles.reload_changed_for(&scope);
            for message in &reload.messages {
                tracing::warn!("{message}");
            }
            if !reload.changed() {
                return;
            }
            // A description read again may take other fields: the scene's
            // components of the styles it touched are checked against the
            // new schema, as `bind_scene` checked them at load (#556)
            let touched: Vec<String> = reload
                .replaced
                .iter()
                .chain(&reload.added)
                .cloned()
                .collect();
            for warning in flint_render::stylize::component_schema::user_style_component_warnings(
                &self.core.world,
                &self.core.styles,
                &scope.project.allow,
                Some(&touched),
            ) {
                tracing::warn!("{warning}");
            }
            // The renderer draws the new installs from the next frame
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.set_styles(self.core.styles.clone());
            }
            // What the scene authors, with --style over it, so a reloaded
            // style a word names keeps the word's settings (#328)
            let authored = self.scene_post_config();
            if let Some(renderer) = &mut self.scene_renderer {
                let mut config = renderer.post_process_config().clone();
                flint_render::stylize::custom::refresh_settings(&mut config, &authored, &reload);
                renderer.set_post_process_config(config);
            }
        }
    }

    fn sync_renderer(&mut self) {
        flint_core::scope!("sync_renderer");
        self.poll_user_styles();

        // The camera is final for the frame: a cut (a teleport, a jump to a
        // new shot) means the picture no longer follows on from the last
        // one, so styles that keep history start over (#343)
        let cut = self.style.camera_cut.observe(
            camera_cut::Shot::of(&self.camera),
            self.core.clock.delta_time as f32,
        );
        if cut {
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.reset_style_history();
            }
        }

        // Particles packed, camera-sorted, for the upload (ADR 0068)
        if self.particles_render_enabled && self.scene_renderer.is_some() {
            self.core.particles.pack(Some(self.camera.position_array()));
        }
        // The subsystems' and the frame's inputs (#739)

        // The frame's inputs, and the world read into the draw lists (#739)
        let ppp = self.renderer_pixels_per_point();
        let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context)
        else {
            return;
        };
        let cam_pos = self.camera.position;
        // The grass bends where the camera stands
        let benders = [GrassEntityPosition {
            position: [cam_pos.x, cam_pos.y, cam_pos.z],
            _pad: 0.0,
        }];
        // A hull on a dynamic body foams with the velocity the solver gave
        // it (#749); the rest have their drawn motion differenced
        let contact_velocities: Vec<_> = self
            .core
            .world
            .entities_with_component(flint_geometry::components::OCEAN_CONTACT)
            .iter()
            .filter_map(|&id| Some((id, self.core.physics.dynamic_velocity(id)?)))
            .collect();
        let mut inputs = FrameInputs {
            // Ocean waves run on the clock scripts see via total_time(),
            // keeping script-side ocean_height() queries in sync with the
            // GPU, and so do held poses (#405), the grass and the post time
            time: self.core.clock.total_time,
            // History styles decay by game time (#342), so a --frame-step
            // capture settles alike at any step
            dt: self.core.clock.delta_time as f32,
            // --style-time holds the post time, as `flint render`'s does (#328)
            look_time: self.launch.style_time.map(f64::from),
            pixels_per_point: ppp,
            view_2d: View2d::of(&self.camera),
            particles: self
                .particles_render_enabled
                .then_some(&self.core.particles.sync),
            world_lines: &self.hud.world_lines,
            style_field: self.hud.style_field.shapes(),
            grass_benders: &benders,
            contact_velocities: &contact_velocities,
            // The deprecated render mode's sea (#746): a frame behind its
            // drain, as the stamp that sets it lands after this
            ocean_tint: self.post.ocean_tint(),
            ..Default::default()
        };
        // What the subsystems draw: the boil, shallow wooze, woozy grass
        let frame = FrameCtx {
            camera: &self.camera,
            boil_on: renderer.post_process_config().boil_on,
        };
        {
            flint_core::scope!("subsystem_frame_inputs");
            self.core.subsystems.frame_inputs(&frame, &mut inputs);
        }
        // A frame being captured keeps what it was drawn with (#1086)
        if self.frame_captures.take_for_frame() {
            self.frame_captures
                .keep_inputs(flint_render::frame_replay::FrameInputsState::of(
                    &inputs,
                    &self.core.world,
                ));
        }
        renderer.prepare(&context.device, &context.queue, &self.core.world, &inputs);
        // Next frame's scripts cast against what this one draws (ADR 0105).
        self.core.script.set_ray_scene(renderer.ray_scene());
    }

    /// Physical pixels per logical point the renderer draws at: the
    /// window's scale factor, which changes when it moves to another
    /// monitor, unless `--pixels-per-point` (#435) pins it. Styles sized in
    /// points (ASCII cells, #265) follow it.
    fn renderer_pixels_per_point(&self) -> f32 {
        self.window.as_ref().map_or(1.0, |window| {
            self.launch
                .pixels_per_point
                .unwrap_or(window.scale_factor() as f32)
        })
    }

    fn drain_post_overrides(&mut self) {
        // Script post-processing overrides for this frame (the commands
        // are in by now: both drains run before this), in place of last
        // frame's, with what a subsystem filled in at the procgen stage (the
        // rhythm ladder's values, ADR 0021). DoF is script-owned (ADR 0027);
        // scene loads reset the sticky config from the def.
        self.post.take_from(&mut self.core.script_requests);
    }

    fn render_hud(&mut self, target_view: &wgpu::TextureView, encoder: &mut wgpu::CommandEncoder) {
        flint_core::scope!("render_hud");
        // The first pass to draw an F2 view lays out what it shows and
        // uploads the glyphs that takes: the pass is timed as a note, so a
        // hitch it makes names the overlay rather than the game's HUD (#1129)
        let opened = self.stats.view.is_open() && self.stats.view != self.stats.drawn_view;
        self.stats.drawn_view = self.stats.view;
        let view = self.stats.view;
        let _overlay = opened.then(|| {
            flint_core::frame_notes::begin(flint_core::frame_notes::NoteKind::StatsOverlay, || {
                match view {
                    super::StatsView::Graph => "opened (graph)",
                    _ => "opened",
                }
                .to_string()
            })
        });
        // Lazy-load any sprite textures referenced by draw commands
        // (must happen before egui_winit borrow)
        self.load_pending_sprites();
        // The F1 card's table (#717), while the subsystems are here to ask
        let key_table = self.hud.key_table_open.then(|| self.debug_key_table());

        let Some(window) = &self.window else { return };
        let Some(context) = &self.render_context else {
            return;
        };
        let Some(egui_winit) = &mut self.hud.egui_winit else {
            return;
        };

        let mut raw_input = egui_winit.take_egui_input(window);
        // Under --frame-step egui runs on the game clock too (#435): its
        // animations (a header opening, a tooltip fading) would otherwise
        // follow the wall clock, and a slow capture frame would land them
        // somewhere else on every run
        if let Some(step) = self.core.clock.frame_step {
            raw_input.time = Some(self.core.clock.total_time);
            raw_input.predicted_dt = step as f32;
        }

        let draw_commands = std::mem::take(&mut self.hud.draw_commands);
        let mut extensions = std::mem::take(&mut self.extensions);
        let mut subsystems = std::mem::take(&mut self.core.subsystems);
        #[cfg(feature = "debug-hud")]
        let mut debug_panels = std::mem::take(&mut self.debug_panels);
        #[cfg(all(feature = "debug-hud", feature = "living"))]
        let view_proj = self.camera.view_projection_matrix();
        let ui_textures = &self.hud.textures;
        let ui_fonts = &self.hud.fonts;
        let ui_fonts_warned = &mut self.hud.fonts_warned;
        let text_halos = &mut self.hud.text_halos;
        text_halos.begin_frame();
        let key_note = &mut self.hud.key_note;

        // World text needs the frame's depth to be drawn against.
        let world_texts = std::mem::take(&mut self.hud.world_texts);
        let text_view = (!world_texts.is_empty()
            && self
                .scene_renderer
                .as_ref()
                .is_some_and(|r| r.frame_depth_ready()))
        .then(|| super::world_text::TextView {
            view_proj: self.camera.view_projection_matrix(),
            eye: self.camera.position_array(),
            screen: [
                context.config.width as f32 / self.hud.egui_ctx.pixels_per_point(),
                context.config.height as f32 / self.hud.egui_ctx.pixels_per_point(),
            ],
            px_per_point: self.hud.egui_ctx.pixels_per_point(),
        });
        let mut laid_text = Vec::new();

        let touch_widgets = self.input.touch_controls().widgets();
        let touch_opacity = self.input.touch_controls().opacity();

        let stats_view = self.stats.view;
        let show_stats = stats_view == super::StatsView::Full;
        // The adapter, and on DX12 its shader compiler (#912)
        let gpu_label = if show_stats {
            flint_render::gpu_select::adapter_short(&context.adapter.get_info())
        } else {
            String::new()
        };
        let stats_data = if show_stats {
            self.scene_renderer.as_ref().map(|r| {
                let mut stats = r.collect_stats();
                // FPS smoothing: rolling window of delta_time samples
                self.stats.frame_times.push_back(self.core.clock.delta_time);
                while self.stats.frame_times.len() > 60 {
                    self.stats.frame_times.pop_front();
                }
                let avg_dt: f64 = self.stats.frame_times.iter().sum::<f64>()
                    / self.stats.frame_times.len().max(1) as f64;
                stats.fps = (1.0 / avg_dt) as f32;
                stats.frame_time_ms = (avg_dt * 1000.0) as f32;
                if let Some(ctx) = &self.render_context {
                    stats.resolution = [ctx.config.width, ctx.config.height];
                    stats.device_memory = flint_render::gpu_counts::device_memory(&ctx.device);
                }
                // A frame that made GPU objects is one frame of the
                // overlay: keep it, so the row can say it (#1091)
                if stats.gpu_calls.created() > 0 {
                    self.stats.last_gpu_objects =
                        Some((self.core.clock.total_time, stats.gpu_calls));
                }
                stats
            })
        } else {
            None
        };
        let last_gpu_objects = self
            .stats
            .last_gpu_objects
            .filter(|_| show_stats)
            .map(|(at, calls)| (self.core.clock.total_time - at, calls));
        // Each style texture's row (#561), only while the full overlay
        // shows: closed, nothing is read or allocated
        let style_target_rows = match (&stats_data, self.scene_renderer.as_ref()) {
            (Some(stats), Some(r)) if stats.style_targets > 0 => {
                super::stats_view::style_target_rows(&r.style_target_entries())
            }
            _ => Vec::new(),
        };
        // The frame-time graph's frames and counters (#1078), while F2
        // shows anything: nothing is read or allocated while it doesn't
        #[cfg(feature = "debug-hud")]
        let clock = &self.core.clock;
        #[cfg(feature = "debug-hud")]
        let mut graph = self.stats.overlay_graph(|| {
            // The display's refresh: the budget line
            clock.vsync_period().or_else(|| {
                let mhz = window.current_monitor()?.refresh_rate_millihertz()?;
                (mhz > 0).then(|| 1000.0 / mhz as f64)
            })
        });
        #[cfg(not(feature = "debug-hud"))]
        let mut graph: Option<OverlayGraph> = None;

        // User styles that don't compile (#275) or don't load (#425), and
        // built-in shaders read from FLINT_STYLE_SHADER_DIR that don't
        // compile (#418): shown until they do
        #[cfg(feature = "debug-hud")]
        let style_errors = {
            let mut errors = self.core.styles.errors();
            errors.extend(flint_render::stylize::dev_shader::errors());
            errors
        };
        // The registry they came from, for the card's "doesn't load" (#742)
        #[cfg(feature = "debug-hud")]
        let style_registry = self.core.styles.clone();
        // Scripts that fail (#693), each once with its count, until the
        // file is saved again or the scene changes
        #[cfg(feature = "debug-hud")]
        let (script_errors, script_errors_unkept) =
            super::script_error_card::rows(self.core.script.errors());

        // A real frame: its texture updates are uploaded below (#944)
        // egui's frame: script UI, the game's, the debug panels (#1096)
        let mut full_output = {
            flint_core::scope!("egui_run");
            #[allow(clippy::disallowed_methods)]
            let full_output = self.hud.egui_ctx.run_ui(raw_input, |ui| {
                let ctx = &ui.ctx().clone();
                // Layers, back to front: script UI, game egui, debug panels.
                // Script UI first: it shares the background layer with any game
                // panels, so issuing it first puts those panels in front of it
                // (see render_draw_commands). Items a script pushed over the
                // UI go on their own Middle layer, above the game's panels and
                // windows and below the debug panels (ADR 0085).
                render_draw_commands(
                    ctx,
                    &draw_commands,
                    ui_textures,
                    ui_fonts,
                    ui_fonts_warned,
                    text_halos,
                );

                // World text is laid out here, so the glyphs it needs go up
                // with this pass's font atlas.
                if let Some(view) = &text_view {
                    laid_text = super::world_text::lay_out(
                        ctx,
                        &world_texts,
                        view,
                        ui_fonts,
                        ui_fonts_warned,
                        text_halos,
                    );
                }

                // The game's own egui, on the root Ui (PlayerExtension::ui,
                // ADR 0121). Its panels sit on the background layer and its
                // windows on Middle, both below the debug panels, which float
                // on Foreground.
                crate::extension::draw_ui(&mut extensions, ui);

                // On-screen touch controls (#133), over the game's UI.
                super::touch_overlay::paint(ctx, &touch_widgets, touch_opacity);

                // The subsystems' own (which preset is where, while two living
                // props presets are compared, #238)
                subsystems.ui(ctx);

                // The pose overlay (#218) and a picked tree's marks and window
                // (#248), in the world and under every panel.
                #[cfg(all(feature = "debug-hud", feature = "living"))]
                for panel in debug_panels.iter_mut() {
                    if let Some(living) = panel
                        .as_any_mut()
                        .downcast_mut::<flint_debug_ui::LivingPropsDebugPanel>()
                    {
                        living.paint_overlay(ctx, &view_proj);
                    }
                }

                #[cfg(feature = "debug-hud")]
                let debug_width = super::debug_panels::show_debug_panels(
                    ctx,
                    &mut debug_panels,
                    &mut |panel, ui| super::debug_panels::draw_panel(panel, &mut extensions, ui),
                );
                #[cfg(not(feature = "debug-hud"))]
                let debug_width = 0.0;

                match (stats_view, stats_data.as_ref()) {
                    (super::StatsView::Full, Some(stats)) => render_stats_overlay(
                        ctx,
                        stats,
                        last_gpu_objects,
                        &style_target_rows,
                        &gpu_label,
                        graph.as_mut(),
                        debug_width,
                    ),
                    (super::StatsView::Graph, _) => {
                        render_graph_overlay(ctx, graph.as_mut(), debug_width)
                    }
                    _ => {}
                }

                // On the web, above the page's wordmark in the same corner
                // (index.html's `.mark`, two lines at bottom 20px; #415)
                // The debug keys' card (F1, #717), and a debug key's note (#367)
                // over everything
                if let Some(table) = &key_table {
                    super::debug_keys::paint_key_table(ctx, table);
                }
                key_note.paint(ctx, 16.0);

                #[cfg(feature = "debug-hud")]
                {
                    let inset = if cfg!(target_arch = "wasm32") {
                        52.0
                    } else {
                        0.0
                    };
                    // The script card in the corner, the style card above it
                    let scripts = flint_debug_ui::paint_script_errors(
                        ctx,
                        &script_errors,
                        script_errors_unkept,
                        inset,
                    );
                    flint_debug_ui::paint_style_errors(
                        ctx,
                        &style_errors,
                        &style_registry,
                        inset + scripts,
                    );
                }
            });
            full_output
        };

        self.hud.draw_commands = draw_commands;
        self.extensions = extensions;
        self.core.subsystems = subsystems;
        #[cfg(feature = "debug-hud")]
        {
            self.debug_panels = debug_panels;
        }
        // Fonts exist from here on; measure_text may now lay text out for real.
        self.hud
            .egui_fonts_ready
            .store(true, std::sync::atomic::Ordering::Relaxed);

        egui_winit.handle_platform_output(window, full_output.platform_output);

        let (paint_jobs, glyphs) = {
            flint_core::scope!("egui_tessellate");
            let paint_jobs = self
                .hud
                .egui_ctx
                .tessellate(full_output.shapes, full_output.pixels_per_point);
            let glyphs = text_view
                .map(|view| super::world_text::tessellate(&self.hud.egui_ctx, laid_text, &view))
                .unwrap_or_default();
            (paint_jobs, glyphs)
        };

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.hud.egui_renderer.take().unwrap();

        // A pass knob may skip the HUD's drawing (#1317); it blends over
        // the frame, so it never repeats
        let ui_times = self.scene_renderer.as_mut().map_or(1, |r| {
            r.pass_times("UI", Some(flint_render::pass_knobs::DRAWS_OVER_THE_FRAME))
        });
        // The HUD's GPU time, with the renderer's phases (ADR 0108), and its
        // debug group for GPU debuggers (#1083).
        let ui_scope = (ui_times > 0).then(|| {
            flint_render::gpu_labels::begin(
                self.scene_renderer.as_mut().and_then(|r| r.gpu_timer_mut()),
                encoder,
                "UI",
            )
        });

        {
            flint_core::scope!("egui_upload", paint_jobs.len());
            for (id, deltas) in &full_output.textures_delta.set {
                for image_delta in deltas {
                    egui_renderer.update_texture(&context.device, &context.queue, *id, image_delta);
                }
            }

            egui_renderer.update_buffers(
                &context.device,
                &context.queue,
                encoder,
                &paint_jobs,
                &screen_descriptor,
            );
        }

        // World text goes under the HUD, against the scene's depth.
        if !glyphs.is_empty() && ui_scope.is_some() {
            let pipeline = self.hud.world_text_pipeline.get_or_insert_with(|| {
                super::world_text::WorldTextPipeline::new(&context.device, context.hud_format)
            });
            pipeline.draw(
                &context.device,
                &context.queue,
                encoder,
                target_view,
                &context.depth_view,
                &glyphs,
                &egui_renderer,
            );
        }

        if ui_scope.is_some() {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HUD Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load, // overlay on top of 3D
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            let mut render_pass = render_pass.forget_lifetime();
            flint_core::scope!("egui_encode");
            egui_renderer.render(&mut render_pass, &paint_jobs, &screen_descriptor);
        }

        if let Some(ui_scope) = ui_scope {
            flint_render::gpu_labels::end(
                self.scene_renderer.as_mut().and_then(|r| r.gpu_timer_mut()),
                encoder,
                ui_scope,
            );
        }

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }
        // Applied above: egui 0.36 asserts when a delta drops unhandled (#944)
        full_output.textures_delta.clear();

        self.hud.egui_renderer = Some(egui_renderer);
    }

    /// The Ocean Debug panel's CPU/GPU parity probe, when its box is
    /// ticked (#749): a cross pinned to the CPU's wave height ahead of the
    /// camera, on the clock the renderer's waves run on.
    #[cfg(feature = "debug-hud")]
    fn add_ocean_probe_lines(&mut self) {
        let time = self.core.clock.total_time;
        let camera_pos = self.camera.position_array();
        let forward = self.camera.forward_vector();
        for panel in &mut self.debug_panels {
            if panel.name() != "Ocean Debug" {
                continue;
            }
            let Some(ocean_panel) = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::OceanDebugPanel>()
            else {
                continue;
            };
            if !ocean_panel.probe_shown() {
                continue;
            }
            let Some(id) = self.core.world.get_id(ocean_panel.entity_name()) else {
                continue;
            };
            let ocean = self.core.world.view(id, flint_geometry::components::OCEAN);
            if !ocean.exists() {
                continue;
            }
            let lines = ocean_panel.probe_lines(&ocean, time, camera_pos, forward);
            self.hud.world_lines.extend(lines);
        }
    }
}
/// The player's part in the game's frame (ADR 0117 decision 4): what
/// `game_core::step` calls back, in the order it does.
impl super::game_core::FrameHost for PlayerApp {
    fn lend(&mut self) -> (&mut super::game_core::GameCore, super::game_core::Lent<'_>) {
        (
            &mut self.core,
            super::game_core::Lent {
                input: &mut self.input,
                camera: &mut self.camera,
                renderer: self.scene_renderer.as_mut(),
                gpu: self.render_context.as_ref().map(|c| (&c.device, &c.queue)),
                pixels_per_point: self.hud.egui_ctx.pixels_per_point(),
                cursor_captured: self.focus.cursor_captured,
                scene_file_writable: self.launch.scene_file_writable,
                note: &mut self.hud.key_note,
                #[cfg(feature = "debug-hud")]
                panels: &mut self.debug_panels,
                audio: Some(&mut self.audio.engine),
                pads: Some(super::subsystem::Pads {
                    gilrs: &mut self.gilrs,
                    rumble: &mut self.pad_rumble,
                    lent: &mut self.pads_lent,
                }),
                quit: Some(&mut self.outcome.exit_requested),
            },
        )
    }

    fn core(&mut self) -> &mut super::game_core::GameCore {
        &mut self.core
    }

    fn before_events(&mut self) {
        // Advance transition phase timing
        {
            flint_core::scope!("advance_transition");
            self.advance_transition();
        }
        // Game extensions, before anything below reads the world
        self.run_extension_pre_updates();
    }

    fn extensions_at(&mut self, stage: super::subsystem::Stage) {
        self.each_extension(|ext, ctx| {
            ext.update(stage, ctx);
            false
        });
    }

    fn extension_takes(&mut self, command: &flint_script::context::ScriptCommand) -> bool {
        self.each_extension(|ext, ctx| ext.script_command(command, ctx))
    }

    fn phase_done(&mut self, phase: super::perf::Phase) {
        self.perf_mark(phase);
        // Every phase can call scripts (on_update, on_draw_ui, animation
        // ends, scene changes): what each called is this frame's (#703)
        // Empty, and no allocation, unless script timing is on (`--perf`)
        let times = self.core.script.take_times();
        if !times.is_empty() {
            self.stats.perf.record_scripts(&times);
        }
        // ... and inside each callback, its functions (#1100)
        let functions = self.core.script.take_function_times();
        if !functions.is_empty() {
            self.stats.perf.record_functions(&functions);
        }
    }

    fn window_for_scripts(&mut self) {
        // Transition state for script access
        let script = &mut self.core.script;
        match &self.transition_phase {
            TransitionPhase::Idle => {
                script.set_transition_state(-1.0, "idle");
            }
            TransitionPhase::Exiting { elapsed, .. } => {
                script.set_transition_state(*elapsed as f64, "exiting");
            }
            TransitionPhase::Loading { .. } => {
                script.set_transition_state(1.0, "loading");
            }
            TransitionPhase::Entering { elapsed } => {
                script.set_transition_state(*elapsed as f64, "entering");
            }
        }
        // The look drawn now (#497): F4's pick and a script's set_look
        // already said so; this hears a game that set it by hand
        self.publish_look();
        let (screen_w, screen_h) = self.script_screen_size();
        let script = &mut self.core.script;
        script.set_screen_size(screen_w, screen_h);
        // Cursor in the same logical-point space as screen_width()/draw_*:
        // InputState holds physical pixels, egui's ppp is the window scale.
        {
            let ppp = self.hud.egui_ctx.pixels_per_point().max(0.01);
            let (mx, my) = self.input.mouse_position;
            script.set_mouse_position(mx as f32 / ppp, my as f32 / ppp);
        }
        script.set_cursor_captured(self.focus.cursor_captured);

        // Sync loaded chunk IDs so scripts can query is_chunk_loaded()
        let chunk_ids: HashSet<String> = self.loaded_chunks.keys().cloned().collect();
        script.set_loaded_chunk_ids(chunk_ids);

        // Where each audio_source is in its sound (#658), read every frame,
        // paused or not: loops play on under a pause menu.
        self.audio.sample_positions();
        script.set_audio_clock(self.audio.sync.positions(), self.audio.output_latency());
        // And which of the sounds scripts started still play (#1000): a
        // command from last frame's batch is in the engine by now
        script.set_sounds_playing(self.audio.engine.playing_keys());
        // The music clock and the conducted snapshot are the music
        // subsystem's, set at `Stage::PreScripts`, just before this
    }

    fn listener(&mut self, at: super::game_core::Listener) {
        match at {
            super::game_core::Listener::FirstPerson(position, yaw, pitch) => {
                self.audio.update_listener(position, yaw, pitch);
            }
            // A script-driven camera (chase cam, etc.)
            super::game_core::Listener::ScriptCamera => {
                let cam_pos = self.camera.position;
                let dir = flint_core::Vec3::new(
                    self.camera.target.x - cam_pos.x,
                    self.camera.target.y - cam_pos.y,
                    self.camera.target.z - cam_pos.z,
                );
                let yaw = dir.x.atan2(dir.z);
                let horiz = (dir.x * dir.x + dir.z * dir.z).sqrt();
                let pitch = (-dir.y).atan2(horiz);
                self.audio.update_listener(cam_pos, yaw, pitch);
            }
        }
    }

    fn host_command(&mut self, command: flint_script::context::ScriptCommand) {
        flint_script::host::apply(self, command);
    }

    fn script_ui_drawn(&mut self) {
        flint_core::scope!("collect_script_ui");
        // Collect draw commands for this frame (scripts + data-driven UI)
        let mut commands = self.core.script.drain_draw_commands();
        let (screen_w, screen_h) = self.script_screen_size();
        let ui_commands = self
            .core
            .script
            .generate_ui_draw_commands(screen_w, screen_h);
        commands.extend(ui_commands);
        self.hud.draw_commands = commands;

        // Depth-tested world lines (ADR 0083) render with this frame's
        // scene, handed over with its inputs
        self.hud.world_lines = self.core.script.drain_world_lines();
        // The style field (#859), on_update's shapes and on_draw_ui's
        self.hud.style_field = self.core.script.drain_style_field();
        // The Ocean Debug panel's parity probe draws with them (#749)
        #[cfg(feature = "debug-hud")]
        self.add_ocean_probe_lines();
        // World text (ADR 0104) is laid out in the HUD's egui pass.
        self.hud.world_texts = self.core.script.drain_world_texts();
    }

    fn sound(&mut self, config: &StateConfig, game_events: &[flint_runtime::GameEvent]) {
        flint_core::scope!("sound");
        // A late bundle (audio, #98) landed: queue what wasn't there at
        // start. Already-loaded sounds are skipped, and an autoplay source
        // that was waiting for its file starts on the update after its
        // sound is decoded.
        let generation = flint_core::vfs::generation();
        if generation != self.audio_vfs_generation {
            self.audio_vfs_generation = generation;
            super::scene_loading::load_audio_from_world(
                &self.core.world,
                &mut self.audio,
                &self.core.project,
                &self.core.scene_path,
                self.scene.preload_audio,
                true,
            );
        }
        // ...and decode it a few milliseconds a frame (#124).
        self.audio.engine.decode_queued(AUDIO_DECODE_BUDGET);

        // Audio triggers from game events (skip when paused)
        if config.runs(AUDIO) {
            self.audio.process_events(game_events, &self.core.world);
            self.audio
                .update(&mut self.core.world, self.core.clock.delta_time)
                .ok();
        }
    }

    fn late_models(&mut self) {
        self.load_late_models();
    }

    fn bones_posed(&mut self) {
        // Push skeletal bone matrices to GPU
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            flint_core::scope!("upload_bones", self.skeletal_entity_assets.len());
            // The clock poses held on twos tick by (#405), before the bones
            // go up: the same seconds the post time reads, so a held pose
            // and a style's boil turn on the same frame
            renderer.set_motion_time(self.core.clock.total_time);
            // ...and the emitters held entities carry (#873), which step
            // right after this, on the same clock and whole-frame rate
            let sync = &mut self.core.particles.sync;
            sync.set_motion_time(self.core.clock.total_time);
            sync.set_motion_hold(renderer.effective_post_process_config().motion_hold);
            for (entity_id, asset_name) in &self.skeletal_entity_assets {
                if let Some(matrices) = self.core.animation.bone_matrices(entity_id) {
                    renderer.update_bone_matrices(
                        &context.device,
                        &context.queue,
                        *entity_id,
                        asset_name,
                        matrices,
                    );
                }
            }
        }
    }

    fn sync(&mut self) {
        // Renderer sync: transforms, particle upload, grass/ocean time
        self.sync_renderer();

        // Script post-override drain (a subsystem's share is in by now)
        flint_core::scope!("post_overrides_and_quality");
        self.drain_post_overrides();

        // A script's set_quality (FlintKart#55), applied here, between the
        // frame's scripts and its draw
        self.apply_script_quality();
    }
}

use super::stats_view::OverlayGraph;

/// The full overlay's graph and the compact one's, in points.
#[cfg(feature = "debug-hud")]
const GRAPH_FULL: egui::Vec2 = egui::vec2(360.0, 72.0);
#[cfg(feature = "debug-hud")]
const GRAPH_COMPACT: egui::Vec2 = egui::vec2(280.0, 40.0);

/// The F2 overlay's panel: dark, a hairline edge, monospace.
fn overlay_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 209))
        .stroke(egui::Stroke::new(
            1.0f32,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 38),
        ))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(10))
}

/// The overlay's corner, beside the debug columns rather than over them.
fn overlay_area(id: &str, right_inset: f32) -> egui::Area {
    egui::Area::new(egui::Id::new(id))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0 - right_inset, 8.0))
        .order(egui::Order::Foreground)
        .interactable(false)
}

/// The graph, its counters and, in the full form, the bands' legend.
#[cfg(feature = "debug-hud")]
fn show_graph(ui: &mut egui::Ui, g: &mut OverlayGraph, full: bool) {
    use flint_debug_ui::widgets::{band_legend, hitch_counters, GraphLayout};
    let layout = GraphLayout {
        size: if full { GRAPH_FULL } else { GRAPH_COMPACT },
        window_secs: if full {
            super::stats_view::GRAPH_SECS
        } else {
            super::stats_view::COMPACT_SECS
        },
        budget_ms: g.budget_ms,
        fps_labels: full,
    };
    g.graph.widget.show(ui, &g.graph.frames, layout);
    ui.add_space(3.0);
    ui.scope(|ui| {
        let size = if full { 10.0 } else { 9.5 };
        ui.style_mut().override_font_id = Some(egui::FontId::monospace(size));
        ui.set_max_width(layout.size.x);
        hitch_counters(ui, &g.graph.counters(g.hitches), !full);
        if full {
            ui.style_mut().override_font_id = Some(egui::FontId::monospace(8.5));
            band_legend(ui);
        }
    });
}

/// F2's compact form (`--stats=graph`): the graph and its counters alone.
fn render_graph_overlay(ctx: &egui::Context, graph: Option<&mut OverlayGraph>, right_inset: f32) {
    let Some(graph) = graph else { return };
    overlay_area("render_stats_graph", right_inset).show(ctx, |ui| {
        overlay_frame()
            .inner_margin(egui::Margin::same(7))
            .show(ui, |ui| {
                #[cfg(feature = "debug-hud")]
                show_graph(ui, graph, false);
                #[cfg(not(feature = "debug-hud"))]
                let _ = (ui, graph);
            });
    });
}

/// `right_inset`: width the debug columns take at the right edge, so the
/// overlay sits beside them rather than over them. `style_targets`: a row
/// for each of the style chain's textures (`stats_view::style_target_rows`).
fn render_stats_overlay(
    ctx: &egui::Context,
    stats: &flint_render::RenderStats,
    last_gpu_objects: Option<(f64, flint_render::gpu_counts::GpuCalls)>,
    style_targets: &[String],
    gpu: &str,
    graph: Option<&mut OverlayGraph>,
    right_inset: f32,
) {
    use flint_render::format_count;

    overlay_area("render_stats_overlay", right_inset).show(ctx, |ui| {
        overlay_frame().show(ui, |ui| {
            ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
            ui.set_min_width(180.0);

            // Header
            ui.colored_label(
                egui::Color32::from_gray(136),
                egui::RichText::new("RENDERING STATS").size(9.0),
            );
            ui.separator();

            // Frame times, a bar a frame, and the hitches (#1078)
            #[cfg(feature = "debug-hud")]
            if let Some(graph) = graph {
                show_graph(ui, graph, true);
                ui.separator();
            }
            #[cfg(not(feature = "debug-hud"))]
            let _ = graph;

            // Core metrics
            ui.horizontal(|ui| {
                ui.label("FPS:");
                ui.colored_label(
                    egui::Color32::from_rgb(74, 222, 128),
                    format!("{:.0}", stats.fps),
                );
                ui.colored_label(
                    egui::Color32::from_gray(102),
                    format!("({:.1}ms)", stats.frame_time_ms),
                );
            });
            ui.label(format!("Draw Calls: {}", stats.draw_calls));
            ui.label(format!("Triangles: {}", format_count(stats.triangles)));

            ui.separator();

            // GPU objects created this frame (#1091): zero on a steady
            // frame, amber when not, and the last frame that made any
            for row in super::stats_view::gpu_alloc_rows(stats, last_gpu_objects) {
                let color = match row.tone {
                    super::stats_view::Tone::Calm => egui::Color32::from_gray(200),
                    super::stats_view::Tone::Quiet => egui::Color32::from_gray(136),
                    super::stats_view::Tone::Alert => egui::Color32::from_rgb(245, 166, 35),
                };
                ui.colored_label(color, row.text);
            }

            ui.separator();

            // Breakdown
            if stats.culled_draws > 0 {
                ui.label(format!(
                    "Entities: {} ({} culled)",
                    stats.entity_draws, stats.culled_draws
                ));
            } else {
                ui.label(format!("Entities: {}", stats.entity_draws));
            }
            if stats.skinned_draws > 0 {
                ui.label(format!("Skinned: {}", stats.skinned_draws));
            }
            ui.label(format!(
                "Terrain: {}/{} chunks",
                stats.terrain_draws, stats.terrain_total_chunks
            ));
            if stats.transparent_draws > 0 {
                ui.label(format!("Transparent: {}", stats.transparent_draws));
            }
            if stats.billboard_draws > 0 {
                ui.label(format!("Billboards: {}", stats.billboard_draws));
            }
            if stats.particle_draws > 0 {
                ui.label(format!(
                    "Particles: {} ({} inst)",
                    stats.particle_draws,
                    format_count(stats.particle_instances)
                ));
            }
            if stats.sprite_batches > 0 {
                ui.label(format!("Sprites: {}", stats.sprite_batches));
            }
            if stats.grass_instances > 0 {
                ui.label(format!(
                    "Grass: {} inst",
                    format_count(stats.grass_instances)
                ));
            }
            // Style chain memory (#302): every target, and how
            // many are full output size (FXAA's included)
            if stats.style_targets > 0 {
                ui.label(format!(
                    "Style targets: {} ({:.1} MB, {} full-size)",
                    stats.style_targets,
                    stats.style_target_bytes as f64 / (1024.0 * 1024.0),
                    stats.full_size_targets
                ));
                // and each of them, with its size and MB (#561)
                for row in style_targets {
                    ui.colored_label(egui::Color32::from_gray(170), row);
                }
            }

            ui.separator();

            // Shadow pass
            ui.label(format!("Shadow Calls: {}", stats.shadow_draw_calls));
            ui.label(format!(
                "Shadow Tris: {}",
                format_count(stats.shadow_triangles)
            ));

            ui.separator();

            // What draws: the adapter, its backend and, on DX12,
            // the shader compiler (#912)
            ui.colored_label(egui::Color32::from_gray(170), gpu);

            // GPU time per phase (ADR 0108), where the device can time
            if let (true, Some(why)) = (!stats.gpu_passes.is_empty(), stats.gpu_timestamps.why()) {
                // Zeros that look like an answer are worse than none (#1316)
                ui.label(flint_render::TIMESTAMPS_UNAVAILABLE);
                ui.colored_label(egui::Color32::from_gray(170), format!("  {why}"));
                ui.separator();
            } else if !stats.gpu_passes.is_empty() {
                ui.label(format!("GPU: {:.2}ms", stats.gpu_frame_ms));
                for pass in &stats.gpu_passes {
                    ui.colored_label(
                        egui::Color32::from_gray(170),
                        format!("  {:<13}{:>6.2}", pass.label, pass.ms),
                    );
                }
                ui.separator();
            }

            // Resolution, and the scene's below render scale 1 (#122)
            let scene = stats.scene_resolution;
            let resolution = if scene == stats.resolution || scene == [0, 0] {
                format!("{}x{}", stats.resolution[0], stats.resolution[1])
            } else {
                format!(
                    "{}x{}, scene {}x{}",
                    stats.resolution[0], stats.resolution[1], scene[0], scene[1]
                )
            };
            ui.colored_label(egui::Color32::from_gray(102), resolution);
        });
    });
}

/// `hud.png` + 3 -> `hud_0003.png`; extension-less paths get `.png`.
pub(crate) fn numbered_capture_path(base: &std::path::Path, n: u32) -> std::path::PathBuf {
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "frame".to_string());
    let ext = base
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "png".to_string());
    base.with_file_name(format!("{stem}_{n:04}.{ext}"))
}

#[cfg(test)]
mod capture_tests {
    use super::numbered_capture_path;
    use std::path::Path;

    #[test]
    fn numbered_path_keeps_dir_and_extension() {
        let p = numbered_capture_path(Path::new("renders/hud.png"), 7);
        assert_eq!(p, Path::new("renders/hud_0007.png"));
    }

    #[test]
    fn numbered_path_defaults_extension() {
        let p = numbered_capture_path(Path::new("shot"), 12);
        assert_eq!(p, Path::new("shot_0012.png"));
    }
}
