//! The style's state: ramps, fonts, palettes, the text capture and each
//! variant's row (moved from `ascii.rs`, #742).

use crate::gpu_counts::CountedDevice;
use super::*;

/// Up to [`MAX_RAMP`] steps of `ramp`, evenly resampled when longer: each
/// step's atlas index and its `ramp_tone` entry (#281), the least tone it
/// draws at for a calibrated ramp, [`EQUAL_BANDS`] on every step of an
/// equal-band one.
pub(super) fn fit_ramp(ramp: &GlyphRamp) -> Vec<(u32, f32)> {
    let indices = ramp.atlas_indices();
    let picked = fit_positions(indices.len());
    let tones = match ramp.map() {
        RampMap::Index => vec![EQUAL_BANDS; picked.len()],
        // The steps kept, banded among themselves
        RampMap::Coverage => calibrated_bounds(
            &picked
                .iter()
                .map(|i| ramp.coverage()[*i])
                .collect::<Vec<_>>(),
        ),
    };
    picked
        .iter()
        .zip(tones)
        .map(|(i, t)| (indices[*i], t))
        .collect()
}

/// The steps of a `len`-step ramp kept in [`MAX_RAMP`]: all of them, or an
/// even resampling end to end.
pub(super) fn fit_positions(len: usize) -> Vec<usize> {
    if len <= MAX_RAMP {
        (0..len).collect()
    } else {
        (0..MAX_RAMP)
            .map(|i| i * (len - 1) / (MAX_RAMP - 1))
            .collect()
    }
}

/// A `ramp_tone` entry saying the ramp it starts takes equal bands, as
/// every ramp did before #281; `EQUAL_BANDS` in ascii_common.wgsl.
pub(super) const EQUAL_BANDS: f32 = -1.0;

/// The sets in use and their atlases stacked on the GPU (#335).
pub(super) struct LoadedStack {
    pub(super) sets: Vec<Arc<GlyphSet>>,
    pub(super) gpu: GpuGlyphStack,
    pub(super) bind: wgpu::BindGroup,
}

impl LoadedStack {
    /// Glyph `index` of font `font`'s origin in the stack.
    pub(super) fn origin(&self, font: u32, index: u32) -> [u32; 2] {
        let font = font as usize;
        let [x, y] = self.sets[font].atlas().cell_origin(index);
        [x, y + self.gpu.tops[font]]
    }
}

#[derive(Default)]
pub(super) struct Fonts {
    pub(super) cache: GlyphSetCache,
    pub(super) palettes: PaletteCache,
    pub(super) stack: Option<LoadedStack>,
    /// The last (font, ramp) a warning was given for, so it's said once.
    pub(super) warned: Option<(String, String)>,
    /// Frames drawn since the caches last looked at the disk (#323).
    pub(super) since_poll: u32,
}

/// The ASCII style asks its caches whether a palette or glyph set file
/// changed every this many frames: a quarter second at 60 fps.
pub(super) const POLL_FRAMES: u32 = 15;

