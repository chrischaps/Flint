//! ASCII (#265): the full-screen, grid-aligned glyph style. Every cell of
//! one screen grid becomes a glyph from a [`GlyphSet`] (#264), picked by
//! the cell's luminance, inked in the cell's colour or a tint.
//!
//! ```toml
//! [post_process.styles.ascii]
//! cell = 8              # cell width in points; the height follows the font
//! font = "builtin"      # or a fonts/<name>.glyphs.toml set
//! ramp = "classic"      # a ramp of that set
//! color = "source"      # source | tint | raw | mono | palette
//! palette = "phosphor"  # for color = "palette": built-in or palettes/<name>.palette.toml
//! ground = "dim"        # dim | black | inverse
//! glow = 0.0            # halo around bright glyphs
//! edges = 1.0           # line glyphs on edges: 0 off .. 1 full (#267)
//! edge_threshold = 0.5  # cell widths of edge a cell needs to draw one
//! edge_max = 3.0        # more than this is texture (grass), not a line
//! edge_depth = 0.6      # relative view-distance step that is an edge
//! edge_normal = 2.5     # normal change (Sobel) that is a crease
//! ```
//!
//! It runs at the display stage: on the finished picture at output
//! resolution, after the render-scale upscale, so glyphs stay crisp at any
//! scale (the ASCII spike's finding 7, #261). The cell is in points, times
//! the host's [`StyleFrame::pixels_per_point`], so it is the same size to
//! the eye on a high-DPI screen. The grid is centred on the frame.
//!
//! Colour modes (#276): `source` (the cell's hue), `tint` (one colour),
//! `raw` (the cell's colour as it is), `mono` (white on grey) and `palette`
//! (a [`Palette`]: nearest ink, as a CGA screen, or a tone ladder, as a
//! phosphor tube).
//!
//! Edges (#267): luminance alone loses silhouettes, so cells along the
//! scene's edges draw the glyph set's line glyphs (`─ │ ╱ ╲` in the
//! built-in set) instead of a tone glyph. Three passes: `ascii_edges.wgsl`
//! finds edge pixels from geometry (depth, normals rebuilt from it, and
//! the style mask, where an ascii-styled entity's outline is always an
//! edge), `ascii_cells.wgsl` reduces each glyph cell to one line glyph by
//! the summed structure tensor of every edge pixel in it, and the glyph
//! pass draws it. Away from edges cells keep their luminance glyphs.
//!
//! Per-entity selection (#266) builds on it.

use super::kit;
use super::{
    StyleDescriptor, StyleFrame, StyleInput, StyleOption, StyleParam, StylePass, StyleStage,
};
use crate::glyph_atlas::{GlyphSet, GlyphSetCache, GpuGlyphAtlas};
use crate::palette::{Palette, PaletteCache, BUILTIN_PALETTES, MAX_COLORS};
use bytemuck::{Pod, Zeroable};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Ramp steps the uniform holds; a longer ramp is resampled to this many.
pub const MAX_RAMP: usize = 128;

/// The colour modes, by their option spelling.
pub const COLOR_MODES: &[&str] = &["source", "tint", "raw", "mono", "palette"];

/// The ground (background fill) modes, by their option spelling.
pub const GROUND_MODES: &[&str] = &["dim", "black", "inverse"];

/// The palette `color = "palette"` uses when the scene names none.
pub const DEFAULT_PALETTE: &str = "phosphor";

