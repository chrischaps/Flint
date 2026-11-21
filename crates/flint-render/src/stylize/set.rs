//! The built styles and the chain that runs them: [`StyleSet`], the
//! ping-pong plans and `run_stage` (moved from `stylize/mod.rs`, #742).

use super::*;

// ---------------------------------------------------------------------------
// The set of built styles
// ---------------------------------------------------------------------------

/// One style's built pass in a [`StyleSet`].
pub(super) struct BuiltSlot {
    /// The install it was built (or failed) from (#742): a user style whose
    /// `.shader.toml` is edited is a new [`Style`] of the same name
    /// ([`StyleRegistry::reload_changed_for`], #275), and a different
    /// generation here throws the old pass away and builds again. Holding
    /// the style keeps what the pass was built from alive while it is.
    pub(super) from: Style,
    pub(super) built: Option<Box<dyn StylePass>>,
    /// Its creation panicked; never retried for this install.
    pub(super) failed: bool,
    /// The shader files its build read from `FLINT_STYLE_SHADER_DIR`
    /// (#418, [`dev_shader`]); empty when that's off. A slot whose file
    /// moves is built again.
    pub(super) files: Vec<std::path::PathBuf>,
}

/// What a build hands back: the pass, or its panic, and the shader files
/// it read under `FLINT_STYLE_SHADER_DIR` ([`dev_shader::build`]).
type Made<T> = (std::thread::Result<T>, Vec<std::path::PathBuf>);

/// A build running on a worker thread (#1121), which sends what it made
/// once. A worker that couldn't start, or died, hangs up instead: the
/// next [`StyleSet::ensure`] that needs it then builds it itself.
type Arrival<T> = std::sync::mpsc::Receiver<Made<T>>;

/// A style being built ahead of its first frame (#1121), by
/// [`StyleSet::prepare`].
pub(super) struct Preparing {
    /// The install it is built from: another by the time it arrives, and
    /// the pass is thrown away.
    pub(super) from: Style,
    pub(super) arrival: Arrival<Box<dyn StylePass>>,
}

/// Run `build` on a thread of its own and hand back where its result
/// arrives. wgpu creates pipelines from any thread on one `Device`, and
/// the frame goes on drawing meanwhile. On the web there are no threads:
/// it builds now, so a game that prepares at a quiet moment pays there.
fn off_thread<T: Send + 'static>(
    what: &str,
    build: impl FnOnce() -> Made<T> + Send + 'static,
) -> Arrival<T> {
    let (sent, arrival) = std::sync::mpsc::channel();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let started = std::thread::Builder::new()
            .name(format!("style build {what}"))
            .spawn(move || {
                // Nobody waiting any more (the renderer went) is fine
                let _ = sent.send(build());
            });
        if let Err(e) = started {
            tracing::warn!(
                "style {what}: no thread to build it ahead on ({e}); it builds when drawn"
            );
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = what;
        let _ = sent.send(build());
    }
    arrival
}

/// What arrived: `Some` once the worker has sent, `None` while it builds
/// (unless `wait`) or after it hung up without sending.
fn arrived<T>(arrival: &Arrival<T>, wait: bool) -> Option<Result<Made<T>, ()>> {
    use std::sync::mpsc::TryRecvError;
    if wait {
        return Some(arrival.recv().map_err(|_| ()));
    }
    match arrival.try_recv() {
        Ok(made) => Some(Ok(made)),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err(())),
    }
}

/// Build `style` at `format`: its `create` under its pipeline cache scope
/// (#911), its shader files recorded (#418), a panic caught. The one body
/// [`StyleSet::ensure`] and [`StyleSet::prepare`]'s workers both run.
fn build_style(
    device: &wgpu::Device,
    style: &Style,
    format: wgpu::TextureFormat,
) -> Made<Box<dyn StylePass>> {
    let descriptor = style.descriptor();
    dev_shader::build(descriptor.name, || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // One install at one format builds the same layouts every
            // time: what lets its pipelines be kept (#911)
            let scope = (descriptor.name, descriptor.generation, format);
            crate::pipeline_cache::in_scope(scope, || {
                (descriptor.create)(&StyleDevice {
                    device,
                    descriptor,
                    style,
                    format,
                })
            })
        }))
    })
}

/// The format a style of `descriptor`'s stage draws in.
fn stage_format(descriptor: &StyleDescriptor, surface: wgpu::TextureFormat) -> wgpu::TextureFormat {
    match descriptor.stage {
        StyleStage::Hdr => crate::postprocess::HDR_FORMAT,
        StyleStage::Display => surface,
    }
}

/// Put what a build made into its slot: the pass, or the failure, which
/// goes onto `failed` for the renderer to say and switch off.
fn install(
    slot: &mut BuiltSlot,
    made: std::thread::Result<Box<dyn StylePass>>,
    files: Vec<std::path::PathBuf>,
    text_capture: bool,
    failed: &mut Vec<Style>,
) {
    slot.files = files;
    match made {
        Ok(pass) => {
            pass.set_text_capture(text_capture);
            slot.built = Some(pass);
        }
        Err(_) => {
            slot.failed = true;
            failed.push(slot.from.clone());
        }
    }
}