pub(super) struct Ascii {
    pub(super) cells_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    /// Per-entity variant cells (#313): their pass and its two entries.
    pub(super) variant_cells_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) variant_cells: Mutex<[Option<Owned>; 2]>,
    /// Which snapping variant holds each cell (#929): its pass and state,
    /// a band of rows a snapping grid.
    pub(super) snap_owner_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) snap_owners: Mutex<Option<Owned>>,
    /// The dissolve's order and rank state (#398), a band of rows per
    /// dissolving variant.
    pub(super) dissolve_order_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) dissolve_rank_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) dissolve: Mutex<[Option<Owned>; 2]>,
    pub(super) stats_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) stats_resolve_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) stats_rows_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) stats_in_bgl: wgpu::BindGroupLayout,
    /// Tone statistics (#289): per (slot, run of a lattice row, #337), per
    /// (slot, row), then per slot.
    pub(super) stats_runs: Mutex<Option<Owned>>,
    pub(super) stats_rows: Mutex<Option<Owned>>,
    pub(super) stats: Mutex<Option<Owned>>,
    pub(super) pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) uniforms: wgpu::Buffer,
    pub(super) uniform_bgl: wgpu::BindGroupLayout,
    /// The uniform and the variant table (#336), and the edge cell pass's
    /// and edge view's uniform with the slots.
    pub(super) table_bgl: wgpu::BindGroupLayout,
    pub(super) edge_bgl: wgpu::BindGroupLayout,
    /// The variant table (#336, the framework's since #740).
    pub(super) table: Mutex<VariantTable>,
    pub(super) input_bgl: wgpu::BindGroupLayout,
    pub(super) cells_bgl: wgpu::BindGroupLayout,
    pub(super) history_bgl: wgpu::BindGroupLayout,
    pub(super) atlas_bgl: wgpu::BindGroupLayout,
    pub(super) entity_bgl: wgpu::BindGroupLayout,
    pub(super) _no_mask: wgpu::Texture,
    pub(super) no_mask_view: wgpu::TextureView,
    /// The input through, outside the styled entities' rect (#385).
    pub(super) blit: kit::Blit,
    pub(super) fonts: Mutex<Fonts>,
    pub(super) history: Mutex<History>,
    /// History taken from a frame capture (#1136), until the next frame.
    pub(super) imported: crate::stylize::history::PendingHistory,
    /// The mask's outlines (#743, `ascii_edge_outline.wgsl`), a fragment a
    /// depth pixel into `outlines`, drawn only while the mask is bound.
    pub(super) outline_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    /// The edge cell pass (#267), summing the edge pixels
    /// (`ascii_edge_source.wgsl`: the shared edges and the mask's outlines,
    /// #743) over each glyph cell.
    pub(super) edge_cell_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) edge_cell_uniforms: wgpu::Buffer,
    /// The edge debug view (#349), drawn instead of the glyphs under
    /// `DebugMode::StyleEdges`.
    pub(super) edge_view_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) edge_view_uniforms: wgpu::Buffer,
    pub(super) edge_view_cells_bgl: wgpu::BindGroupLayout,
    /// Each edge cell's glyph, decided once per cell (#910): the thin pass
    /// (`ascii_edge_thin.wgsl`, into `edge_thin`, [`EDGE_THIN_PAD`] bigger
    /// than the grid on every side), then the corner pass
    /// (`ascii_edge_corners.wgsl`, into `edge_slots`), of which the glyph
    /// pass and the edge view read one texel a cell.
    pub(super) edge_thin_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) edge_corner_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    /// The thin pass's last group (#493): last frame's edge slots and the
    /// style mask.
    pub(super) thin_in_bgl: wgpu::BindGroupLayout,
    pub(super) edges_in_bgl: wgpu::BindGroupLayout,
    /// The edge pixels' inputs (#743): the shared edges, mask and depth.
    pub(super) source_bgl: wgpu::BindGroupLayout,
    /// The edge cell pass's last group: last frame's cells and the outlines.
    pub(super) past_outline_bgl: wgpu::BindGroupLayout,
    /// The outline pass's target, at the depth's size.
    pub(super) outlines: Mutex<Option<Owned>>,
    pub(super) _no_edges: wgpu::Texture,
    pub(super) no_edges_view: wgpu::TextureView,
    pub(super) edge_cells: Mutex<Option<Owned>>,
    pub(super) edge_thin: Mutex<Option<Owned>>,
    pub(super) edge_slots: Mutex<Option<Owned>>,
    /// Last frame's edge cells and slots, copied before this frame's
    /// passes write theirs, and whether they follow on (#477).
    pub(super) edge_past: Mutex<EdgePast>,
    /// The textures above as a frame capture reads them (#1161), noted
    /// after the pass that writes each.
    pub(super) held: crate::stylize::HeldTargets,
    pub(super) edge_thin_uniforms: wgpu::Buffer,
    /// The glyph grid as text (#317): `fs_text` in ascii.wgsl, drawn only
    /// while `capture` is on, into `text` and copied to its read-back.
    pub(super) text_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    pub(super) capture: AtomicBool,
    pub(super) text: Mutex<Option<TextCapture>>,
}

/// The text pass's texel per cell: glyph code, ink, ground, styled.
pub(super) const TEXT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;
pub(super) const TEXT_TEXEL_BYTES: u32 = 16;