pub(super) static STYLE: StyleDescriptor = StyleDescriptor {
    name: "ascii",
    label: "ASCII",
    stage: StyleStage::Display,
    params: &[
        StyleParam {
            name: "cell",
            default: 8.0,
            min: 4.0,
            max: 32.0,
        },
        StyleParam {
            name: "brightness",
            default: 0.0,
            min: -1.0,
            max: 1.0,
        },
        StyleParam {
            name: "contrast",
            default: 1.0,
            min: 0.0,
            max: 4.0,
        },
        StyleParam {
            name: "background",
            default: 0.15,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "ink",
            default: 1.0,
            min: 0.0,
            max: 2.0,
        },
        StyleParam {
            name: "levels",
            default: 0.0,
            min: 0.0,
            max: 16.0,
        },
        StyleParam {
            name: "footprint",
            default: 1.0,
            min: 1.0,
            max: 4.0,
        },
        StyleParam {
            name: "edges",
            default: 1.0,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "edge_threshold",
            default: 0.5,
            min: 0.05,
            max: 2.0,
        },
        StyleParam {
            name: "edge_max",
            default: 3.0,
            min: 0.5,
            max: 8.0,
        },
        StyleParam {
            name: "edge_depth",
            default: 0.6,
            min: 0.05,
            max: 4.0,
        },
        StyleParam {
            name: "edge_normal",
            default: 2.5,
            min: 0.2,
            max: 8.0,
        },
        StyleParam {
            name: "tint_r",
            default: 0.35,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "tint_g",
            default: 1.0,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "tint_b",
            default: 0.45,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "glow",
            default: 0.0,
            min: 0.0,
            max: 2.0,
        },
    ],
    options: &[
        StyleOption {
            name: "font",
            default: "builtin",
            choices: &[],
        },
        StyleOption {
            name: "ramp",
            default: "",
            choices: &["classic", "blocks", "minimal", "detailed"],
        },
        StyleOption {
            name: "color",
            default: "source",
            choices: COLOR_MODES,
        },
        StyleOption {
            name: "palette",
            default: DEFAULT_PALETTE,
            choices: BUILTIN_PALETTES,
        },
        StyleOption {
            name: "ground",
            default: "dim",
            choices: GROUND_MODES,
        },
    ],
    enabled: None,
    // The mask gives styled entities their outlines (#267); with no entity
    // asking for a style it is one cleared target and an empty pass
    uses_mask: true,
    create: |d| {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "ASCII Uniform BGL");
        let input_bgl = kit::sampled_layout(device, "ASCII Input BGL");
        let atlas_bgl = load_layout(device, "ASCII Atlas BGL");
        let cells_bgl = load_layout(device, "ASCII Cells BGL");
        let edges_in_bgl = load_layout(device, "ASCII Edges In BGL");
        let depth_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Depth BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let mask_bgl = kit::mask_layout(device, "ASCII Mask BGL");
        // Bound in place of the mask if a frame comes without one
        let no_mask = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ASCII No Mask"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::style_mask::STYLE_MASK_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let no_mask_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ASCII No Mask BG"),
            layout: &mask_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &no_mask.create_view(&Default::default()),
                ),
            }],
        });
        Box::new(Ascii {
            pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Pipeline",
                &kit::shader(device, "ASCII Shader", include_str!("ascii.wgsl")),
                &[&uniform_bgl, &input_bgl, &atlas_bgl, &cells_bgl],
                d.format,
            ),
            edge_pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Edge Pipeline",
                &kit::shader(
                    device,
                    "ASCII Edge Shader",
                    include_str!("ascii_edges.wgsl"),
                ),
                &[&uniform_bgl, &depth_bgl, &mask_bgl],
                EDGE_FORMAT,
            ),
            cell_pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Cell Pipeline",
                &kit::shader(
                    device,
                    "ASCII Cell Shader",
                    include_str!("ascii_cells.wgsl"),
                ),
                &[&uniform_bgl, &edges_in_bgl],
                CELL_FORMAT,
            ),
            uniforms: kit::uniform_buffer::<AsciiUniforms>(device, "ASCII Uniforms"),
            edge_uniforms: kit::uniform_buffer::<EdgeUniforms>(device, "ASCII Edge Uniforms"),
            cell_uniforms: kit::uniform_buffer::<CellUniforms>(device, "ASCII Cell Uniforms"),
            uniform_bgl,
            input_bgl,
            atlas_bgl,
            cells_bgl,
            edges_in_bgl,
            depth_bgl,
            mask_bgl,
            _no_mask: no_mask,
            no_mask_bind,
            fonts: Mutex::new(Fonts::default()),
            edges: Mutex::new(None),
            cells: Mutex::new(None),
        })
    },
};