fn build_ownership(device: &wgpu::Device) -> Made<ownership::OwnershipPass> {
    dev_shader::build("ownership", || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ownership::OwnershipPass::new(device)
        }))
    })
}

fn build_edges(device: &wgpu::Device) -> Made<edges::EdgePass> {
    dev_shader::build("edges", || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::pipeline_cache::in_scope("Style Edges", || edges::EdgePass::new(device))
        }))
    })
}

fn build_fade(device: &wgpu::Device) -> Made<fade::FadePass> {
    dev_shader::build("fade", || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fade::FadePass::new(device)))
    })
}

/// The styles whose pipelines exist, by name. Lives on the post pipeline;
/// nothing in it depends on the frame's size.
#[derive(Default)]
pub struct StyleSet {
    pub(super) slots: std::collections::HashMap<&'static str, BuiltSlot>,
    /// Styles building on worker threads (#1121), by name.
    pub(super) preparing: std::collections::HashMap<&'static str, Preparing>,
    /// The shared passes building on worker threads (#1121).
    pub(super) edges_preparing: Option<Arrival<edges::EdgePass>>,
    pub(super) fade_preparing: Option<Arrival<fade::FadePass>>,
    pub(super) ownership_preparing: Option<Arrival<ownership::OwnershipPass>>,
    /// The shared edge pass (#301), built the first time a style needs it.
    pub(super) edges: Option<edges::EdgePass>,
    /// The edge pass's creation panicked; the styles that need it draw
    /// without (their [`StyleFrame::edges`] is `None`).
    pub(super) edges_failed: bool,
    /// The shared cross-fade (#396), built the first time a style that
    /// gets it is on.
    pub(super) fade: Option<fade::FadePass>,
    /// Its creation panicked: styles draw at full blend.
    pub(super) fade_failed: bool,
    /// How many times the edge pass has been drawn, for the test that a
    /// frame draws it once however many styles read it.
    pub(super) edge_runs: std::sync::atomic::AtomicU64,
    /// Whether glyph styles keep their frame as text (#317), handed to
    /// each pass when it's built.
    pub(super) text_capture: bool,
    /// The draw that shows a style's `blend` (#369), any style's. Shared
    /// so [`prepare`](Self::prepare)'s worker can build its pipelines
    /// ahead (#1159).
    pub(super) blend: std::sync::Arc<blend::BlendPass>,
    /// The draws a prepare has sent a worker to build, by format (#1159),
    /// so the next asks only for the rest without touching the pass's
    /// locks. Forgotten with the pass.
    pub(super) blend_asked: Vec<(wgpu::TextureFormat, blend::Draws)>,
    /// The ownership pass (#384), built with the first style.
    pub(super) ownership: Option<ownership::OwnershipPass>,
    pub(super) ownership_failed: bool,
    /// The shader files the shared passes (edges, cross-fade, ownership).
    pub(super) shared_files: Vec<std::path::PathBuf>,
    /// Calls to [`ensure`](Self::ensure) since the files were last looked at.
    pub(super) dev_polls: u32,
    /// Something was built again from changed files since
    /// [`take_rebuilt`](Self::take_rebuilt) last asked.
    pub(super) rebuilt: bool,
    /// History taken from a capture (#1136) for styles not built yet,
    /// handed to each at the [`ensure`](Self::ensure) that builds it.
    pub(super) pending_history: Vec<history::ImportedHistory>,
    /// What the styles said of the history handed to them.
    pub(super) history_notes: history::Notes,
}

impl StyleSet {
    /// The glyph atlases the built styles hold, bytes (#1091).
    pub fn glyph_atlas_bytes(&self) -> u64 {
        self.slots
            .values()
            .filter_map(|s| s.built.as_ref())
            .map(|p| p.glyph_atlas_bytes())
            .sum()
    }

    /// The draw every stage runs after a style whose `blend` is under 1
    /// (#369); hand it to [`run_stage`].
    pub(crate) fn blend_pass(&self) -> &blend::BlendPass {
        &self.blend
    }

