//! The ASCII passes' uniform blocks, edge formats and layouts (moved from
//! `ascii.rs`, #742).

use crate::gpu_counts::CountedDevice;
use super::*;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct AsciiUniforms {
    pub(super) cell: [f32; 2],
    pub(super) origin: [f32; 2],
    pub(super) glyph: [f32; 2],
    pub(super) size: [f32; 2],
    pub(super) tint: [f32; 3],
    pub(super) color_mode: u32,
    pub(super) brightness: f32,
    pub(super) contrast: f32,
    pub(super) background: f32,
    pub(super) ink: f32,
    pub(super) levels: f32,
    pub(super) footprint: f32,
    pub(super) ramp_len: u32,
    pub(super) srgb: u32,
    pub(super) glow: f32,
    pub(super) ground: u32,
    pub(super) palette_len: u32,
    pub(super) palette_match: u32,
    pub(super) palette_bg: [f32; 3],
    /// What inks the line glyphs (#351): 0 cell, 1 color, 2 hue.
    pub(super) edge_ink: u32,
    pub(super) smoothing: f32,
    pub(super) hysteresis: f32,
    pub(super) dither: f32,
    pub(super) history: u32,
    /// Line glyphs' strength over the tone glyph (#267).
    pub(super) edges: f32,
    /// Whether the edge passes ran this frame.
    pub(super) edge_on: u32,
    /// The whole frame's exposure in stops and tone stretch (#289).
    pub(super) exposure: f32,
    pub(super) stretch: f32,
    /// The whole frame's palette chroma and ink dither (#320).
    pub(super) chroma: f32,
    pub(super) palette_dither: f32,
    /// For edge ink color: how far the colour mixes toward the near
    /// side's hue (#351).
    pub(super) edge_tint: f32,
    /// How many corner glyphs the whole frame's font has: 0, 4 or 9 (#348).
    pub(super) corners: u32,
    /// The edge glyphs' atlas origins, in `EdgeGlyphs::slots()` order (the
    /// four lines, then the corners, #348), two a vec4.
    pub(super) edge_glyphs: [[u32; 4]; EDGE_SLOTS.div_ceil(2)],
    /// Every variant's ramp, one after another (the whole frame's first):
    /// each step's atlas cell origin, two steps a vec4.
    pub(super) ramp: [[u32; 4]; MAX_RAMP / 2],
    /// Every variant's palette inks, one palette after another (the whole
    /// frame's first; display values, rgb, unused w).
    pub(super) palette: [[f32; 4]; MAX_COLORS],
    /// The same inks in OKLab (#320; L, a, b, unused w), for matching.
    pub(super) palette_lab: [[f32; 4]; MAX_COLORS],
    /// x: rows of `variants` in use (1 = the whole frame alone); y: 1 when
    /// the whole frame passes through (only entities asked, #266); z:
    /// bands (#313); w: grids in `snap_grids` (#338).
    pub(super) variant_meta: [u32; 4],
    /// The variant cell state's bands (#313), `variant_meta[2]` of them:
    /// each grid's cell and origin, and its (first row, rows, columns,
    /// footprint's f32 bits).
    pub(super) bands: [[f32; 4]; MAX_BANDS],
    pub(super) band_rows: [[u32; 4]; MAX_BANDS],
    /// The grids of the variants whose silhouettes snap to whole cells
    /// (#338), `variant_meta[3]` of them: (cell, origin).
    pub(super) snap_grids: [[f32; 4]; MAX_SNAP_GRIDS],
    /// Reprojected history (#344): this frame's clip to world, last
    /// frame's world to clip (column-major), and x = 1 to read each cell's
    /// past where its surface was last frame.
    pub(super) inv_view_proj: [[f32; 4]; 4],
    pub(super) prev_view_proj: [[f32; 4]; 4],
    pub(super) reproject: [u32; 4],
    /// The line glyphs' ink for edge ink color (#351), display rgb, unused w.
    pub(super) edge_color: [f32; 4],
    /// The dissolving variants' bands (#398): x = how many.
    pub(super) dissolve_meta: [u32; 4],
    /// Each one's (slot, first row, rows, columns).
    pub(super) dissolve: [[u32; 4]; MAX_DISSOLVE],
    /// Beside `ramp`, each step's least tone for a calibrated ramp (#281),
    /// four a vec4; [`EQUAL_BANDS`] at a ramp's first step for equal bands.
    pub(super) ramp_tone: [[f32; 4]; MAX_RAMP / 4],
    /// Where the style mask can hold a variant's id (#507), output pixels
    /// (x0, y0, x1, y1): outside it the passes read no mask
    /// ([`StyleFrame::mask_rect`], the whole frame when that is `None`).
    pub(super) mask_rect: [f32; 4],
    /// Each of `snap_grids`' band in the snap owner state (#929,
    /// [`OwnerBand::packed`]); zeros for a grid without one.
    pub(super) snap_bands: [[i32; 4]; MAX_SNAP_GRIDS],
}

