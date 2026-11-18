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

/// The styles whose pipelines exist, by name. Lives on the post pipeline;
/// nothing in it depends on the frame's size.
#[derive(Default)]
pub struct StyleSet {
    pub(super) slots: std::collections::HashMap<&'static str, BuiltSlot>,
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
    /// The draw that shows a style's `blend` (#369), any style's.
    pub(super) blend: blend::BlendPass,
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
}

impl StyleSet {
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
        self.reload_changed_shaders();
        let mut failed = Vec::new();
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
            let format = match descriptor.stage {
                StyleStage::Hdr => crate::postprocess::HDR_FORMAT,
                StyleStage::Display => surface_format,
            };
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
                || {
                    dev_shader::build(descriptor.name, || {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            // One install at one format builds the same
                            // layouts every time: what lets its pipelines be
                            // kept (#911)
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
                },
            );
            slot.files = files;
            match made {
                Ok(pass) => {
                    pass.set_text_capture(self.text_capture);
                    slot.built = Some(pass);
                }
                Err(_) => {
                    slot.failed = true;
                    failed.push(style.clone());
                }
            }
        }
        if self.ownership.is_none()
            && !self.ownership_failed
            && self.slots.values().any(|s| s.built.is_some())
        {
            let (made, files) = dev_shader::build("ownership", || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    ownership::OwnershipPass::new(device)
                }))
            });
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
            let (made, files) = dev_shader::build("edges", || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::pipeline_cache::in_scope("Style Edges", || edges::EdgePass::new(device))
                }))
            });
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
            let (made, files) = dev_shader::build("fade", || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    fade::FadePass::new(device)
                }))
            });
            self.shared_files.extend(files);
            match made {
                Ok(pass) => self.fade = Some(pass),
                Err(_) => {
                    tracing::warn!("style cross-fade unavailable: its shader failed to build");
                    self.fade_failed = true;
                }
            }
        }
        failed
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
        self.blend = blend::BlendPass::default();
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
        made
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