/// The text pass's target and read-back for one grid size, and what the
/// last frame copied into it needs to become text.
pub(super) struct TextCapture {
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    pub(super) buffer: wgpu::Buffer,
    pub(super) grid: [u32; 2],
    /// Bytes a row of the read-back, padded to wgpu's copy alignment.
    pub(super) row_bytes: u32,
    /// The grid's first visible column and row: the offset texel is off
    /// screen when the grid starts at the frame's edge.
    pub(super) first: [u32; 2],
    /// Each glyph's atlas origin (packed `x | y << 16`) to its character.
    pub(super) glyphs: Vec<(u32, char)>,
    /// Mask ids a display style after ASCII draws over: their cells show
    /// that style in the finished frame, so they're written as passed.
    pub(super) overdrawn: Vec<u32>,
    /// Whether a frame has been copied in since the last read.
    pub(super) copied: bool,
}

impl TextCapture {
    pub(super) fn new(device: &wgpu::Device, grid: [u32; 2]) -> Self {
        let texture = device.create_texture_counted(&wgpu::TextureDescriptor {
            label: Some("ASCII Text"),
            size: wgpu::Extent3d {
                width: grid[0].max(1),
                height: grid[1].max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let row_bytes = (grid[0].max(1) * TEXT_TEXEL_BYTES).div_ceil(align) * align;
        let buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("ASCII Text Read-back"),
            size: (row_bytes * grid[1].max(1)) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            texture,
            view,
            buffer,
            grid,
            row_bytes,
            first: [0, 0],
            glyphs: Vec::new(),
            overdrawn: Vec::new(),
            copied: false,
        }
    }

    /// The copied frame as text; waits on the device for the copy.
    pub(super) fn read(&mut self, device: &wgpu::Device) -> Option<GlyphText> {
        if !self.copied {
            return None;
        }
        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = crate::wait_for_gpu(device);
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().expect("mapped buffer range");
        let words = |x: u32, y: u32| -> [u32; 4] {
            let at = (y * self.row_bytes + x * TEXT_TEXEL_BYTES) as usize;
            let w: &[u8] = &data[at..at + TEXT_TEXEL_BYTES as usize];
            std::array::from_fn(|i| u32::from_le_bytes(w[i * 4..i * 4 + 4].try_into().unwrap()))
        };
        let [cols, rows] = [
            self.grid[0].saturating_sub(self.first[0]),
            self.grid[1].saturating_sub(self.first[1]),
        ];
        let mut cells = Vec::with_capacity((cols * rows) as usize);
        for y in 0..rows {
            for x in 0..cols {
                let [code, ink, ground, id] = words(x + self.first[0], y + self.first[1]);
                let ours = code != text::TEXT_NONE && !self.overdrawn.contains(&id);
                let glyph = ours.then(|| {
                    self.glyphs
                        .iter()
                        .find(|(o, _)| *o == code)
                        .map_or('?', |(_, c)| *c)
                });
                // Over a cell another style draws, the ground alone
                let ground = text::unpack_rgb(ground);
                let ink = if code != text::TEXT_NONE && !ours {
                    ground
                } else {
                    text::unpack_rgb(ink)
                };
                cells.push(GlyphCell { glyph, ink, ground });
            }
        }
        drop(data);
        self.buffer.unmap();
        self.copied = false;
        Some(GlyphText { cols, rows, cells })
    }
}

/// A variant's settings, read through the getters of the whole frame or of
/// one per-entity variant.
pub(super) fn variant_uniform(
    param: &dyn Fn(&str) -> f32,
    option: &dyn Fn(&str) -> String,
    cell: [f32; 2],
    origin: [f32; 2],
    ramp: (u32, u32),
    palette: &PalettePlace,
    flags: u32,
) -> VariantUniform {
    VariantUniform {
        cell,
        origin,
        // The caller's, read as one colour (`variant_color`, #315)
        tint: [0.0; 3],
        color_mode: color_mode(&option("color")),
        brightness: param("brightness"),
        contrast: param("contrast").max(0.0),
        background: param("background").clamp(0.0, 1.0),
        ink: param("ink").max(0.0),
        levels: param("levels").max(0.0),
        footprint: param("footprint").clamp(1.0, 8.0),
        ramp_start: ramp.0,
        ramp_len: ramp.1,
        flags,
        glow: param("glow").max(0.0),
        ground: ground_mode(&option("ground")),
        palette_start: palette.start,
        palette_bg: palette.background,
        palette_len: palette.len,
        palette_match: palette.matching,
        exposure: param("exposure").clamp(-8.0, 8.0),
        stretch: param("normalize").clamp(0.0, 1.0),
        // Given by `assign_bands`
        cell_row: 0,
        // In points here; the caller scales it to pixels (`outline_pixels`)
        outline: param("outline"),
        min_coverage: param("min_coverage").clamp(0.0, 1.0),
        palette_dither: param("palette_dither").clamp(0.0, 1.0),
        chroma: param("chroma").clamp(0.0, 8.0),
        // The caller's, from the variant's font (#335)
        glyph: [0.0; 2],
        edge_start: NO_EDGE_START,
        cell_share: param("cell_share").clamp(1.0 / 16.0, 1.0),
        dissolve_row: 0,
        corners: 0,
        _pad_dissolve: [0; 2],
        // The caller's (#507)
        bounds: [0.0; 4],
        owner: [0; 4],
    }
}

/// How many corner glyphs the edge cells may draw in `set`'s font (#348):
/// its own count, or none with `edge_corner = 0`, which turns every corner
/// off, the coherence test's and the line ends' and gaps' too (#637).
pub(super) fn corner_glyphs(frame: &StyleFrame<'_>, set: &GlyphSet) -> u32 {
    if frame.param("edge_corner") <= 0.0 {
        return 0;
    }
    set.edges().corner_count() as u32
}

/// The silhouette outline (#288), `points` wide, in output pixels: at
/// least one whole pixel when on, so a 1 pt line never vanishes between
/// samples on a fractional display.
pub(super) fn outline_pixels(points: f32, pixels_per_point: f32) -> f32 {
    if points.is_nan() || points <= 0.0 {
        return 0.0;
    }
    let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    (points * ppp).clamp(1.0, 4.0)
}

/// Where one palette sits in the uniform's packed inks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct PalettePlace {
    pub(super) start: u32,
    pub(super) len: u32,
    pub(super) matching: u32,
    pub(super) background: [f32; 3],
}

/// The palettes in use, packed one after another into the uniform.
#[derive(Default)]
pub(super) struct Palettes {
    pub(super) inks: Vec<[f32; 4]>,
    /// The inks in OKLab, beside `inks` (#320).
    pub(super) lab: Vec<[f32; 4]>,
    pub(super) placed: Vec<(String, PalettePlace)>,
}

impl Palettes {
    /// Where `palette` sits, packing it on first use; `None` when its inks
    /// don't fit in what's left.
    pub(super) fn place(&mut self, palette: &Palette) -> Option<PalettePlace> {
        if let Some((_, at)) = self.placed.iter().find(|(n, _)| n == palette.name()) {
            return Some(*at);
        }
        let colors = palette.colors();
        if colors.is_empty() || self.inks.len() + colors.len() > MAX_COLORS {
            return None;
        }
        let at = PalettePlace {
            start: self.inks.len() as u32,
            len: colors.len() as u32,
            matching: palette.matching().index(),
            background: palette.background(),
        };
        self.inks
            .extend(colors.iter().map(|c| [c[0], c[1], c[2], 1.0]));
        self.lab.resize(at.start as usize, [0.0; 4]);
        self.lab
            .extend(palette.lab_colors().iter().map(|c| [c[0], c[1], c[2], 0.0]));
        self.placed.push((palette.name().to_string(), at));
        Some(at)
    }
}

/// Where an anchored grid starts: a cell centred on `anchor`, folded into
/// the first cell so the shader's arithmetic stays small.
pub fn anchored_origin(anchor: [f32; 2], cell: [f32; 2]) -> [f32; 2] {
    [
        (anchor[0] - 0.5 * cell[0]).rem_euclid(cell[0]).round(),
        (anchor[1] - 0.5 * cell[1]).rem_euclid(cell[1]).round(),
    ]
}

/// The ramps in use, packed one after another into the uniform: each step
/// a (font, atlas index) pair, the font an index into the frame's
/// [`FontsInUse`] (#335), made an origin in the glyph stack at upload, and
/// beside it the step's `ramp_tone` entry (#281).
#[derive(Default)]
pub(super) struct Ramps {
    pub(super) steps: Vec<(u32, u32)>,
    pub(super) tones: Vec<f32>,
    pub(super) placed: Vec<((u32, String), (u32, u32))>,
}

/// The key a font's four edge glyphs are packed under in [`Ramps`]; no
/// ramp is named with a NUL.
pub(super) const EDGES_KEY: &str = "\0edges";

impl Ramps {
    /// Where ramp `name` of `set` (font `font`) sits, packing it on first
    /// use; `None` when it doesn't fit in what's left.
    pub(super) fn place(&mut self, font: u32, set: &GlyphSet, name: &str) -> Option<(u32, u32)> {
        let ramp = set.ramp(name).unwrap_or_else(|| set.default_ramp());
        self.pack(font, name, fit_ramp(ramp))
    }