/// One per-entity variant's settings (#266), a row of the variant table's
/// storage buffer (#336). Row 0 is unused: the whole frame draws from the
/// uniform's own fields and the cell state.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct VariantUniform {
    pub(super) cell: [f32; 2],
    pub(super) origin: [f32; 2],
    pub(super) tint: [f32; 3],
    pub(super) color_mode: u32,
    pub(super) brightness: f32,
    pub(super) contrast: f32,
    pub(super) background: f32,
    pub(super) ink: f32,
    pub(super) levels: f32,
    pub(super) footprint: f32,
    pub(super) ramp_start: u32,
    pub(super) ramp_len: u32,
    pub(super) flags: u32,
    pub(super) glow: f32,
    pub(super) ground: u32,
    pub(super) palette_start: u32,
    pub(super) palette_bg: [f32; 3],
    pub(super) palette_len: u32,
    pub(super) palette_match: u32,
    /// Exposure in stops and tone stretch, 0..1 (#289).
    pub(super) exposure: f32,
    pub(super) stretch: f32,
    /// The first row + 1 of the variant's band in the variant cell state
    /// (#313); 0 when it has none and the glyph pass samples for itself.
    pub(super) cell_row: u32,
    /// Silhouette outline width in output pixels (#288; 0 = off).
    pub(super) outline: f32,
    /// The least ink of a cell this variant only grazes (#288).
    pub(super) min_coverage: f32,
    /// Ordered dither between the two nearest inks, 0..1 (#320).
    pub(super) palette_dither: f32,
    /// Palette chroma before matching (#320).
    pub(super) chroma: f32,
    /// The variant's font's glyph cell in atlas pixels (#335).
    pub(super) glyph: [f32; 2],
    /// Where the variant's edge glyphs sit in the ramp table, or
    /// [`NO_EDGE_START`] for the whole frame's (a variant in its font).
    pub(super) edge_start: u32,
    /// With [`SNAP`], the least share of a cell's 16 samples the variant
    /// must cover to draw the cell whole (#338).
    pub(super) cell_share: f32,
    /// The first row + 1 of the variant's band in the dissolve rank state
    /// (#398); 0 when it has none and its cells take their order as it is.
    pub(super) dissolve_row: u32,
    /// How many corner glyphs the variant's own font has (#348); read only
    /// with an `edge_start` of its own.
    pub(super) corners: u32,
    pub(super) _pad_dissolve: [u32; 2],
    /// Where the mask can hold the variant's id (#507), output pixels
    /// (x0, y0, x1, y1): its draws' screen bounds and the mask's rounding
    /// ([`StyleFrame::variant_rect`]), the whole frame when they have none.
    pub(super) bounds: [f32; 4],
    /// With [`SNAP`], its grid's band in the snap owner state (#929,
    /// [`OwnerBand::packed`]); zeros for none.
    pub(super) owner: [i32; 4],
}

/// The rect [`rect_corners`] made, back.
pub(super) fn corners_rect(c: [f32; 4]) -> kit::PixelRect {
    kit::PixelRect {
        x: c[0] as u32,
        y: c[1] as u32,
        width: (c[2] - c[0]).max(0.0) as u32,
        height: (c[3] - c[1]).max(0.0) as u32,
    }
}

/// A rect as the shaders take one, (x0, y0, x1, y1) in pixels; `None` is
/// the whole `size` frame.
pub(super) fn rect_corners(rect: Option<kit::PixelRect>, size: [u32; 2]) -> [f32; 4] {
    rect.map_or([0.0, 0.0, size[0] as f32, size[1] as f32], |r| {
        [
            r.x as f32,
            r.y as f32,
            (r.x + r.width) as f32,
            (r.y + r.height) as f32,
        ]
    })
}

