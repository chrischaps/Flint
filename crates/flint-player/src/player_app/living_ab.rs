//! Two looks at one grove (#238): A and B, each a preset laid over the scene as
//! it loaded, on the same props, the same camera and the same clock.
//!
//! The scene's own living state is always one of the two, the one being shown
//! or tuned: the F3 panel, the knobs and scripts all reach it as they always
//! have. The other is a [`LivingSide`] kept beside it, with its own props
//! (forked from the scene's, so the same trees on the same clocks), its own
//! wooze field, boil clock, shallow wooze and boil amounts. Both move on by the
//! same `dt` every frame, and take turns with one pose cache, since a pose
//! depends on no knob. A flip swaps the two, and the side now shown writes all of
//! its bones once, since the buffers held the other's. Nothing else changes, so
//! with the clock frozen what changes on screen is what the two presets differ in.
//!
//! A split draws the shown side as the frame always is, submits it, then draws
//! the other into a texture of its own and copies its half over (see
//! [`flint_render::SplitView`]). Two submissions, because the bones and the
//! boil reach the GPU through the queue's writes, and one submission would show
//! the second side's to both. Both sides write every bone every frame while
//! split, since the buffers swap hands twice a frame. A is always left.

use flint_procgen::{BlendFrame, LivingProps, ProcGenResolver};
use flint_render::{Camera, SceneRenderer, SplitView};
use flint_wooze::{BoilClock, BoilFrame, WoozeField};

use super::living_subsystem::LivingSubsystem;
use super::shallow_wooze::ShallowWooze;
use super::subsystem::StageCtx;

/// What a side called "as it was" names: the knobs as they stood when the
/// comparison began.
pub const AS_IT_WAS: &str = "";

/// The boil as one side has it: what the passes get and what off remembers.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SideBoil {
    paint: f32,
    ink: f32,
    on: u32,
    off: Option<(f32, f32)>,
}

/// The look that isn't the scene's own right now.
pub struct LivingSide {
    props: LivingProps,
    wooze: Option<WoozeField>,
    wooze_time: f64,
    boil_clock: BoilClock,
    shallow: ShallowWooze,
    boil: SideBoil,
}

/// A comparison under way.
pub struct LivingAb {
    /// What A and B are: a preset's name, or [`AS_IT_WAS`].
    names: [String; 2],
    hidden: LivingSide,
    /// Whether the scene's own state is B's now.
    showing_b: bool,
    /// Whether both are on screen, A left and B right.
    split: bool,
    /// The second drawing, made the first time it's split.
    view: Option<SplitView>,
}

impl LivingAb {
    /// Whether both sides are on screen.
    pub fn is_split(&self) -> bool {
        self.split
    }
}

/// The labels' ink, the Living Props panel's.
const INK: egui::Color32 = egui::Color32::from_rgb(232, 180, 90);
const PAPER: egui::Color32 = egui::Color32::from_rgb(244, 236, 220);

/// Which preset is where: one tab over the view while flipping, one each side of
/// a hairline seam while split. The side the knobs reach has a dot. All of it
/// stays in a band at the top and on the seam's two columns, so the rest of the
/// frame is the scene's.
pub fn paint_labels(ctx: &egui::Context, ab: &AbState) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("living_ab_labels"),
    ));
    let screen = ctx.screen_rect();
    let mid = screen.center().x;
    let tab = |at: egui::Pos2, align: egui::Align2, letter: &str, name: &str, live: bool| {
        let text = format!("{letter}  {}", side_label(name));
        let galley = painter.layout_no_wrap(text, egui::FontId::monospace(12.0), PAPER);
        let size = galley.size() + egui::vec2(if live { 26.0 } else { 16.0 }, 8.0);
        let rect = align.anchor_size(at, size);
        painter.rect_filled(
            rect,
            4.0,
            // Premultiplied by hand: the HUD blends in display space (CLAUDE.md).
            egui::Color32::from_rgba_premultiplied(0, 0, 0, 150),
        );
        let mut left = rect.left() + 8.0;
        if live {
            painter.circle_filled(egui::pos2(left + 3.0, rect.center().y), 3.0, INK);
            left += 10.0;
        }
        painter.galley(
            egui::pos2(left, rect.center().y - galley.size().y * 0.5),
            galley,
            PAPER,
        );
    };
    let top = screen.top() + 8.0;
    if ab.split {
        let seam = [
            egui::pos2(mid, screen.top()),
            egui::pos2(mid, screen.bottom()),
        ];
        painter.line_segment(seam, egui::Stroke::new(1.0_f32, INK));
        let gap = 8.0;
        tab(
            egui::pos2(mid - gap, top),
            egui::Align2::RIGHT_TOP,
            "A",
            &ab.a,
            !ab.showing_b,
        );
        tab(
            egui::pos2(mid + gap, top),
            egui::Align2::LEFT_TOP,
            "B",
            &ab.b,
            ab.showing_b,
        );
    } else {
        let (letter, name) = if ab.showing_b {
            ("B", &ab.b)
        } else {
            ("A", &ab.a)
        };
        tab(
            egui::pos2(mid, top),
            egui::Align2::CENTER_TOP,
            letter,
            name,
            false,
        );
    }
}