    /// Build every style `config` switches on that isn't built yet.
    /// Returns the ones whose creation panicked (they stay off for the
    /// rest of the run).
    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        config: &PostProcessConfig,
        ids: &StyleIds,
        styles: &StyleRegistry,
    ) -> Vec<Style> {
        // A style no longer registered (another scene's, an old install)
        // lets go of its pass and of what it was built from
        self.slots
            .retain(|name, slot| styles.style(name).is_some_and(|s| s.same(&slot.from)));
        self.preparing
            .retain(|name, p| styles.style(name).is_some_and(|s| s.same(&p.from)));
        self.reload_changed_shaders();
        let mut failed = Vec::new();
        // What the workers finished since the last frame goes in first
        // (#1121), so a style prepared in time costs this frame nothing
        let names: Vec<&'static str> = self.preparing.keys().copied().collect();
        for name in names {
            self.collect_prepared(name, false, &mut failed);
        }
        for style in styles.styles() {
            let descriptor = style.descriptor();
            let slot = self
                .slots
                .entry(descriptor.name)
                .or_insert_with(|| BuiltSlot {
                    from: style.clone(),
                    built: None,
                    failed: false,
                    files: Vec::new(),
                });
            if slot.built.is_some() || slot.failed || !descriptor.is_on(config, ids) {
                continue;
            }
            // Still building ahead: waiting for it is never longer than
            // building it again here
            if self.preparing.contains_key(descriptor.name) {
                let waited = flint_core::frame_notes::timed(
                    flint_core::frame_notes::NoteKind::StyleBuild,
                    || format!("{} (waited for its prepare)", descriptor.name),
                    || self.collect_prepared(descriptor.name, true, &mut failed),
                );
                if waited {
                    continue;
                }
            }
            let Some(slot) = self.slots.get_mut(descriptor.name) else {
                continue;
            };
            let format = stage_format(descriptor, surface_format);
            // A frame note (#1077): a style switched on mid-game builds on
            // the frame that first draws it, and that frame says so
            let noted = || {
                if !style.is_user() {
                    descriptor.name.to_string()
                } else {
                    format!("{} (user style)", descriptor.name)
                }
            };
            let (made, files) = flint_core::frame_notes::timed(
                flint_core::frame_notes::NoteKind::StyleBuild,
                noted,
                || build_style(device, style, format),
            );
            install(slot, made, files, self.text_capture, &mut failed);
        }
        if self.ownership.is_none()
            && !self.ownership_failed
            && self.slots.values().any(|s| s.built.is_some())
        {
            let (made, files) = match self.ownership_preparing.take() {
                Some(arrival) => match arrived(&arrival, true) {
                    Some(Ok(made)) => made,
                    _ => build_ownership(device),
                },
                None => build_ownership(device),
            };
            self.shared_files.extend(files);
            match made {
                Ok(pass) => self.ownership = Some(pass),
                Err(_) => {
                    tracing::warn!(
                        "style ownership unavailable: its shader failed to build; whole-frame \
                         styles draw over other styles' entities"
                    );
                    self.ownership_failed = true;
                }
            }
        }
        if self.edges.is_none()
            && !self.edges_failed
            && styles
                .iter()
                .any(|d| d.needs_edges() && d.is_on(config, ids))
        {
            let (made, files) = match self.edges_preparing.take() {
                Some(arrival) => match arrived(&arrival, true) {
                    Some(Ok(made)) => made,
                    _ => build_edges(device),
                },
                None => build_edges(device),
            };
            self.shared_files.extend(files);
            match made {
                Ok(pass) => self.edges = Some(pass),
                Err(_) => {
                    tracing::warn!(
                        "shared style edges unavailable: the edge shader failed to build"
                    );
                    self.edges_failed = true;
                }
            }
        }
        if self.fade.is_none()
            && !self.fade_failed
            && styles
                .iter()
                .any(|d| d.is_on(config, ids) && d.framework_fades(config, ids))
        {
            let (made, files) = match self.fade_preparing.take() {
                Some(arrival) => match arrived(&arrival, true) {
                    Some(Ok(made)) => made,
                    _ => build_fade(device),
                },
                None => build_fade(device),
            };
            self.shared_files.extend(files);
            match made {
                Ok(pass) => self.fade = Some(pass),
                Err(_) => {
                    tracing::warn!("style cross-fade unavailable: its shader failed to build");
                    self.fade_failed = true;
                }
            }
        }
        self.hand_over_history();
        failed
    }

    /// Each built style's frame-to-frame history after the frame numbered
    /// `frame` (#1136, [`history`]), in name order: their states, and the
    /// copies staged in `encoder`.
    pub(crate) fn export_history(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: u64,
    ) -> (Vec<history::StyleHistoryState>, Vec<history::StagedHistory>) {
        let mut names: Vec<&&'static str> = self.slots.keys().collect();
        names.sort();
        let (mut states, mut staged) = (Vec::new(), Vec::new());
        for name in names {
            let Some(pass) = self.slots[*name].built.as_deref() else {
                continue;
            };
            let mut out = history::HistoryExport::new(device, encoder, name, frame);
            pass.export_history(&mut out);
            let (state, copies) = out.finish();
            states.extend(state);
            staged.extend(copies);
        }
        (states, staged)
    }

    /// Hand `histories` (taken from a capture, #1136) to their styles: each
    /// keeps its own until the next frame it draws. A style not built yet
    /// gets its own when it is.
    pub(crate) fn import_history(&mut self, histories: Vec<history::ImportedHistory>) {
        for mut h in histories {
            h.notes = self.history_notes.clone();
            self.pending_history
                .retain(|p| p.state.style != h.state.style);
            self.pending_history.push(h);
        }
        self.hand_over_history();
    }

    fn hand_over_history(&mut self) {
        if self.pending_history.is_empty() {
            return;
        }
        let slots = &self.slots;
        self.pending_history.retain(|h| {
            match slots
                .get(h.state.style.as_str())
                .and_then(|s| s.built.as_deref())
            {
                Some(pass) => {
                    pass.import_history(h.clone());
                    false
                }
                None => true,
            }
        });
    }

    /// What the styles said of the history handed to them since the last
    /// call, and a line for each history no style drawn took.
    pub(crate) fn take_history_notes(&mut self) -> Vec<String> {
        let mut said =
            std::mem::take(&mut *self.history_notes.lock().unwrap_or_else(|e| e.into_inner()));
        said.extend(self.pending_history.iter().map(|h| {
            format!(
                "{}: the capture's history waits for a style the frame didn't draw",
                h.state.style
            )
        }));
        said
    }

    /// Start building, on worker threads, every style `config` switches on
    /// that isn't built or building yet, and the shared passes (edges,
    /// cross-fade, ownership) they will need (#1121). Returns the names of
    /// the styles started. The frame goes on drawing meanwhile; the first
    /// [`ensure`](Self::ensure) after a worker finishes puts its pass in,
    /// and one that needs a style still building waits for it, which is
    /// never longer than building it would be. A host calls it ahead of a
    /// look it will show: a scene's entry, a script's `prepare_look`.
    /// Building twice is avoided both ways: what is built or building is
    /// left alone, and `ensure` builds nothing a worker is on.
    ///
    /// The stage's own draws come too (#1159): the blend, reveal and tear
    /// pipelines each style's table asks for at its stage's format, which
    /// a frame otherwise builds the first time a style's `reveal` lies
    /// between 0 and 1, its `blend` drops under 1 or it follows the tear.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        config: &PostProcessConfig,
        ids: &StyleIds,
        styles: &StyleRegistry,
    ) -> Vec<&'static str> {
        self.prepare_with(device, surface_format, config, ids, styles, &[])
    }

    /// [`prepare`](Self::prepare), with every stage draw (blend, reveal
    /// and tear) built for the styles in `every_draw`, whatever their
    /// tables say now: a scene's `preload_styles`, whose styles a script
    /// will switch on, tween and tear later (#1159).
    pub fn prepare_with(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        config: &PostProcessConfig,
        ids: &StyleIds,
        styles: &StyleRegistry,
        every_draw: &[&str],
    ) -> Vec<&'static str> {
        self.prepare_draws(device, surface_format, config, ids, styles, every_draw);
        let mut started = Vec::new();
        for style in styles.styles() {
            let descriptor = style.descriptor();
            if !descriptor.is_on(config, ids) || self.preparing.contains_key(descriptor.name) {
                continue;
            }
            if self
                .slots
                .get(descriptor.name)
                .is_some_and(|s| s.from.same(style) && (s.built.is_some() || s.failed))
            {
                continue;
            }
            let format = stage_format(descriptor, surface_format);
            let (device, from) = (device.clone(), style.clone());
            let building = style.clone();
            let arrival = off_thread(descriptor.name, move || {
                build_style(&device, &building, format)
            });
            self.preparing
                .insert(descriptor.name, Preparing { from, arrival });
            started.push(descriptor.name);
        }
        let any_on = styles.iter().any(|d| d.is_on(config, ids));
        if any_on
            && self.ownership.is_none()
            && !self.ownership_failed
            && self.ownership_preparing.is_none()
        {
            let device = device.clone();
            self.ownership_preparing =
                Some(off_thread("ownership", move || build_ownership(&device)));
        }
        let edges = styles
            .iter()
            .any(|d| d.needs_edges() && d.is_on(config, ids));
        if edges && self.edges.is_none() && !self.edges_failed && self.edges_preparing.is_none() {
            let device = device.clone();
            self.edges_preparing = Some(off_thread("edges", move || build_edges(&device)));
        }
        let fades = styles
            .iter()
            .any(|d| d.is_on(config, ids) && d.framework_fades(config, ids));
        if fades && self.fade.is_none() && !self.fade_failed && self.fade_preparing.is_none() {
            let device = device.clone();
            self.fade_preparing = Some(off_thread("fade", move || build_fade(&device)));
        }
        started
    }

    /// Start one worker building the blend, reveal and tear pipelines the
    /// styles `config` switches on will draw (each style's from its
    /// table, every one for a style in `every_draw`), at each stage's
    /// format, that no earlier prepare asked for (#1159). It writes them
    /// straight into the shared [`blend::BlendPass`], whose locks make a
    /// frame that needs one meanwhile wait for it: nothing comes back to
    /// collect, and a pipeline a frame built first is found there and not
    /// built again. Returns whether it started one.
    fn prepare_draws(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        config: &PostProcessConfig,
        ids: &StyleIds,
        styles: &StyleRegistry,
        every_draw: &[&str],
    ) -> bool {
        let mut wanted: Vec<(wgpu::TextureFormat, blend::Draws)> = Vec::new();
        for descriptor in styles.iter() {
            if !descriptor.is_on(config, ids) {
                continue;
            }
            let draws = if every_draw.contains(&descriptor.name) {
                blend::Draws::ALL
            } else {
                blend::Draws::of(config, descriptor.name)
            };
            let format = stage_format(descriptor, surface_format);
            match wanted.iter_mut().find(|(f, _)| *f == format) {
                Some((_, d)) => *d = d.or(draws),
                None => wanted.push((format, draws)),
            }
        }
        // What no earlier prepare asked for. Never the pass's own locks:
        // a worker holds one while it builds, and asking would stall this
        // thread for that build (190 ms at a scene's entry, measured)
        let wanted: Vec<_> = wanted
            .into_iter()
            .map(|(format, draws)| {
                let asked = match self.blend_asked.iter_mut().find(|(f, _)| *f == format) {
                    Some((_, asked)) => asked,
                    None => {
                        self.blend_asked.push((format, blend::Draws::default()));
                        &mut self.blend_asked.last_mut().expect("just pushed").1
                    }
                };
                let new = draws.minus(*asked);
                *asked = asked.or(draws);
                (format, new)
            })
            .filter(|(_, draws)| draws.any())
            .collect();
        if wanted.is_empty() {
            return false;
        }
        let (device, pass) = (device.clone(), self.blend.clone());
        // Nobody waits on the arrival: the pass holds what it built
        drop(off_thread("blend", move || {
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                for (format, draws) in wanted {
                    pass.warm(&device, format, draws);
                }
            }));
            (made, Vec::new())
        }));
        true
    }

    /// The styles building ahead on worker threads (#1121), by name.
    pub fn preparing(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.preparing.keys().copied().collect();
        names.sort_unstable();
        names
    }

    /// Put in style `name`'s prepared pass if its worker has sent it
    /// (waiting for it when `wait`). True when the slot is settled
    /// (built, or failed and pushed onto `failed`); false while it
    /// builds, or when the worker hung up and the caller must build.
    fn collect_prepared(
        &mut self,
        name: &'static str,
        wait: bool,
        failed: &mut Vec<Style>,
    ) -> bool {
        let Some(preparing) = self.preparing.get(name) else {
            return false;
        };
        let Some(result) = arrived(&preparing.arrival, wait) else {
            return false;
        };
        let Preparing { from, .. } = self.preparing.remove(name).expect("just read");
        let Ok((made, files)) = result else {
            return false;
        };
        let slot = self.slots.entry(name).or_insert_with(|| BuiltSlot {
            from: from.clone(),
            built: None,
            failed: false,
            files: Vec::new(),
        });
        // Another install took the name meanwhile, or ensure built it
        if !slot.from.same(&from) || slot.built.is_some() || slot.failed {
            return slot.built.is_some() || slot.failed;
        }
        install(slot, made, files, self.text_capture, failed);
        true
    }

    /// With `FLINT_STYLE_SHADER_DIR` set (#418), every
    /// [`dev_shader::POLL_FRAMES`] calls: drop each pass that read a
    /// shader file whose stamp moved, so this [`ensure`](Self::ensure)
    /// builds it again from the file. A drop sets
    /// [`take_rebuilt`](Self::take_rebuilt), since the kept bind groups
    /// hold the old passes' buffers.
    pub(super) fn reload_changed_shaders(&mut self) {
        if !dev_shader::enabled() {
            return;
        }
        self.dev_polls += 1;
        if self.dev_polls < dev_shader::POLL_FRAMES {
            return;
        }
        self.dev_polls = 0;
        let changed = dev_shader::changed();
        if changed.is_empty() {
            return;
        }
        let moved = |files: &[std::path::PathBuf]| files.iter().any(|f| changed.contains(f));
        for slot in self.slots.values_mut() {
            if moved(&slot.files) {
                slot.built = None;
                slot.failed = false;
                slot.files.clear();
            }
        }
        if moved(&self.shared_files) {
            self.edges = None;
            self.edges_failed = false;
            self.fade = None;
            self.fade_failed = false;
            self.ownership = None;
            self.ownership_failed = false;
            self.shared_files.clear();
        }
        // The blend draw builds lazily, outside a build: always again
        self.blend = Default::default();
        self.blend_asked.clear();
        let names: Vec<String> = changed
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        tracing::info!(
            "style shaders changed ({}): building again",
            names.join(", ")
        );
        self.rebuilt = true;
    }

    /// Whether a pass was dropped for a changed shader file (#418) since
    /// the last call. The renderer then forgets the kept bind groups.
    pub fn take_rebuilt(&mut self) -> bool {
        std::mem::take(&mut self.rebuilt)
    }

    /// The shared cross-fade (#396), for [`run_stage`]; `None` until a
    /// style that gets it has been on.
    pub(crate) fn fade(&self) -> Option<&fade::FadePass> {
        self.fade.as_ref()
    }

    /// How many times the shared cross-fade has been drawn since the
    /// renderer was made (#396).
    pub fn fade_runs(&self) -> u64 {
        self.fade.as_ref().map_or(0, fade::FadePass::runs)
    }

    /// Draw the shared edges (#301) if any of `styles` (this frame's
    /// active ones, both stages) needs them, inside a GPU-timer scope of
    /// their own. Returns the texture for [`StyleFrame::edges`].
    pub(crate) fn encode_edges<'a>(
        &self,
        styles: &[(&'a StyleDescriptor, &dyn StylePass)],
        frame: &mut StyleFrame<'a>,
        encoder: &mut wgpu::CommandEncoder,
        scope: &mut Scope<'_>,
    ) -> Option<Arc<StyleTexture>> {
        let pass = self.edges.as_ref()?;
        let readers: Vec<_> = styles
            .iter()
            .filter(|(d, _)| d.wants_edges(frame))
            .collect();
        if readers.is_empty() {
            return None;
        }
        let creases = readers.iter().any(|(d, _)| d.needs_creases());
        // Where they read them (#743): the union of their rects, unless
        // one reads the whole frame
        let was = frame.descriptor;
        let mut rect = Some(kit::PixelRect::EMPTY);
        for (d, style) in &readers {
            frame.descriptor = d;
            rect = rect.zip(style.edges_rect(frame)).map(|(a, b)| a.union(b));
        }
        frame.descriptor = was;
        let frame = &*frame;
        let mut made = None;
        scope(edges::EDGE_LABEL, encoder, &mut |encoder| {
            made = Some(pass.encode(frame, encoder, creases, rect));
        });
        self.edge_runs
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // A pass knob that skips the edges (#1317) never called the scope:
        // the readers still read the target, as it was last drawn
        made.or_else(|| Some(edges::EdgePass::target(frame)))
    }

    /// The ownership pass (#384) [`run_stage`] draws after each style.
    pub(crate) fn ownership(&self) -> Option<&ownership::OwnershipPass> {
        self.ownership.as_ref()
    }

    /// How many times the shared edge pass has been drawn since the
    /// renderer was made.
    pub fn edge_runs(&self) -> u64 {
        self.edge_runs.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Have glyph styles keep each frame's grid as text (#317), now and
    /// for every style built later.
    pub fn set_text_capture(&mut self, on: bool) {
        self.text_capture = on;
        for pass in self.slots.values().filter_map(|s| s.built.as_ref()) {
            pass.set_text_capture(on);
        }
    }

    /// The glyph grid style `name` drew in the last frame encoded with text
    /// capture on, after its submission ([`StylePass::read_text`]).
    pub fn read_text(&self, name: &str, device: &wgpu::Device) -> Option<GlyphText> {
        self.slots.get(name)?.built.as_deref()?.read_text(device)
    }

    /// Whether style `name` is built.
    pub fn is_built(&self, name: &str) -> bool {
        self.slots.get(name).is_some_and(|s| s.built.is_some())
    }

    /// Whether the pass built for style `name` is `style`'s install (#742),
    /// for tests that a reload builds again.
    pub fn built_from(&self, name: &str) -> Option<&Style> {
        self.slots
            .get(name)
            .filter(|s| s.built.is_some())
            .map(|s| &s.from)
    }

    /// The built styles of `stage` that are on this frame (by `config` or
    /// an entity's request in `ids`), in chain order.
    ///
    /// A style whose table `blend` (or `reveal`, #406) is 0 (#399) is left out: all of it
    /// would be drawn back over by its input, so the chain skips it and
    /// the frame is the one without it, byte for byte and at no cost. It
    /// stays built and [`is_on`](StyleDescriptor::is_on), so the mask,
    /// ownership (#384) and its GPU state are what they were at 0.01, and
    /// a fade out meets the plain frame with no pop.
    pub fn active<'s>(
        &'s self,
        stage: StyleStage,
        config: &PostProcessConfig,
        ids: &StyleIds,
        styles: &'s StyleRegistry,
    ) -> Vec<(&'s StyleDescriptor, &'s dyn StylePass)> {
        if !config.enabled {
            return Vec::new();
        }
        let mut on: Vec<(f32, (&StyleDescriptor, &dyn StylePass))> = styles
            .styles()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.stage == stage && s.is_on(config, ids))
            .filter(|(_, s)| config.styles.shows_in(s.name, config))
            .filter_map(|(i, s)| {
                let slot = self.slots.get(s.name)?;
                // A pass built from an install since replaced waits for ensure
                if !slot.from.same(s) {
                    return None;
                }
                let pass = slot.built.as_deref()?;
                Some((chain_order(i, s, config), (s.descriptor(), pass)))
            })
            .collect();
        // Stable: equal orders keep registration order
        on.sort_by(|a, b| a.0.total_cmp(&b.0));
        on.into_iter().map(|(_, style)| style).collect()
    }
}