    /// Ramp `name` of font `font` and the font's edge glyphs (#335), both
    /// or neither: (the ramp's start and length, the edges' start).
    pub(super) fn place_with_edges(
        &mut self,
        font: u32,
        set: &GlyphSet,
        name: &str,
    ) -> Option<((u32, u32), u32)> {
        let ramp = set.ramp(name).unwrap_or_else(|| set.default_ramp());
        let fit = fit_ramp(ramp);
        let atlas = set.atlas();
        let edges: Vec<(u32, f32)> = set
            .edges()
            .slots()
            .iter()
            .map(|c| (atlas.index_of(*c).unwrap_or(0), EQUAL_BANDS))
            .collect();
        let need = |key: &str, len: usize| {
            if self.find(font, key).is_some() {
                0
            } else {
                len
            }
        };
        if self.steps.len() + need(name, fit.len()) + need(EDGES_KEY, edges.len()) > MAX_RAMP {
            return None;
        }
        let ramp = self.pack(font, name, fit)?;
        let edges = self.pack(font, EDGES_KEY, edges)?;
        Some((ramp, edges.0))
    }

    pub(super) fn find(&self, font: u32, name: &str) -> Option<(u32, u32)> {
        self.placed
            .iter()
            .find(|((f, n), _)| *f == font && n == name)
            .map(|(_, at)| *at)
    }