impl std::fmt::Debug for LivingAb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LivingAb")
            .field("names", &self.names)
            .field("showing_b", &self.showing_b)
            .field("split", &self.split)
            .finish_non_exhaustive()
    }
}

/// How a comparison stands, for the panel and the overlay.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AbState {
    pub a: String,
    pub b: String,
    pub showing_b: bool,
    pub split: bool,
}

/// A side's name as a person reads it.
pub fn side_label(name: &str) -> &str {
    if name == AS_IT_WAS {
        "as it was"
    } else {
        name
    }
}

impl LivingSubsystem {
    /// How the comparison stands, if there is one.
    pub(super) fn living_ab_state(&self) -> Option<AbState> {
        let ab = self.tune.ab.as_ref()?;
        Some(AbState {
            a: ab.names[0].clone(),
            b: ab.names[1].clone(),
            showing_b: ab.showing_b,
            split: ab.split,
        })
    }

    fn side_boil(&self, ctx: &StageCtx<'_>) -> SideBoil {
        let (paint, ink, on) = ctx.renderer.as_ref().map_or((0.0, 0.0, 2), |r| {
            let config = r.post_process_config();
            let (paint, ink) = config.boil_amounts();
            (paint, ink, config.boil_on)
        });
        SideBoil {
            paint,
            ink,
            on,
            off: self.tune.boil_off,
        }
    }

    fn set_side_boil(&mut self, ctx: &mut StageCtx<'_>, boil: SideBoil) {
        self.tune.boil_off = boil.off;
        if let Some(renderer) = &mut ctx.renderer {
            let mut config = renderer.post_process_config().clone();
            config.set_boil_amounts(boil.paint, boil.ink);
            config.boil_on = boil.on;
            renderer.set_post_process_config(config);
        }
    }

    /// Put the hidden side where the scene's own state is, and that where the
    /// hidden side was. The pose cache stays with the scene.
    fn swap_living_sides(&mut self, ctx: &mut StageCtx<'_>) {
        let boil = self.side_boil(ctx);
        let Some(ab) = self.tune.ab.as_mut() else {
            return;
        };
        let side = &mut ab.hidden;
        let live = ctx.resolver.living_props_mut();
        std::mem::swap(live, &mut side.props);
        live.swap_cache(&mut side.props);
        std::mem::swap(&mut self.wooze, &mut side.wooze);
        std::mem::swap(&mut self.wooze_time, &mut side.wooze_time);
        std::mem::swap(&mut self.boil_clock, &mut side.boil_clock);
        std::mem::swap(&mut self.shallow_wooze, &mut side.shallow);
        let theirs = std::mem::replace(&mut side.boil, boil);
        self.set_side_boil(ctx, theirs);
    }

    /// Lay a preset over the knobs as the scene loaded them, each species its
    /// own (#244), or leave them as they are for [`AS_IT_WAS`], and finish
    /// whatever that starts, now.
    fn become_preset(&mut self, ctx: &mut StageCtx<'_>, name: &str) -> Result<(), String> {
        let mut result = Ok(());
        if name != AS_IT_WAS {
            self.reset_living_knobs(ctx);
            result = self.load_living_preset(ctx, name);
        }
        ctx.resolver.settle_living_now(None);
        // Glide and drift reach a part on its next beat, which would show the
        // last look's for up to a beat: a whole look, frozen, for ever.
        ctx.resolver.living_props_mut().restart_cycles();
        result
    }