/// Keep only style `name` of a stage's [`StyleSet::active`] list when
/// `name` is given (the `style` debug view's `mask_view`, #334), so it
/// reads the stage's original input rather than another style's output;
/// without a name, the list as it is.
pub fn solo<'d, T: Copy>(
    styles: Vec<(&'d StyleDescriptor, T)>,
    name: Option<&str>,
) -> Vec<(&'d StyleDescriptor, T)> {
    match name {
        Some(name) => styles.into_iter().filter(|(d, _)| d.name == name).collect(),
        None => styles,
    }
}

/// Where the style at registry index `index` stands in its stage's chain
/// (#299): its table's `order`, else the index itself, so a scene that
/// orders nothing runs in registration order. Lower runs first.
pub fn chain_order(index: usize, descriptor: &StyleDescriptor, config: &PostProcessConfig) -> f32 {
    config
        .styles
        .order(descriptor.name)
        .or(descriptor.default_order)
        .unwrap_or(index as f32)
}

/// Which view each style of a stage reads and writes: `Input` is what the
/// stage starts from, `Ping(n)` the framework's two targets, `Final` the
/// caller's target. In the display stage ([`display_plan`]) the input is
/// the framework's own texture, so a later style may write it again, and
/// `Final` may be read too when it is a texture the frame owns (FXAA's
/// intermediate, #302).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Input,
    Ping(u8),
    Final,
}