/// A variant whose edge glyphs are the uniform's own; `NO_EDGE_START` in
/// ascii_common.wgsl.
pub(super) const NO_EDGE_START: u32 = u32::MAX;

/// The most glyphs an edge cell can draw (#348): four lines, four corners,
/// four tees and the cross, numbered so in the shaders
/// (`ascii_edge_slot.wgsl`).
pub(super) const EDGE_SLOTS: usize = 13;

/// What the edge pixels are read with (#743), `EdgeSource` in
/// `ascii_edge_source.wgsl`: the first member of the edge cell pass's and
/// the edge view's uniforms.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct EdgeSource {
    pub(super) inv_view_proj: [[f32; 4]; 4],
    pub(super) camera_pos: [f32; 3],
    /// The plain `ascii` id (0 = none); the variants' ids are the ones the
    /// variant table gives a slot (#336).
    pub(super) plain_id: u32,
    /// The shared `edge_depth` over the style's: the shared depth strength
    /// times this is the style's.
    pub(super) depth_scale: f32,
    /// The shared `edge_normal` over the style's.
    pub(super) normal_scale: f32,
    /// 1 while the mask is bound, so the styled outlines are looked for.
    pub(super) outlines: u32,
    pub(super) _pad: u32,
}

/// The edge cell pass's uniforms (#267), `ascii_edge_cells.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct EdgeCellUniforms {
    pub(super) src: EdgeSource,
    pub(super) cell: [f32; 2],
    pub(super) origin: [f32; 2],
    pub(super) size: [f32; 2],
    pub(super) threshold: f32,
    pub(super) max_len: f32,
    pub(super) edge_size: [f32; 2],
    /// Whether the picture arrives linear, to be read as display values
    /// for the near side's hue (#351).
    pub(super) srgb: u32,
    /// The coherence under which a cell is marked as a corner (#348; 0 off).
    pub(super) corner: f32,
    /// 1 when the past edge cells hold last frame's (#477).
    pub(super) history: u32,
    /// How far an edge's measures may stray before its decisions change:
    /// the style's `hysteresis`.
    pub(super) hold: f32,
    /// How much of last frame's length and direction a cell keeps: the
    /// style's `smoothing`, for this frame's length (#342).
    pub(super) smoothing: f32,
    pub(super) _pad: u32,
}

/// The edge thin pass's uniforms (#477), `ascii_edge_thin.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct EdgeThinUniforms {
    /// 1 when the past edge slots hold last frame's.
    pub(super) history: u32,
    /// Cells past the border a line may go and stay with the cell that drew
    /// it last frame.
    pub(super) reach: f32,
    /// Rows of the variant table in use (1 = the whole frame alone), and
    /// 1 when the whole frame's cells are drawn, not passed through: with
    /// the grid, what says which cells draw a line (#493). Here and not
    /// read from the ASCII uniform, which is written after the edge passes
    /// and not at all under the edge view.
    pub(super) variants: u32,
    pub(super) whole_frame: u32,
    pub(super) cell: [f32; 2],
    pub(super) origin: [f32; 2],
    pub(super) size: [f32; 2],
    pub(super) _pad: [u32; 2],
}

/// How far past a cell's border, in cells per unit of `hysteresis`, a line
/// stays with the cell that drew it last frame (#477): the default 0.35
/// holds it a third of a cell past. Measured on the motion bench (reach
/// 0.175 left 1.7 reversals a frame, 0.35 left 0.9).
pub(super) const EDGE_REACH: f32 = 1.0;

/// The edge debug view's uniforms (#349), `ascii_edge_view.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct EdgeViewUniforms {
    pub(super) src: EdgeSource,
    pub(super) cell: [f32; 2],
    pub(super) origin: [f32; 2],
    pub(super) size: [f32; 2],
    pub(super) edge_size: [f32; 2],
    pub(super) threshold: f32,
    pub(super) max_len: f32,
    pub(super) srgb: f32,
    pub(super) dim: f32,
    pub(super) thick: f32,
    /// How many corner glyphs the frame's font has (#348): the view draws
    /// the corners the glyphs do.
    pub(super) corners: u32,
    pub(super) _pad: [f32; 2],
}

