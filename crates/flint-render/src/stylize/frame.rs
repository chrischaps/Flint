//! What a style's pass reads each frame: [`StylePass`], [`StyleFrame`] and
//! its helpers (moved from `stylize/mod.rs`, #742).

use super::*;
use crate::gpu_counts::CountedDevice;

/// A style's GPU side: pipelines, layouts and uniform buffers, made once.
pub trait StylePass: Send + Sync {
    /// Record this frame's draws: read `input`, write every pixel of
    /// `output` (both [`StyleFrame::width`] x [`StyleFrame::height`], in
    /// [`StyleFrame::format`]). `input_id` distinguishes the views the
    /// input can be; key bind groups over it with it
    /// ([`StyleFrame::sampled_input`] does).
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    );

    /// Keep each frame's glyph grid for [`read_text`](Self::read_text)
    /// (#317). Only a style that draws glyphs does anything with it.
    fn set_text_capture(&self, _on: bool) {}

    /// The glyph atlases this pass holds on the GPU, bytes (#1091), for
    /// F2's memory line. Only a style that draws glyphs has any.
    fn glyph_atlas_bytes(&self) -> u64 {
        0
    }

    /// The glyph grid the last frame encoded with capture on drew, once
    /// that frame's submission is done (this waits on the device). `None`
    /// from a style that draws no glyphs, or before such a frame.
    fn read_text(&self, _device: &wgpu::Device) -> Option<GlyphText> {
        None
    }

    /// Whether the style keeps state from frame to frame that a second
    /// draw in one frame would advance (ASCII's cell history, #277; a user
    /// style's `history` target): such a style refuses a pass knob's
    /// repeat (#1317) and draws once. False, the default, for a style whose
    /// every draw is the same work.
    fn keeps_history(&self) -> bool {
        false
    }

    /// The shared edges' pixels (at [`StyleFrame::depth_size`]) this
    /// style reads this frame (#743), when it reads them only near its
    /// entities: the edge pass draws the union of its readers' rects and
    /// leaves the rest 0. `None`, the default, is the whole frame. `frame`
    /// is this style's ([`StyleFrame::descriptor`] is its descriptor).
    fn edges_rect(&self, _frame: &StyleFrame<'_>) -> Option<kit::PixelRect> {
        None
    }

    /// The grid this style draws the whole frame in whole cells of, when
    /// it wants the edge of another style's entity snapped to it (#534):
    /// the framework's [`ownership`] cut then gives each cell of the grid
    /// wholly to one side, by how much of it is the other style's, rather
    /// than cutting a cell along the entity's outline. Asked of a display
    /// style once a frame, before the stage runs (`frame.descriptor` is
    /// this style's); only the first style of the stage that answers is
    /// heard. `None` (the default) keeps the cut to the mask's pixel.
    fn ownership_cells(&self, _frame: &StyleFrame<'_>) -> Option<OwnershipCells> {
        None
    }

    /// Hand out the history this style keeps from frame to frame, after a
    /// captured frame (#1136, [`history`]): a part each, with its key and
    /// the textures it read as last frame's, staged in `out`'s encoder. A
    /// style that keeps none (the default) adds nothing.
    fn export_history(&self, _out: &mut history::HistoryExport<'_>) {}

    /// Keep `history`, taken from a capture (#1136), until the next frame
    /// this style draws, and draw that frame from it where its key is the
    /// frame's. The default keeps no history and says so.
    fn import_history(&self, history: history::ImportedHistory) {
        let line = format!("{}: keeps no history to take back", history.state.style);
        history.say(line);
    }
}

/// A whole-frame grid the [`ownership`] cut snaps to (#534), in output
/// pixels: `cell` wide and tall from `origin`, as the style draws it. A
/// cell is the style's (drawn whole, another style's pixels in it
/// included) while at least `share` of its 16 lattice samples are not
/// another style's; otherwise it is the other style's whole, and the
/// style's own pixels in it show what came into the style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnershipCells {
    pub cell: [f32; 2],
    pub origin: [f32; 2],
    /// 1/16..1: the least share of a cell's samples the style must hold.
    pub share: f32,
}

/// The stage's snapped ownership (#534): which style snaps (its index in
/// the frame's registry), its grid, and where another style's ids can be on
/// screen, so the cut leaves every other pixel at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SnappedOwnership {
    pub style: u32,
    pub cells: OwnershipCells,
    /// Output pixels (x0, y0, x1, y1).
    pub rect: [f32; 4],
}