    /// Compare preset `a` with preset `b` on this grove, showing A. Either may be
    /// [`AS_IT_WAS`]. A comparison already going ends first, keeping what it
    /// showed. What can't be loaded is named, and that side keeps what it had.
    pub(super) fn compare_living(
        &mut self,
        ctx: &mut StageCtx<'_>,
        a: &str,
        b: &str,
    ) -> Result<(), String> {
        if ctx.resolver.living_props().is_empty() {
            return Err("this scene has no living props to compare".to_string());
        }
        self.stop_comparing_living(ctx);
        if self.tune.at_load.is_none() {
            self.tune.at_load = Some(self.living_at_load(ctx));
        }
        let hidden = LivingSide {
            props: ctx.resolver.living_props().fork(),
            wooze: self.wooze.clone(),
            wooze_time: self.wooze_time,
            boil_clock: self.boil_clock,
            shallow: self.shallow_wooze.clone(),
            boil: self.side_boil(ctx),
        };
        self.tune.ab = Some(Box::new(LivingAb {
            names: [a.to_string(), b.to_string()],
            hidden,
            showing_b: false,
            split: false,
            view: None,
        }));
        let mut problems = Vec::new();
        // B first, on the side that'll be hidden: it starts as a copy of now.
        self.swap_living_sides(ctx);
        if let Err(problem) = self.become_preset(ctx, b) {
            problems.push(format!("B ({}): {problem}", side_label(b)));
        }
        self.swap_living_sides(ctx);
        if let Err(problem) = self.become_preset(ctx, a) {
            problems.push(format!("A ({}): {problem}", side_label(a)));
        }
        ctx.resolver.living_props_mut().redraw_all();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// Show the other side, on the same clock and camera. Returns whether there
    /// was a comparison to flip.
    pub(super) fn flip_living(&mut self, ctx: &mut StageCtx<'_>) -> bool {
        if self.tune.ab.is_none() {
            return false;
        }
        self.swap_living_sides(ctx);
        if let Some(ab) = self.tune.ab.as_mut() {
            ab.showing_b = !ab.showing_b;
        }
        ctx.resolver.living_props_mut().redraw_all();
        true
    }

    /// Show A and B side by side, or one at a time. Returns whether there was a
    /// comparison to split.
    pub(super) fn split_living(&mut self, ctx: &mut StageCtx<'_>, split: bool) -> bool {
        let Some(ab) = self.tune.ab.as_mut() else {
            return false;
        };
        if ab.split != split {
            ab.split = split;
            // Whatever the buffers hold now, the shown side writes all of it.
            ctx.resolver.living_props_mut().redraw_all();
        }
        true
    }

    /// End the comparison, keeping the side that's shown.
    pub(super) fn stop_comparing_living(&mut self, ctx: &mut StageCtx<'_>) {
        if self.tune.ab.take().is_some() {
            ctx.resolver.living_props_mut().redraw_all();
        }
    }

    /// Move the hidden side on by the same `dt` as the scene's own, after it.
    pub(super) fn tick_hidden_side(&mut self, ctx: &mut StageCtx<'_>, dt: f64) {
        let Some(ab) = self.tune.ab.as_mut() else {
            return;
        };
        let side = &mut ab.hidden;
        if side.wooze.is_some() {
            side.wooze_time += dt;
            let world = &ctx.world;
            side.props.track_positions(|entity| {
                world.get_world_position(entity).map(|p| [p.x, p.y, p.z])
            });
        }
        if let Some(field) = &side.wooze {
            let sample = field.sample(ctx.camera.position_array(), side.wooze_time);
            side.boil_clock.tick(dt, &sample);
        }
        let wooze = side.wooze.as_ref().map(|field| (field, side.wooze_time));
        ctx.resolver
            .process_living_beside(&mut side.props, dt, wooze);
        if let Some(field) = &side.wooze {
            side.shallow.advance(ctx.world, field, side.wooze_time, dt);
        }
    }

    /// The hidden side's ring report, for `--exit-after`.
    pub(super) fn hidden_side_stats(&self) -> Option<flint_procgen::LivingStats> {
        Some(self.tune.ab.as_ref()?.hidden.props.stats())
    }

    /// What A and B would be, for a key or the panel that starts a comparison:
    /// the panel's picks, or the knobs as they stand against `linklater` (the
    /// epic's sweet spot) or the first preset on offer.
    pub(super) fn living_ab_picks(&self, ctx: &StageCtx<'_>) -> (String, String) {
        if let Some(picks) = &self.tune.ab_picks {
            return picks.clone();
        }
        let names = self.living_preset_names(ctx);
        let b = names
            .iter()
            .find(|name| *name == "linklater")
            .or(names.first())
            .cloned()
            .unwrap_or_default();
        (AS_IT_WAS.to_string(), b)
    }

    /// F5: stop or start the clock living props run on, as the panel's Freeze.
    pub(super) fn toggle_living_freeze(&mut self, ctx: &mut StageCtx<'_>) {
        if !ctx.resolver.living_props().is_empty() {
            self.tune.frozen = !self.tune.frozen;
        }
    }

    /// F6 flips (`split` false) and F7 splits or unsplits, starting a comparison
    /// of the picks if there isn't one. In a scene with no living props, nothing.
    pub(super) fn living_ab_key(&mut self, ctx: &mut StageCtx<'_>, split: bool) {
        if ctx.resolver.living_props().is_empty() {
            return;
        }
        if self.tune.ab.is_none() {
            let (a, b) = self.living_ab_picks(ctx);
            let said = match self.compare_living(ctx, &a, &b) {
                Ok(()) => format!(
                    "Comparing A ({}) with B ({}).",
                    side_label(&a),
                    side_label(&b)
                ),
                Err(problem) => problem,
            };
            println!("[living] {said}");
            self.tune.status = Some(said);
            if split {
                self.split_living(ctx, true);
            }
            return;
        }
        if split {
            let now = self.living_split();
            self.split_living(ctx, !now);
        } else {
            self.flip_living(ctx);
        }
    }

    /// The comparison asked for at start, once there are props to compare.
    pub(super) fn start_asked_comparison(&mut self, ctx: &mut StageCtx<'_>) {
        if ctx.resolver.living_props().is_empty() {
            return;
        }
        let Some(split) = self.tune.compare_at_start.take() else {
            return;
        };
        let (a, b) = self.living_ab_picks(ctx);
        match self.compare_living(ctx, &a, &b) {
            Ok(()) => println!(
                "[living] comparing A ({}) with B ({}){}",
                side_label(&a),
                side_label(&b),
                if split { ", split" } else { "" }
            ),
            Err(problem) => eprintln!("[living] --compare: {problem}"),
        }
        if split {
            self.split_living(ctx, true);
        }
    }

    /// Whether this frame is split.
    pub(super) fn living_split(&self) -> bool {
        self.tune.ab.as_ref().is_some_and(|ab| ab.split)
    }
}

/// Everything [`draw_other_side`] borrows from the player, which is also holding
/// its renderer and context.
pub struct OtherSide<'a> {
    pub ab: &'a mut LivingAb,
    pub resolver: &'a mut ProcGenResolver,
    pub blend: &'a mut BlendFrame,
    pub world: &'a flint_ecs::FlintWorld,
    pub camera: &'a Camera,
    /// The scene's own boil drawing this frame, to put back.
    pub own_boil: BoilFrame,
    /// The scene's own shallow wooze, to put back.
    pub own_shallow: &'a ShallowWooze,
}