/// The ping-pong plan for `count` styles. With `to_final`, the last style
/// writes the caller's target; without, the stage's result stays in a ping
/// target (the HDR stage, whose result the composite reads).
pub fn plan(count: usize, to_final: bool) -> Vec<(Slot, Slot)> {
    let mut steps = Vec::with_capacity(count);
    let mut src = Slot::Input;
    for i in 0..count {
        let dst = if to_final && i + 1 == count {
            Slot::Final
        } else {
            Slot::Ping((i % 2) as u8)
        };
        steps.push((src, dst));
        src = dst;
    }
    steps
}

/// The display stage's plan for `count` styles (#302): the slot the
/// composite draws into, and each style's (reads, writes). The stage owns
/// its input, so after the first style has read it the input is free to
/// be a ping: a chain of any length needs one texture beside it, and only
/// from two styles on. With `final_owned` (FXAA on, whose intermediate is
/// the final target) the intermediate is the other texture: the composite
/// draws into whichever of the two the chain's parity leaves the last
/// style writing the intermediate, so the stage adds one full-size texture
/// to FXAA's, never more.
pub fn display_plan(count: usize, final_owned: bool) -> (Slot, Vec<(Slot, Slot)>) {
    if count == 0 {
        return (Slot::Final, Vec::new());
    }
    let pair = match (final_owned, count % 2) {
        (false, _) => [Slot::Input, Slot::Ping(0)],
        (true, 1) => [Slot::Input, Slot::Final],
        (true, _) => [Slot::Final, Slot::Input],
    };
    let steps = (0..count)
        .map(|i| {
            let dst = if i + 1 == count {
                Slot::Final
            } else {
                pair[(i + 1) % 2]
            };
            (pair[i % 2], dst)
        })
        .collect();
    (pair[0], steps)
}