/// The view a style reads, with an id that's stable for as long as the
/// chain's bind groups are.
pub struct StyleInput<'a> {
    pub view: &'a wgpu::TextureView,
    pub id: u64,
}

/// The camera, for styles that read depth (edges, fog-aware washes).
#[derive(Clone, Copy, Debug)]
pub struct StyleCamera {
    /// World to clip, column-major (for projecting an anchor, #266).
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub position: [f32; 3],
    pub near: f32,
    pub far: f32,
}

/// Everything a style may read in one frame.
pub struct StyleFrame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub config: &'a PostProcessConfig,
    pub descriptor: &'a StyleDescriptor,
    /// The registry the frame is drawn with (#742): what the style mask's
    /// ids and the owner table (#384) name styles by.
    pub styles: &'a StyleRegistry,
    /// The size of the input and output, in pixels.
    pub width: u32,
    pub height: u32,
    /// The input's and output's format.
    pub format: wgpu::TextureFormat,
    /// The scene's depth (single-sample; under a render scale, the scaled
    /// scene's). Sample it by UV: at the display stage it is smaller than
    /// the output.
    pub depth: &'a wgpu::TextureView,
    /// [`depth`](Self::depth)'s size in pixels, which a view doesn't say:
    /// the scene's, so smaller than `width` x `height` at the display stage
    /// under a render scale. The style mask is this size too.
    pub depth_size: [u32; 2],
    /// Whose `depth` is, so bind groups over it may be kept
    /// ([`StyleFrame::depth_bind`], #379, #960).
    pub depth_key: crate::bind_cache::DepthKey<'a>,
    pub camera: StyleCamera,
    /// The entity style mask (#263): one style id a pixel
    /// ([`STYLE_MASK_FORMAT`](crate::style_mask::STYLE_MASK_FORMAT),
    /// `texture_2d<u32>`, 0 = unstyled), at the scene's resolution like
    /// [`depth`](Self::depth), so a display-stage style reads it by UV
    /// with `textureLoad`. `Some` whenever a style with
    /// [`uses_mask`](StyleDescriptor::uses_mask) is on. It lives with the
    /// chain; bind it with [`mask_bind`](Self::mask_bind).
    pub mask: Option<&'a wgpu::TextureView>,
    /// Whether some pixel of [`mask`](Self::mask) may hold a blend below 1
    /// this frame: an entity part-way into its style (#278) or a veil in
    /// front of one (#290). The framework's cross-fade (#396) runs only
    /// while it holds.
    pub mask_fades: bool,
    /// Which id each style name has in [`mask`](Self::mask).
    pub style_ids: &'a crate::style_mask::StyleIds,
    /// Where each style id's entities can be on screen this frame (#385),
    /// from the world bounds of the draws the mask draws. Read it through
    /// [`styled_rect`](Self::styled_rect).
    pub style_bounds: &'a crate::style_mask::StyleBounds,
    /// Whether passes are cut to [`styled_rect`](Self::styled_rect) and the
    /// other rects (#385): the renderer's
    /// [`set_style_scissor`](crate::SceneRenderer::set_style_scissor).
    /// Off, they are `None` and the passes draw whole, while
    /// [`styled_area`](Self::styled_area) still says where the entities are.
    pub scissor: bool,
    /// The shared geometry edges (#301, [`edges`] says what the channels
    /// hold): [`EDGE_FORMAT`], the depth's size, read by UV with
    /// `textureLoad`. `Some` whenever a style that
    /// [`needs`](StyleDescriptor::needs) them is on; the framework draws
    /// them once, before the first stage, for every style that reads them.
    /// Bind it with [`edges_bind`](Self::edges_bind).
    pub edges: Option<Arc<StyleTexture>>,
    /// The style albedo (#869): each opaque mesh's own colour before light
    /// ([`ALBEDO_FORMAT`](crate::style_albedo::ALBEDO_FORMAT), linear rgb,
    /// alpha 1 where a mesh was drawn and 0 where none was: the sky,
    /// terrain, grass, the ocean, particles), at the depth's size, read by
    /// UV with `textureLoad`. `Some` whenever a style that
    /// [`uses_albedo`](StyleDescriptor::uses_albedo) is on; key bind groups
    /// over it with [`bind_over`](Self::bind_over) and
    /// [`albedo_or`](Self::albedo_or).
    pub albedo: Option<Arc<StyleTexture>>,
    /// The chain's linear, clamped sampler.
    pub linear_sampler: &'a wgpu::Sampler,
    /// Seconds of game time, for animated styles.
    pub time: f32,
    /// Seconds of game time since the last frame (#342), for history that
    /// decays at a rate rather than a step a frame. Game time, so it is the
    /// fixed step under `--frame-step` and the capture is deterministic.
    /// Read it through [`per_60hz`] to turn a per-60 Hz-frame weight into
    /// this frame's.
    pub dt: f32,
    /// This frame's number: one more than the last frame the renderer drew
    /// through the post chain. A style that keeps history from frame to
    /// frame (ASCII's temporal stability, #277) trusts it only when the
    /// last frame it drew is `frame - 1`, so a frame drawn without it (the
    /// style switched off a while) starts it over.
    pub frame: u64,
    /// Bumped by [`SceneRenderer::reset_style_history`](crate::SceneRenderer::reset_style_history)
    /// (a new scene, a camera cut): history from another epoch is not
    /// this picture's past.
    pub history_epoch: u64,
    /// Physical pixels per logical point of the window (the OS scale
    /// factor). A style sized in points multiplies by it, so its cells keep
    /// their size on a high-DPI display. 1 headless unless asked.
    pub pixels_per_point: f32,
    /// The scene's folder, when the host knows it.
    pub scene_dir: Option<&'a std::path::Path>,
    /// Where a style finds the files it loads (a glyph set, a palette, a
    /// paper): the scene's project and folder, searched by the one rule
    /// (ADR 0114). With no scene, the working directory stands in for the
    /// scene's folder.
    pub assets: &'a flint_core::project::AssetScope,
    /// The renderer's debug shading mode: a style with a debug view of its
    /// own draws it under its mode (ASCII's edges under
    /// [`DebugMode::StyleEdges`](crate::DebugMode::StyleEdges), #349).
    pub debug_mode: crate::DebugMode,
    pub(crate) binds: &'a BindCache,
    pub(crate) targets: &'a StyleTargets,
    /// The grid the stage's ownership cut snaps to (#534), set by
    /// [`run_stage`] from [`StylePass::ownership_cells`]; `None` cuts to
    /// the mask's pixel.
    pub(crate) snapped_ownership: Option<SnappedOwnership>,
}