    pub(super) fn pack(&mut self, font: u32, name: &str, steps: Vec<(u32, f32)>) -> Option<(u32, u32)> {
        if let Some(at) = self.find(font, name) {
            return Some(at);
        }
        if self.steps.len() + steps.len() > MAX_RAMP {
            return None;
        }
        let at = (self.steps.len() as u32, steps.len() as u32);
        // A test may pad `steps` alone; the tones line up from here
        self.tones.resize(self.steps.len(), EQUAL_BANDS);
        self.tones.extend(steps.iter().map(|(_, t)| *t));
        self.steps.extend(steps.into_iter().map(|(i, _)| (font, i)));
        self.placed.push(((font, name.to_string()), at));
        Some(at)
    }
}

/// Fonts one frame can draw in (#335), the whole frame's first; a variant
/// naming another past these is drawn in the whole frame's (logged once).
pub const MAX_FONTS: usize = 8;

/// The tallest glyph stack, inside the least texture size every backend
/// offers; a font that would pass it is drawn in the whole frame's.
pub(super) const MAX_STACK_TEXELS: u32 = 8192;

/// The glyph sets a frame draws in (#335): the whole frame's at 0, then
/// each per-entity variant's own in the order the variants name them.
pub(super) struct FontsInUse {
    pub(super) sets: Vec<Arc<GlyphSet>>,
}

impl FontsInUse {
    pub(super) fn new(base: Arc<GlyphSet>) -> Self {
        Self { sets: vec![base] }
    }

    /// `set`'s index, adding it on first use; `None` when the frame holds
    /// [`MAX_FONTS`] already or its atlas would make the stack too tall.
    pub(super) fn index(&mut self, set: &Arc<GlyphSet>) -> Option<u32> {
        if let Some(i) = self.sets.iter().position(|s| Arc::ptr_eq(s, set)) {
            return Some(i as u32);
        }
        let tall: u32 = self.sets.iter().map(|s| s.atlas().size()[1]).sum();
        let [w, h] = set.atlas().size();
        if self.sets.len() == MAX_FONTS || tall + h > MAX_STACK_TEXELS || w > MAX_STACK_TEXELS {
            return None;
        }
        self.sets.push(set.clone());
        Some(self.sets.len() as u32 - 1)
    }
}

/// A kept bind group of one cell state texture, keyed by the history's
/// generation and which of the two it is.
pub(super) fn cells_bind(
    frame: &StyleFrame<'_>,
    name: &'static str,
    layout: &wgpu::BindGroupLayout,
    texture: &StyleTexture,
    variant: u64,
) -> Arc<wgpu::BindGroup> {
    frame.bind(name, variant, || {
        frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
            label: Some(name),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&texture.view),
            }],
        })
    })
}