/// How many textures of its own a stage's `steps` draw through: the pings
/// they name, plus the input when the stage owns it (the display stage).
pub fn plan_textures(steps: &[(Slot, Slot)], owns_input: bool) -> usize {
    let used = |slot: Slot| steps.iter().any(|(s, d)| *s == slot || *d == slot);
    (owns_input && !steps.is_empty()) as usize + (0..2).filter(|n| used(Slot::Ping(*n))).count()
}

/// Ids of the views a style's input can be, for keying bind groups.
pub(crate) mod input_id {
    /// The scene's HDR buffer.
    pub const HDR: u64 = 0;
    /// The HDR stage's ping targets: `HDR_PING + n`.
    pub const HDR_PING: u64 = 1;
    /// What the composite writes when display styles follow it.
    pub const DISPLAY_IN: u64 = 3;
    /// The display stage's ping targets: `DISPLAY_PING + n`.
    pub const DISPLAY_PING: u64 = 4;
    /// FXAA's intermediate, when the display stage draws through it (#302).
    pub const FXAA_IN: u64 = 6;
}

pub(super) const HDR_PINGS: [&str; 2] = ["style hdr ping 0", "style hdr ping 1"];
pub(super) const DISPLAY_PINGS: [&str; 2] = ["style display ping 0", "style display ping 1"];