/// Whether per-entity styles scissor their passes to their entities
/// (#385): on unless `FLINT_STYLE_SCISSOR=0`, which is for checking that
/// the scissored frame is the full one, byte for byte. Read once, unless a
/// host said otherwise first ([`set_scissor_enabled`]).
pub fn scissor_enabled() -> bool {
    use std::sync::atomic::Ordering;
    match SCISSOR_SET.load(Ordering::Relaxed) {
        SCISSOR_ON => true,
        SCISSOR_OFF => false,
        _ => {
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ON.get_or_init(|| flint_core::env::STYLE_SCISSOR.flag().unwrap_or(true))
        }
    }
}

/// Switch the per-entity style scissor for the process, over
/// `FLINT_STYLE_SCISSOR` (#538): the web page's `?scissor=0`, which has no
/// environment, for measuring what the scissor saves in a browser. Call it
/// before the renderer is made, which reads it for its own switch
/// ([`SceneRenderer::set_style_scissor`](crate::SceneRenderer::set_style_scissor)).
pub fn set_scissor_enabled(on: bool) {
    SCISSOR_SET.store(
        if on { SCISSOR_ON } else { SCISSOR_OFF },
        std::sync::atomic::Ordering::Relaxed,
    );
}

const SCISSOR_ON: u8 = 1;
const SCISSOR_OFF: u8 = 2;
static SCISSOR_SET: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// A weight `w` that means "keep this much of last frame, per 60 Hz frame"
/// as this frame's weight, `w^(dt * 60)` (#342): at 120 Hz two frames keep
/// what one 60 Hz frame keeps, at 30 Hz one frame keeps what two do, so a
/// change settles in the same game time at any rate. A `dt` that isn't a
/// positive number says nothing about the rate and reads as 1/60.
pub fn per_60hz(w: f32, dt: f32) -> f32 {
    let dt = if dt.is_finite() && dt > 0.0 {
        dt
    } else {
        1.0 / 60.0
    };
    w.clamp(0.0, 1.0).powf(dt * 60.0)
}