/// Edge pixels: the unit gradient's structure tensor and the edge's kind.
const EDGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// One texel a glyph cell: its line glyph's slot + 1, edge length, outline.
const CELL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The style mask id meaning "no entity asks for ascii".
const NO_STYLE_ID: u32 = 0xFFFF;

/// A fragment-stage texture at binding 0 read with `textureLoad`.
fn load_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
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

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct AsciiUniforms {
    cell: [f32; 2],
    origin: [f32; 2],
    glyph: [f32; 2],
    size: [f32; 2],
    tint: [f32; 3],
    color_mode: u32,
    brightness: f32,
    contrast: f32,
    background: f32,
    ink: f32,
    levels: f32,
    footprint: f32,
    ramp_len: u32,
    srgb: u32,
    glow: f32,
    ground: u32,
    palette_len: u32,
    palette_match: u32,
    palette_bg: [f32; 3],
    _pad: u32,
    /// Line glyphs' strength over the tone glyph.
    edges: f32,
    /// Whether the edge and cell passes ran this frame.
    edge_on: u32,
    _pad_edges: [u32; 2],
    /// The four edge glyphs' atlas origins, in `EdgeGlyphs::all()` order.
    edge_glyphs: [[u32; 4]; 2],
    ramp: [[u32; 4]; MAX_RAMP / 2],
    palette: [[f32; 4]; MAX_COLORS],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct EdgeUniforms {
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 3],
    style_id: u32,
    _pad: [f32; 2],
    depth_threshold: f32,
    normal_threshold: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CellUniforms {
    cell: [f32; 2],
    origin: [f32; 2],
    size: [f32; 2],
    threshold: f32,
    max_len: f32,
    edge_size: [f32; 2],
    _pad: [f32; 2],
}

/// The grid's size in cells for a frame of `size` pixels, plus one: the
/// cell pass's target is offset by one so the part cell left of or above
/// the origin is texel 0.
pub fn grid_cells(size: [u32; 2], cell: [f32; 2], origin: [f32; 2]) -> [u32; 2] {
    let n = |px: u32, c: f32, o: f32| {
        ((px.max(1) as f32 - 1.0 - o) / c.max(1.0)).floor().max(0.0) as u32 + 2
    };
    [
        n(size[0], cell[0], origin[0]),
        n(size[1], cell[1], origin[1]),
    ]
}

/// The cell, in output pixels, for a cell `points` wide at
/// `pixels_per_point`, with a font whose glyphs are `glyph` pixels: whole
/// pixels, at least 2 wide, the height keeping the glyph's aspect.
pub fn cell_pixels(points: f32, pixels_per_point: f32, glyph: [u32; 2]) -> [f32; 2] {
    let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    let w = (points.max(1.0) * ppp).round().max(2.0);
    let aspect = glyph[1].max(1) as f32 / glyph[0].max(1) as f32;
    [w, (w * aspect).round().max(2.0)]
}

/// Where the grid starts so that the part cells at the borders match: half
/// of what's left over on each axis.
pub fn grid_origin(size: [u32; 2], cell: [f32; 2]) -> [f32; 2] {
    let left = |n: u32, c: f32| ((n as f32) % c) * 0.5;
    [
        left(size[0], cell[0]).floor(),
        left(size[1], cell[1]).floor(),
    ]
}

/// The colour mode's shader number; an unknown name is source colour.
pub fn color_mode(name: &str) -> u32 {
    COLOR_MODES.iter().position(|m| *m == name).unwrap_or(0) as u32
}

/// The ground mode's shader number; an unknown name is `dim`.
pub fn ground_mode(name: &str) -> u32 {
    GROUND_MODES.iter().position(|m| *m == name).unwrap_or(0) as u32
}

/// Up to [`MAX_RAMP`] steps of `indices`, evenly resampled when longer.
fn fit_ramp(indices: &[u32]) -> Vec<u32> {
    if indices.len() <= MAX_RAMP {
        return indices.to_vec();
    }
    (0..MAX_RAMP)
        .map(|i| indices[i * (indices.len() - 1) / (MAX_RAMP - 1)])
        .collect()
}

/// The set in use and its atlas on the GPU.
struct LoadedFont {
    name: String,
    set: Arc<GlyphSet>,
    _gpu: GpuGlyphAtlas,
    bind: wgpu::BindGroup,
}

#[derive(Default)]
struct Fonts {
    search: Vec<PathBuf>,
    cache: GlyphSetCache,
    palettes: PaletteCache,
    loaded: Option<LoadedFont>,
    /// The last (font, ramp) a warning was given for, so it's said once.
    warned: Option<(String, String)>,
}

impl Fonts {
    /// Look in the scene's folder and its parent, as the player finds
    /// `fonts/`, then the working directory.
    fn search_for(scene_dir: Option<&Path>) -> Vec<PathBuf> {
        let mut search = Vec::new();
        if let Some(dir) = scene_dir {
            search.push(dir.to_path_buf());
            if let Some(parent) = dir.parent() {
                search.push(parent.to_path_buf());
            }
        }
        search.push(PathBuf::from("."));
        search
    }
}

/// A target of the style's own at a size the chain's targets don't come in
/// (the framework's are the frame's size): the edge pixels at the depth's
/// size, and the cells at the grid's. Kept while the size holds, with the
/// bind group the next pass reads it through.
struct Owned {
    size: [u32; 2],
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

impl Owned {
    /// `slot`'s target, remade at `size` when it's another size.
    fn ensure<'s>(
        slot: &'s mut Option<Owned>,
        device: &wgpu::Device,
        label: &'static str,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        layout: &wgpu::BindGroupLayout,
    ) -> &'s Owned {
        let size = [size[0].max(1), size[1].max(1)];
        if slot.as_ref().is_none_or(|o| o.size != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            *slot = Some(Owned {
                size,
                _texture: texture,
                view,
                bind,
            });
        }
        slot.as_ref().expect("made above")
    }
}