/// The edge debug view's colours (#349), as `ascii_edge_view.wgsl` paints
/// them (display values), each with what it marks: the legend `flint
/// render --debug-mode edges` draws and F4 lists.
pub const EDGE_VIEW_LEGEND: &[([f32; 3], &str)] = &[
    ([0.25, 0.85, 1.0], "depth step"),
    ([1.0, 0.68, 0.2], "crease"),
    ([1.0, 0.35, 0.85], "styled outline"),
    ([0.4, 1.0, 0.45], "cell draws a line glyph"),
    ([0.25, 0.55, 0.3], "cell yields its line (dashed)"),
    ([1.0, 0.9, 0.3], "cell draws a corner or tee (#348)"),
    ([1.0, 0.25, 0.2], "past edge_max: texture"),
    ([0.55, 0.55, 0.55], "under edge_threshold"),
];

/// The four edge keys the view (#349) is read against, as `config` sets
/// them: `edge_threshold 0.5, edge_max 3 (cell widths); edge_depth 0.6,
/// edge_normal 2.5`.
pub fn edge_view_thresholds(config: &crate::postprocess::PostProcessConfig) -> String {
    let p = |name: &str| STYLE.param(config, name);
    format!(
        "edge_threshold {}, edge_max {} (cell widths); edge_depth {}, edge_normal {}",
        p("edge_threshold"),
        p("edge_max"),
        p("edge_depth"),
        p("edge_normal")
    )
}

/// One texel a glyph cell: its line glyph's slot + 1, edge length, outline.
/// 32-bit, so z holds the outline and cross-axis bits and the near side's hue above
/// them as an exact integer (#351); x, y and w are kept at f16 precision,
/// as they were.
pub(super) const EDGE_CELL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// The edge thin and corner passes' texel a cell (#910): slots packed as
/// `ascii_edge_slot.wgsl` packs them, 20 bits since the corner pass keeps
/// what it found beside what the cell draws (#477).
pub(super) const EDGE_SLOT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// How many cells bigger than the grid the thin pass's texture is on each
/// side (`THIN_PAD` in ascii_edge_slot.wgsl).
pub(super) const EDGE_THIN_PAD: u32 = 1;
/// How many of the largest cell in play the glyph pass's rect reaches past
/// the styled entities' screen bounds (#385): a whole cell (a snapped
/// silhouette) round any styled pixel, and one more.
pub(super) const GLYPH_RECT_CELLS: u32 = 2;
/// How many cells from a cell the edge cells its glyph depends on this
/// frame can be (#930): the corner pass reads the thin pass's texels a
/// cell round it (`ascii_edge_corners.wgsl`, the diagonal ones too), and a
/// thin texel's arms read the lines of its four neighbours, each of which
/// reads the neighbour it leans to (`ascii_edge_thin.wgsl`): one plus two,
/// along an axis. Every cell within this of a cell the glyph pass draws is
/// one the edge cell pass writes: `edge_regions` takes the cells under the
/// glyph rect (`GLYPH_RECT_CELLS` round the entities) and this many less
/// that round them. A pass that reads further raises it.
pub(super) const EDGE_CORNER_REACH: u32 = 3;


/// A fragment uniform buffer at binding 0.
pub(super) fn uniform_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// A fragment-stage texture at binding 0 read with `textureLoad`.
pub(super) fn load_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

/// A target of the style's own at a size the chain's don't come in: the
/// edge pixels at the depth's size, the edge cells at the grid's. Kept
/// while the size holds; `generation` keys bind groups over it.
pub(super) struct Owned {
    pub(super) size: [u32; 2],
    pub(super) generation: u64,
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    pub(super) bind: wgpu::BindGroup,
}

impl Owned {
    /// `slot`'s target, remade at `size` when it's another size.
    pub(super) fn ensure<'s>(
        slot: &'s mut Option<Owned>,
        device: &wgpu::Device,
        label: &'static str,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        layout: &wgpu::BindGroupLayout,
    ) -> &'s Owned {
        let size = [size[0].max(1), size[1].max(1)];
        if slot.as_ref().is_none_or(|o| o.size != size) {
            let generation = slot.as_ref().map_or(0, |o| o.generation + 1);
            let texture = device.create_texture_counted(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                // Copied too: the edge cells and slots keep last frame's in
                // a second texture (#477)
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            *slot = Some(Owned {
                size,
                generation,
                texture,
                view,
                bind,
            });
        }
        slot.as_ref().expect("made above")
    }
}