impl StyleFrame<'_> {
    pub fn texel_size(&self) -> [f32; 2] {
        [1.0 / self.width as f32, 1.0 / self.height as f32]
    }

    /// Whether [`format`](Self::format) is sRGB: samples come back linear
    /// and writes are encoded.
    pub fn srgb(&self) -> bool {
        self.format.is_srgb()
    }

    /// This style's parameter `name`, from the config or its default.
    pub fn param(&self, name: &str) -> f32 {
        self.descriptor.param(self.config, name)
    }

    /// This style's option `name`, from the config or its default.
    pub fn option(&self, name: &str) -> &str {
        self.descriptor.option(self.config, name)
    }

    /// This style's colour `name` (`tint`, #315): its `_r`/`_g`/`_b`
    /// channels from the config or their defaults, in display values.
    pub fn color(&self, name: &str) -> [f32; 3] {
        self.descriptor.color(self.config, name)
    }

    /// A texture of the style's own, `width` x `height` in `format`, made
    /// on first use and kept until the chain is rebuilt (on resize).
    /// `name` must be unique across styles; prefix it with the style's.
    pub fn target(&self, name: &'static str, format: wgpu::TextureFormat) -> Arc<StyleTexture> {
        self.targets
            .get(self.device, name, format, self.width, self.height)
    }

    /// [`target`](Self::target) at a size of the style's own, such as one
    /// texel per glyph cell. A name asked for at a new size is made again,
    /// so bind groups over it must be keyed by something that changes with
    /// it (see the ASCII style's history generation).
    pub fn target_sized(
        &self,
        name: &'static str,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Arc<StyleTexture> {
        self.targets.get(self.device, name, format, width, height)
    }

    /// A bind group kept with the chain, made with `make` the first time.
    pub fn bind(
        &self,
        name: &'static str,
        variant: u64,
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        self.binds.get(name, self.keyed(variant), make)
    }

    /// [`bind`](Self::bind) over `over`, what the group binds that may be
    /// made again while the chain lives (the shared edges, the albedo, an
    /// image): kept while those are the same objects, made again in the old
    /// group's place when one isn't (#960).
    pub fn bind_over(
        &self,
        name: &'static str,
        variant: u64,
        over: &[crate::bind_cache::Bound<'_>],
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        self.binds.get_bound(name, self.keyed(variant), over, make)
    }

    /// `variant` told apart by the style's install (#742): a user style
    /// reloaded under its old names never binds the old install's groups.
    /// A built-in's (generation 0) is `variant` itself.
    pub(super) fn keyed(&self, variant: u64) -> u64 {
        match self.descriptor.generation {
            0 => variant,
            g => {
                // splitmix64 of the pair: no install's keys meet another's
                let mut z = variant ^ g.wrapping_mul(0x9e37_79b9_7f4a_7c15);
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                z ^ (z >> 31)
            }
        }
    }

    /// A bind group over [`depth`](Self::depth): kept while the chain owns
    /// the depth, or while the caller hands the same depth view (#379,
    /// #960).
    pub fn depth_bind(
        &self,
        name: &'static str,
        variant: u64,
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        self.depth_bind_over(name, variant, &[], make)
    }

    /// [`depth_bind`](Self::depth_bind) over `over` as well, as
    /// [`bind_over`](Self::bind_over).
    pub fn depth_bind_over(
        &self,
        name: &'static str,
        variant: u64,
        over: &[crate::bind_cache::Bound<'_>],
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        self.binds
            .get_over(self.depth_key, name, self.keyed(variant), over, make)
    }

    /// The shared edges' view, or `none` while there are none, as a
    /// [`Bound`](crate::bind_cache::Bound) for [`bind_over`](Self::bind_over).
    pub fn edges_or<'v>(&'v self, none: &'v wgpu::TextureView) -> crate::bind_cache::Bound<'v> {
        crate::bind_cache::Bound::View(self.edges.as_ref().map_or(none, |e| &e.view))
    }

    /// The style albedo's view, or `none` while there is none, as
    /// [`edges_or`](Self::edges_or) is the edges'.
    pub fn albedo_or<'v>(&'v self, none: &'v wgpu::TextureView) -> crate::bind_cache::Bound<'v> {
        crate::bind_cache::Bound::View(self.albedo.as_ref().map_or(none, |a| &a.view))
    }

    /// True the first time it's asked about `name` while the chain lives:
    /// for uniforms that only change on resize, such as texel sizes.
    pub fn first_write(&self, name: &'static str) -> bool {
        self.binds.first_write(name)
    }

    /// A kept bind group of `view` (binding 0) and the linear sampler
    /// (binding 1), the shape most passes read a texture in. `view` must
    /// live as long as the chain: a [`target`](Self::target) of this style.
    pub fn sampled(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
    ) -> Arc<wgpu::BindGroup> {
        self.bind(name, 0, || self.make_sampled(name, layout, view))
    }

    /// [`sampled`](Self::sampled) over the style's input, keyed by which
    /// view the input is this frame.
    pub fn sampled_input(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
        input: &StyleInput<'_>,
    ) -> Arc<wgpu::BindGroup> {
        self.bind(name, input.id, || {
            self.make_sampled(name, layout, input.view)
        })
    }

    /// A kept bind group of the style mask at binding 0, for a layout made
    /// with [`kit::mask_layout`]; `None` while there's no mask.
    pub fn mask_bind(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
    ) -> Option<Arc<wgpu::BindGroup>> {
        let mask = self.mask?;
        Some(self.bind(name, 0, || {
            self.device
                .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some(name),
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(mask),
                    }],
                })
        }))
    }

    /// A kept bind group of the shared edges at binding 0, for a layout
    /// made with [`kit::texture_layout`]; `None` while no style needs them.
    pub fn edges_bind(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
    ) -> Option<Arc<wgpu::BindGroup>> {
        let edges = self.edges.as_ref()?;
        let over = [crate::bind_cache::Bound::View(&edges.view)];
        Some(self.bind_over(name, 0, &over, || {
            self.device
                .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some(name),
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&edges.view),
                    }],
                })
        }))
    }

    /// The style mask id of style `name` this frame, if an entity asks
    /// for it.
    pub fn style_id(&self, name: &str) -> Option<u16> {
        self.style_ids.id(name)
    }

    /// `(mask id, variant)` for each of this style's per-entity variants
    /// this frame (#266), in id order. Read a variant's settings with
    /// [`variant_param`](Self::variant_param) and
    /// [`variant_option`](Self::variant_option).
    pub fn variants(&self) -> impl Iterator<Item = (u16, &crate::style_mask::StyleVariant)> {
        self.style_ids.variants_of(self.descriptor.name)
    }

    /// Parameter `name` of `variant`: the entity's, else the scene's, else
    /// the style's default.
    pub fn variant_param(&self, variant: &crate::style_mask::StyleVariant, name: &str) -> f32 {
        variant.param(self.descriptor, self.config, name)
    }

    /// Colour `name` of `variant` (#315), channel by channel: the entity's,
    /// else the scene's, else the style's default.
    pub fn variant_color(&self, variant: &crate::style_mask::StyleVariant, name: &str) -> [f32; 3] {
        match self.descriptor.color_setting(name) {
            Some(c) => c.channels.map(|p| self.variant_param(variant, p.name)),
            None => [0.0; 3],
        }
    }

    /// Option `name` of `variant`: the entity's, else the scene's, else the
    /// style's default.
    pub fn variant_option<'v>(
        &'v self,
        variant: &'v crate::style_mask::StyleVariant,
        name: &str,
    ) -> &'v str {
        variant.option(self.descriptor, self.config, name)
    }

    /// The mask ids whose pixels another running style owns (#384,
    /// [`StyleDescriptor::foreign_ids`]). The framework already puts the
    /// input back on them after a style that
    /// [`yields`](StyleDescriptor::yields) draws; a style reads this to
    /// skip work there (pencil's contours, ASCII's text grid).
    pub fn foreign_ids(&self) -> Vec<u16> {
        self.styles
            .foreign_ids(self.descriptor, self.config, self.style_ids)
    }

    /// The owner table the ownership pass reads after this style, or
    /// `None` when it has nothing to do: the style doesn't yield, there is
    /// no mask, a debug view runs one style alone, or no pixel is another
    /// style's.
    pub(crate) fn owner_table(&self) -> Option<Vec<u32>> {
        if !self.descriptor.yields
            || self.mask.is_none()
            || matches!(
                self.debug_mode,
                crate::DebugMode::StyleMask | crate::DebugMode::StyleEdges
            )
        {
            return None;
        }
        let me = self.styles.index_of(self.descriptor.name)? as u32;
        let table = self.styles.owner_table(self.config, self.style_ids);
        // Under a snapped cut (#534) a style that isn't the snapping one
        // gives back the other styles' pixels in the cells the snapping
        // style holds, even with nothing of its own to cut
        let snapped = self.snapped_ownership.is_some_and(|s| s.style != me);
        (snapped || table.iter().any(|&o| o != ownership::NONE && o != me)).then_some(table)
    }

    /// Where the mask can hold an id another running style owns (#534),
    /// as [`mask_rect`](Self::mask_rect) is for this style's own ids:
    /// `None` when they could be anywhere, empty when none is in view.
    pub fn foreign_rect(&self, pad: u32) -> Option<kit::PixelRect> {
        let foreign = self.foreign_ids();
        self.ids_rect(pad, |id| foreign.binary_search(&id).is_ok())
    }

    /// Whether `config` switches this style on for the whole frame, rather
    /// than only for the entities that ask for it.
    pub fn whole_frame(&self) -> bool {
        self.descriptor.is_enabled(self.config)
    }

    /// Whether the style draws every pixel this frame: its table is on and
    /// its `scope` (where it has one) isn't `entities`. Otherwise it draws
    /// only its entities' pixels and passes the rest through.
    pub fn covers_frame(&self) -> bool {
        self.whole_frame() && self.option("scope") != "entities"
    }

    /// The pixels of this frame (`width` x `height`) the style can change
    /// (#385): the screen bounds of its per-entity variants' draws (opt-outs
    /// left out), grown by `pad` pixels for how far a styled pixel reaches
    /// (a cell, a blur) and by a pixel or two for the mask's own rounding.
    /// `None` when that could be anywhere: the style
    /// [`covers_frame`](Self::covers_frame), one of its draws has no bounds
    /// or crosses the camera plane, or `FLINT_STYLE_SCISSOR=0` turns the
    /// scissor off. An empty rect means no entity of the style is in view.
    ///
    /// Outside it a per-entity style's passthrough holds, so a pass that
    /// writes the input there ([`kit::scissored_pass`]) is the full pass.
    pub fn styled_rect(&self, pad: u32) -> Option<kit::PixelRect> {
        if !self.scissors() {
            return None;
        }
        self.styled_area(pad)
    }

    /// [`styled_rect`](Self::styled_rect) whether or not the scissor is on
    /// (#930): where the style's entities are, for state a style keeps only
    /// around them, so that the frame is the same with the scissor off.
    /// ASCII's edge cells are its one user: what lies outside the area is
    /// empty either way, and a cell's edge history was what an earlier
    /// frame left there only while the scissor was on. `None` as for
    /// `styled_rect` when the entities could be anywhere.
    pub fn styled_area(&self, pad: u32) -> Option<kit::PixelRect> {
        if self.covers_frame() {
            return None;
        }
        self.variants_area(pad, |_, v| !v.off)
    }

    /// Whether the scissor is on, for this renderer and the process
    /// (`FLINT_STYLE_SCISSOR`).
    fn scissors(&self) -> bool {
        self.scissor && scissor_enabled()
    }

    /// The pixels of this frame where the style mask can hold one of the
    /// style's ids (#507): the screen bounds of every per-entity variant's
    /// draws, opt-outs included, grown by `pad` and the mask's rounding as
    /// [`styled_rect`](Self::styled_rect) is, whether or not the style
    /// covers the frame. Outside it every mask texel within `pad` pixels
    /// reads as none of the style's, so a whole-frame pass can skip its
    /// mask reads there. `None` when the ids could be anywhere (as for
    /// `styled_rect`, `FLINT_STYLE_SCISSOR=0` included); empty when none is
    /// in view.
    pub fn mask_rect(&self, pad: u32) -> Option<kit::PixelRect> {
        self.variants_rect(pad, |_, _| true)
    }

    /// [`mask_rect`](Self::mask_rect) of the one variant with mask id `id`:
    /// where its own pixels can be.
    pub fn variant_rect(&self, id: u16, pad: u32) -> Option<kit::PixelRect> {
        self.variants_rect(pad, |of, _| of == id)
    }

    pub(super) fn variants_rect(
        &self,
        pad: u32,
        with: impl Fn(u16, &crate::style_mask::StyleVariant) -> bool,
    ) -> Option<kit::PixelRect> {
        if !self.scissors() {
            return None;
        }
        self.variants_area(pad, with)
    }

    /// [`variants_rect`](Self::variants_rect) with the scissor on or off.
    fn variants_area(
        &self,
        pad: u32,
        with: impl Fn(u16, &crate::style_mask::StyleVariant) -> bool,
    ) -> Option<kit::PixelRect> {
        let mine: Vec<u16> = self
            .variants()
            .filter(|&(id, v)| with(id, v))
            .map(|(id, _)| id)
            .collect();
        self.ids_area(pad, |id| mine.contains(&id))
    }

    /// [`ids_area`](Self::ids_area) while the scissor is on, else `None`.
    fn ids_rect(&self, pad: u32, with: impl Fn(u16) -> bool) -> Option<kit::PixelRect> {
        if !self.scissors() {
            return None;
        }
        self.ids_area(pad, with)
    }

    /// The screen bounds of the ids `with` picks, grown by `pad` and the
    /// mask's rounding ([`variants_rect`](Self::variants_rect)).
    fn ids_area(&self, pad: u32, with: impl Fn(u16) -> bool) -> Option<kit::PixelRect> {
        let mut uv = crate::style_mask::IdBounds::Nowhere;
        for (id, _) in self.style_ids.iter() {
            if with(id) {
                uv = uv.union(self.style_bounds.get(id));
            }
        }
        let [x0, y0, x1, y1] = match uv {
            crate::style_mask::IdBounds::Everywhere => return None,
            crate::style_mask::IdBounds::Nowhere => return Some(kit::PixelRect::EMPTY),
            crate::style_mask::IdBounds::Uv(r) => r,
        };
        let (w, h) = (self.width as f32, self.height as f32);
        let rect = kit::PixelRect {
            x: (x0 * w).floor().max(0.0) as u32,
            y: (y0 * h).floor().max(0.0) as u32,
            width: 0,
            height: 0,
        };
        let right = ((x1 * w).ceil().max(0.0) as u32).min(self.width);
        let bottom = ((y1 * h).ceil().max(0.0) as u32).min(self.height);
        let rect = kit::PixelRect {
            width: right.saturating_sub(rect.x),
            height: bottom.saturating_sub(rect.y),
            ..rect
        };
        // The mask is drawn at the depth's size: a display pixel reads the
        // mask texel under it, up to one texel (of this frame's pixels)
        // past the projected box, and MSAA or a rasterizer's rounding a
        // pixel more
        let texel = (self.width as f32 / self.depth_size[0].max(1) as f32)
            .max(self.height as f32 / self.depth_size[1].max(1) as f32)
            .ceil() as u32;
        let slack = 2 + texel;
        let rect = rect.padded(pad.saturating_add(slack), self.width, self.height);
        tracing::trace!(
            style = self.descriptor.name,
            pad,
            x = rect.x,
            y = rect.y,
            w = rect.width,
            h = rect.height,
            "styled rect"
        );
        Some(rect)
    }

    /// Where world point `p` lands in this frame, in pixels from the top
    /// left; `None` behind the camera.
    pub fn project(&self, p: [f32; 3]) -> Option<[f32; 2]> {
        let m = &self.camera.view_proj;
        let row = |r: usize| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
        let w = row(3);
        if !w.is_finite() || w <= 1e-6 {
            return None;
        }
        let (x, y) = (row(0) / w, row(1) / w);
        Some([
            (x * 0.5 + 0.5) * self.width as f32,
            (0.5 - y * 0.5) * self.height as f32,
        ])
    }

    /// A kept bind group of one uniform buffer at binding 0.
    pub fn uniform(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
    ) -> Arc<wgpu::BindGroup> {
        self.bind(name, 0, || {
            self.device
                .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some(name),
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                })
        })
    }

    pub(super) fn make_sampled(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.device
            .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some(name),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(self.linear_sampler),
                    },
                ],
            })
    }
}