struct Ascii {
    pipeline: wgpu::RenderPipeline,
    edge_pipeline: wgpu::RenderPipeline,
    cell_pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    edge_uniforms: wgpu::Buffer,
    cell_uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    atlas_bgl: wgpu::BindGroupLayout,
    cells_bgl: wgpu::BindGroupLayout,
    edges_in_bgl: wgpu::BindGroupLayout,
    depth_bgl: wgpu::BindGroupLayout,
    mask_bgl: wgpu::BindGroupLayout,
    _no_mask: wgpu::Texture,
    no_mask_bind: wgpu::BindGroup,
    fonts: Mutex<Fonts>,
    edges: Mutex<Option<Owned>>,
    cells: Mutex<Option<Owned>>,
}

impl Ascii {
    /// The font the config names, uploaded; the built-in set when it can't
    /// be loaded (the cache has warned).
    fn font<'f>(&self, frame: &StyleFrame<'_>, fonts: &'f mut Fonts) -> &'f LoadedFont {
        let search = Fonts::search_for(frame.scene_dir);
        if fonts.search != search {
            fonts.cache = GlyphSetCache::new(search.clone());
            fonts.palettes = PaletteCache::new(search.clone());
            fonts.search = search;
            fonts.loaded = None;
        }
        let name = frame.option("font").to_string();
        let set = fonts
            .cache
            .get(&name)
            .unwrap_or_else(|_| GlyphSet::builtin());
        let stale = fonts
            .loaded
            .as_ref()
            .is_none_or(|l| l.name != name || !Arc::ptr_eq(&l.set, &set));
        if stale {
            let gpu = GpuGlyphAtlas::upload(frame.device, frame.queue, set.atlas(), "ASCII Atlas");
            let bind = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Atlas BG"),
                layout: &self.atlas_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gpu.view),
                }],
            });
            fonts.loaded = Some(LoadedFont {
                name,
                set,
                _gpu: gpu,
                bind,
            });
        }
        fonts.loaded.as_ref().expect("loaded above")
    }
}