/// A GPU-timer scope around one style: `(label, encoder, draw)`.
pub(crate) type Scope<'s> = dyn FnMut(&'static str, &mut wgpu::CommandEncoder, &mut dyn FnMut(&mut wgpu::CommandEncoder))
    + 's;

/// Run `styles` (one stage's, in order) by `steps` ([`plan`] or
/// [`display_plan`], one per style), from `input` through the stage's ping
/// targets; only the pings the steps name are made. With `to`, the last
/// style writes there and this returns `None`; without, it returns the
/// target holding the result and that target's input id. `frame` carries
/// the stage's size and format; its descriptor is set per style. A style
/// whose `blend` is under 1 (#369) has what it read drawn back over its
/// result by `blender`, inside its own timing scope.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_stage<'a>(
    styles: &[(&'a StyleDescriptor, &dyn StylePass)],
    steps: &[(Slot, Slot)],
    frame: &mut StyleFrame<'a>,
    stage: StyleStage,
    encoder: &mut wgpu::CommandEncoder,
    input: StyleInput<'_>,
    to: Option<StyleInput<'_>>,
    blender: &blend::BlendPass,
    ownership: Option<&ownership::OwnershipPass>,
    fade: Option<&fade::FadePass>,
    scope: &mut Scope<'_>,
) -> Option<(Arc<StyleTexture>, u64)> {
    debug_assert_eq!(styles.len(), steps.len());
    let (names, base) = match stage {
        StyleStage::Hdr => (HDR_PINGS, input_id::HDR_PING),
        StyleStage::Display => (DISPLAY_PINGS, input_id::DISPLAY_PING),
    };
    let mut result = None;
    let pings: [Option<Arc<StyleTexture>>; 2] = std::array::from_fn(|n| {
        let slot = Slot::Ping(n as u8);
        steps
            .iter()
            .any(|(s, d)| *s == slot || *d == slot)
            .then(|| frame.target(names[n], frame.format))
    });
    let ping = |n: u8| pings[n as usize].as_ref().expect("a ping the steps name");
    // A grid the ownership cut snaps to (#534), asked before any style of
    // the stage draws, so a style that runs before the snapping one cuts
    // along the same cells. The display stage's alone: its styles all draw
    // at the output's size, which the grid is in
    frame.snapped_ownership = match stage {
        StyleStage::Display if ownership.is_some() => snapped_ownership(styles, frame),
        _ => None,
    };
    for ((descriptor, pass), (src, dst)) in styles.iter().zip(steps.iter().copied()) {
        frame.descriptor = *descriptor;
        let src = match src {
            Slot::Input => StyleInput {
                view: input.view,
                id: input.id,
            },
            Slot::Ping(n) => StyleInput {
                view: &ping(n).view,
                id: base + u64::from(n),
            },
            Slot::Final => {
                let to = to.as_ref().expect("planned with a final target");
                StyleInput {
                    view: to.view,
                    id: to.id,
                }
            }
        };
        let dst_view = match dst {
            Slot::Ping(n) => {
                result = Some((ping(n).clone(), base + u64::from(n)));
                &ping(n).view
            }
            Slot::Final => {
                result = None;
                to.as_ref().expect("planned with a final target").view
            }
            Slot::Input => {
                result = None;
                input.view
            }
        };
        let frame = &*frame;
        // A style that draws its own whole-frame blend (#879, pencil's
        // `transition = "draw"`) has drawn it already
        let blend = if descriptor.draws_own_blend(frame.config, None) {
            1.0
        } else {
            frame.config.styles.blend(descriptor.name)
        };
        // The framework's cross-fade (#396), timed with the style it fades
        let fade = fade.filter(|_| fade::FadePass::applies(frame, descriptor));
        scope(descriptor.label, encoder, &mut |encoder| {
            pass.encode(frame, encoder, &src, dst_view);
            // The blend, and where the style shows when it has a reveal
            // (#406)
            blender.draw(frame, encoder, &src, dst_view, blend);
            // This style's own entities below blend 1 get some of its
            // input back (#396)
            if let Some(fade) = fade {
                fade.encode(frame, descriptor, encoder, &src, dst_view);
            }
            // Each pixel one style's (#384): another style's entities get
            // this style's input back
            if let Some(ownership) = ownership {
                ownership.encode(frame, encoder, &src, dst_view);
            }
        });
    }
    result
}

/// The first style of `styles` that yields and answers
/// [`StylePass::ownership_cells`] (#534), with where the ids the other
/// running styles own can be. `None` when none answers, its grid is no
/// grid, or no other style's entity is in view.
fn snapped_ownership<'a>(
    styles: &[(&'a StyleDescriptor, &dyn StylePass)],
    frame: &mut StyleFrame<'a>,
) -> Option<SnappedOwnership> {
    for (descriptor, pass) in styles {
        if !descriptor.yields {
            continue;
        }
        frame.descriptor = *descriptor;
        let Some(cells) = pass.ownership_cells(frame) else {
            continue;
        };
        let sane = |v: [f32; 2], least: f32| v.iter().all(|x| x.is_finite() && *x >= least);
        if !sane(cells.cell, 1.0) || !sane(cells.origin, f32::MIN) || !cells.share.is_finite() {
            return None;
        }
        let cells = OwnershipCells {
            share: cells.share.clamp(1.0 / 16.0, 1.0),
            ..cells
        };
        let style = frame.styles.index_of(descriptor.name)? as u32;
        // A cell touching a foreign id reads it: grow by two cells
        let reach = (2.0 * cells.cell[0].max(cells.cell[1])).ceil() as u32;
        let rect = match frame.foreign_rect(reach) {
            Some(r) if r.width == 0 || r.height == 0 => return None,
            Some(r) => [
                r.x as f32,
                r.y as f32,
                (r.x + r.width) as f32,
                (r.y + r.height) as f32,
            ],
            None => [0.0, 0.0, frame.width as f32, frame.height as f32],
        };
        return Some(SnappedOwnership { style, cells, rect });
    }
    None
}