/// The second half of a split frame: the hidden side's bones, boil and shallow
/// wooze, drawn into the split view and copied over its half of `frame` (a view
/// of the swapchain in `copy_format`). The scene's own first drawing must be
/// submitted already. Leaves the renderer as the scene's own state wants it,
/// with every one of its bones to be written again next frame.
#[allow(clippy::too_many_arguments)]
pub fn draw_other_side(
    other: OtherSide<'_>,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    depth_view: &wgpu::TextureView,
    frame: &wgpu::TextureView,
    size: (u32, u32),
    formats: (wgpu::TextureFormat, wgpu::TextureFormat),
) {
    let OtherSide {
        ab,
        resolver,
        blend,
        world,
        camera,
        own_boil,
        own_shallow,
    } = other;
    let view = ab
        .view
        .get_or_insert_with(|| SplitView::new(device, size.0, size.1, formats.0, formats.1));
    view.ensure_size(device, size.0, size.1);
    let side = &mut ab.hidden;

    // Its bones, whole, into the buffers the first drawing has finished with.
    side.props.redraw_all();
    super::living_props::upload_props(&mut side.props, blend, renderer, device, queue);

    // Its boil and shallow wooze, then the scene as it now stands.
    let own_config = renderer.post_process_config().clone();
    let mut config = own_config.clone();
    config.set_boil_amounts(side.boil.paint, side.boil.ink);
    config.boil_on = side.boil.on;
    let boil = side
        .wooze
        .as_ref()
        .map(|field| {
            let sample = field.sample(camera.position_array(), side.wooze_time);
            side.boil_clock.frame(&sample, side.boil.on)
        })
        .unwrap_or_default();
    renderer.set_post_process_config(config);
    renderer.set_boil(boil);
    renderer.set_shallow_wooze(side.shallow.draws().iter().copied());
    renderer.update_from_world(world, device);
    renderer.encode_again_to(
        device,
        queue,
        encoder,
        depth_view,
        camera,
        view.scene_view(),
    );

    // A on the left, wherever the scene's own state is.
    let seam = view.seam_x(0.5);
    if ab.showing_b {
        view.encode_copy(encoder, frame, 0, seam);
    } else {
        view.encode_copy(encoder, frame, seam, size.0);
    }

    // Back as the scene's own state has it. Its bones get written next frame,
    // and next frame's extraction rebuilds its draws.
    renderer.set_post_process_config(own_config);
    renderer.set_boil(own_boil);
    renderer.set_shallow_wooze(own_shallow.draws().iter().copied());
    resolver.living_props_mut().redraw_all();
}