impl StylePass for Ascii {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let mut fonts = self.fonts.lock().unwrap_or_else(|e| e.into_inner());
        let ramp_name = frame.option("ramp").to_string();
        let set = self.font(frame, &mut fonts).set.clone();
        let ramp = match set.ramp(&ramp_name) {
            Some(r) => r,
            None => {
                let key = (set.name().to_string(), ramp_name.clone());
                if !ramp_name.is_empty() && fonts.warned.as_ref() != Some(&key) {
                    tracing::warn!(
                        "ascii: glyph set `{}` has no ramp `{ramp_name}`; using `{}` ({})",
                        set.name(),
                        set.default_ramp().name(),
                        set.ramps().map(|r| r.name()).collect::<Vec<_>>().join(", ")
                    );
                    fonts.warned = Some(key);
                }
                set.default_ramp()
            }
        };
        let atlas = set.atlas();
        let glyph = atlas.cell_size();
        let steps = fit_ramp(ramp.atlas_indices());
        let mut ramp_origins = [[0u32; 4]; MAX_RAMP / 2];
        for (i, index) in steps.iter().enumerate() {
            let [x, y] = atlas.cell_origin(*index);
            let slot = &mut ramp_origins[i / 2];
            slot[(i % 2) * 2] = x;
            slot[(i % 2) * 2 + 1] = y;
        }
        let palette = if frame.option("color") == "palette" {
            let name = match frame.option("palette") {
                "" => DEFAULT_PALETTE,
                n => n,
            };
            fonts.palettes.get(name).ok()
        } else {
            None
        }
        .unwrap_or_else(|| Palette::builtin(DEFAULT_PALETTE).expect("built-in"));
        let cell = cell_pixels(frame.param("cell"), frame.pixels_per_point, glyph);
        let size = [frame.width, frame.height];
        let origin = grid_origin(size, cell);
        let mut edge_glyphs = [[0u32; 4]; 2];
        for (i, ch) in set.edges().all().iter().enumerate() {
            // A set's edge glyphs are always in its atlas (GlyphSet checks)
            let [x, y] = atlas
                .index_of(*ch)
                .map_or([0, 0], |index| atlas.cell_origin(index));
            edge_glyphs[i / 2][(i % 2) * 2] = x;
            edge_glyphs[i / 2][(i % 2) * 2 + 1] = y;
        }
        let edges = frame.param("edges").clamp(0.0, 1.0);
        let mut cells_slot = self.cells.lock().unwrap_or_else(|e| e.into_inner());
        let cells = Owned::ensure(
            &mut cells_slot,
            frame.device,
            "ASCII Cells",
            CELL_FORMAT,
            grid_cells(size, cell, origin),
            &self.cells_bgl,
        );
        if edges > 0.0 {
            self.encode_edges(frame, encoder, cells, cell, origin);
        }
        let uniforms = AsciiUniforms {
            cell,
            origin,
            glyph: [glyph[0] as f32, glyph[1] as f32],
            size: [frame.width as f32, frame.height as f32],
            tint: [
                frame.param("tint_r"),
                frame.param("tint_g"),
                frame.param("tint_b"),
            ],
            color_mode: color_mode(frame.option("color")),
            brightness: frame.param("brightness"),
            contrast: frame.param("contrast").max(0.0),
            background: frame.param("background").clamp(0.0, 1.0),
            ink: frame.param("ink").max(0.0),
            levels: frame.param("levels").max(0.0),
            footprint: frame.param("footprint").clamp(1.0, 8.0),
            ramp_len: steps.len() as u32,
            srgb: frame.srgb() as u32,
            glow: frame.param("glow").max(0.0),
            ground: ground_mode(frame.option("ground")),
            palette_len: palette.colors().len() as u32,
            palette_match: palette.matching().index(),
            palette_bg: palette.background(),
            _pad: 0,
            edges,
            edge_on: u32::from(edges > 0.0),
            _pad_edges: [0; 2],
            edge_glyphs,
            ramp: ramp_origins,
            palette: palette.uniform_colors(),
        };
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));
        let font = fonts.loaded.as_ref().expect("loaded by font()");
        kit::fullscreen_pass(
            encoder,
            "ASCII Pass",
            output,
            &self.pipeline,
            &[
                &frame.uniform("ASCII Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("ASCII Input BG", &self.input_bgl, input),
                &font.bind,
                &cells.bind,
            ],
        );
    }
}

impl Ascii {
    /// Edge pixels from depth, normals and the mask, then each cell's line
    /// glyph from them (#267).
    fn encode_edges(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        cells: &Owned,
        cell: [f32; 2],
        origin: [f32; 2],
    ) {
        let edge_uniforms = EdgeUniforms {
            inv_view_proj: frame.camera.inv_view_proj,
            camera_pos: frame.camera.position,
            style_id: frame.style_id(STYLE.name).map_or(NO_STYLE_ID, u32::from),
            _pad: [0.0; 2],
            depth_threshold: frame.param("edge_depth").max(1e-3),
            normal_threshold: frame.param("edge_normal").max(1e-3),
        };
        frame
            .queue
            .write_buffer(&self.edge_uniforms, 0, bytemuck::bytes_of(&edge_uniforms));
        // At the depth's size, so a line is one depth pixel wide
        let mut edges_slot = self.edges.lock().unwrap_or_else(|e| e.into_inner());
        let edges = Owned::ensure(
            &mut edges_slot,
            frame.device,
            "ASCII Edges",
            EDGE_FORMAT,
            frame.depth_size,
            &self.edges_in_bgl,
        );
        let depth = frame.depth_bind("ASCII Depth BG", 0, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Depth BG"),
                layout: &self.depth_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(frame.depth),
                }],
            })
        });
        let mask = frame.mask_bind("ASCII Mask BG", &self.mask_bgl);
        kit::fullscreen_pass(
            encoder,
            "ASCII Edge Pass",
            &edges.view,
            &self.edge_pipeline,
            &[
                &frame.uniform(
                    "ASCII Edge Uniform BG",
                    &self.uniform_bgl,
                    &self.edge_uniforms,
                ),
                &depth,
                mask.as_deref().unwrap_or(&self.no_mask_bind),
            ],
        );

        let cell_uniforms = CellUniforms {
            cell,
            origin,
            size: [frame.width as f32, frame.height as f32],
            threshold: frame.param("edge_threshold").max(0.01),
            max_len: frame.param("edge_max"),
            edge_size: [edges.size[0] as f32, edges.size[1] as f32],
            _pad: [0.0; 2],
        };
        frame
            .queue
            .write_buffer(&self.cell_uniforms, 0, bytemuck::bytes_of(&cell_uniforms));
        let edges_in = &edges.bind;
        kit::fullscreen_pass(
            encoder,
            "ASCII Cell Pass",
            &cells.view,
            &self.cell_pipeline,
            &[
                &frame.uniform(
                    "ASCII Cell Uniform BG",
                    &self.uniform_bgl,
                    &self.cell_uniforms,
                ),
                edges_in,
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_shader_layout() {
        // 128 bytes of scalars, 2 vec4<u32> of edge glyphs, 64 vec4<u32> of
        // ramp, then the palette's vec4<f32>s
        assert_eq!(
            std::mem::size_of::<AsciiUniforms>(),
            128 + 2 * 16 + 64 * 16 + MAX_COLORS * 16
        );
        assert_eq!(std::mem::offset_of!(AsciiUniforms, edges), 112);
        assert_eq!(std::mem::offset_of!(AsciiUniforms, edge_glyphs), 128);
        assert_eq!(std::mem::offset_of!(AsciiUniforms, ramp), 160);
        assert_eq!(std::mem::size_of::<EdgeUniforms>(), 96);
        assert_eq!(std::mem::size_of::<CellUniforms>(), 48);
    }

    #[test]
    fn the_cell_grid_covers_the_part_cells_at_both_borders() {
        // Whole cells, no origin: 160 cells and one spare for the offset
        assert_eq!(grid_cells([1280, 720], [8.0, 8.0], [0.0, 0.0]), [161, 91]);
        // 1283 leaves 3 px: origin 1, a part cell at each border
        let origin = grid_origin([1283, 725], [8.0, 8.0]);
        let [w, h] = grid_cells([1283, 725], [8.0, 8.0], origin);
        // The last pixel's cell, offset by one, is the last texel
        let last = |px: u32, o: f32| ((px as f32 - 1.0 - o) / 8.0).floor() as u32 + 1;
        assert_eq!(w - 1, last(1283, origin[0]));
        assert_eq!(h - 1, last(725, origin[1]));
        // The first pixel's cell is -1 when the origin isn't 0: texel 0
        assert_eq!(((0.0 - origin[0]) / 8.0).floor() as i32 + 1, 0);
    }

    #[test]
    fn edges_are_on_by_default_and_every_edge_param_is_in_range() {
        for name in [
            "edges",
            "edge_threshold",
            "edge_max",
            "edge_depth",
            "edge_normal",
        ] {
            let p = STYLE.params.iter().find(|p| p.name == name).unwrap();
            assert!(p.min <= p.default && p.default <= p.max, "{name}");
        }
        let edges = STYLE.params.iter().find(|p| p.name == "edges").unwrap();
        assert_eq!(edges.default, 1.0);
        assert!(STYLE.uses_mask, "outlines read the style mask");
    }

    #[test]
    fn cells_scale_with_pixels_per_point_and_keep_the_glyph_aspect() {
        assert_eq!(cell_pixels(8.0, 1.0, [8, 8]), [8.0, 8.0]);
        assert_eq!(cell_pixels(8.0, 2.0, [8, 8]), [16.0, 16.0]);
        assert_eq!(cell_pixels(8.0, 1.5, [8, 8]), [12.0, 12.0]);
        assert_eq!(cell_pixels(8.0, 1.0, [8, 16]), [8.0, 16.0]);
        // Never below 2 px, and a broken scale factor counts as 1
        assert_eq!(cell_pixels(0.5, 1.0, [8, 8]), [2.0, 2.0]);
        assert_eq!(cell_pixels(8.0, f32::NAN, [8, 8]), [8.0, 8.0]);
        assert_eq!(cell_pixels(8.0, 0.0, [8, 8]), [8.0, 8.0]);
    }

    #[test]
    fn the_grid_is_centred() {
        // 1280 / 8 is whole; 720 / 12 is whole; 1283 leaves 3: 1 on the left
        assert_eq!(grid_origin([1280, 720], [8.0, 8.0]), [0.0, 0.0]);
        assert_eq!(grid_origin([1283, 725], [8.0, 8.0]), [1.0, 2.0]);
        assert_eq!(grid_origin([1920, 1080], [12.0, 12.0]), [0.0, 0.0]);
    }

    #[test]
    fn colour_modes_parse_and_fall_back_to_source() {
        assert_eq!(color_mode("source"), 0);
        assert_eq!(color_mode("tint"), 1);
        assert_eq!(color_mode("raw"), 2);
        assert_eq!(color_mode("mono"), 3);
        assert_eq!(color_mode("palette"), 4);
        assert_eq!(color_mode("plaid"), 0);
        assert_eq!(ground_mode("dim"), 0);
        assert_eq!(ground_mode("black"), 1);
        assert_eq!(ground_mode("inverse"), 2);
        assert_eq!(ground_mode(""), 0);
    }

    #[test]
    fn every_offered_palette_is_built_in() {
        let palette = STYLE.options.iter().find(|o| o.name == "palette").unwrap();
        assert!(Palette::builtin(palette.default).is_some());
        for choice in palette.choices {
            assert!(Palette::builtin(choice).is_some(), "{choice}");
        }
    }

    #[test]
    fn long_ramps_are_resampled_end_to_end() {
        let short: Vec<u32> = (0..10).collect();
        assert_eq!(fit_ramp(&short), short);
        let long: Vec<u32> = (0..300).collect();
        let fit = fit_ramp(&long);
        assert_eq!(fit.len(), MAX_RAMP);
        assert_eq!(fit[0], 0);
        assert_eq!(*fit.last().unwrap(), 299);
        assert!(fit.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn every_builtin_ramp_fits_without_resampling() {
        let set = GlyphSet::builtin();
        for ramp in set.ramps() {
            assert!(ramp.len() <= MAX_RAMP, "{}", ramp.name());
        }
        for choice in STYLE
            .options
            .iter()
            .find(|o| o.name == "ramp")
            .unwrap()
            .choices
        {
            assert!(
                set.ramp(choice).is_some(),
                "built-in set has no ramp {choice}"
            );
        }
    }
}
