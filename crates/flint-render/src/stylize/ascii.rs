//! ASCII (#265): the full-screen, grid-aligned glyph style. Every cell of
//! one screen grid becomes a glyph from a [`GlyphSet`] (#264), picked by
//! the cell's luminance, inked in the cell's colour or a tint.
//!
//! ```toml
//! [post_process.styles.ascii]
//! cell = 8              # cell width in points; the height follows the font
//! font = "builtin"      # or a fonts/<name>.glyphs.toml set
//! ramp = "classic"      # a ramp of that set
//! color = "source"      # source | tint | raw
//! ```
//!
//! It runs at the display stage: on the finished picture at output
//! resolution, after the render-scale upscale, so glyphs stay crisp at any
//! scale (the ASCII spike's finding 7, #261). The cell is in points, times
//! the host's [`StyleFrame::pixels_per_point`], so it is the same size to
//! the eye on a high-DPI screen. The grid is centred on the frame.
//!
//! Edge glyphs (#267), per-entity selection (#266) and more colour modes
//! (#276) build on it.

use super::kit;
use super::{
    StyleDescriptor, StyleFrame, StyleInput, StyleOption, StyleParam, StylePass, StyleStage,
};
use crate::glyph_atlas::{GlyphSet, GlyphSetCache, GpuGlyphAtlas};
use bytemuck::{Pod, Zeroable};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Ramp steps the uniform holds; a longer ramp is resampled to this many.
pub const MAX_RAMP: usize = 128;

/// The colour modes, by their option spelling.
pub const COLOR_MODES: &[&str] = &["source", "tint", "raw"];

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
    ],
    enabled: None,
    create: |d| {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "ASCII Uniform BGL");
        let input_bgl = kit::sampled_layout(device, "ASCII Input BGL");
        let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Atlas BGL"),
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
        });
        Box::new(Ascii {
            pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Pipeline",
                &kit::shader(device, "ASCII Shader", include_str!("ascii.wgsl")),
                &[&uniform_bgl, &input_bgl, &atlas_bgl],
                d.format,
            ),
            uniforms: kit::uniform_buffer::<AsciiUniforms>(device, "ASCII Uniforms"),
            uniform_bgl,
            input_bgl,
            atlas_bgl,
            fonts: Mutex::new(Fonts::default()),
        })
    },
};

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
    ramp: [[u32; 4]; MAX_RAMP / 2],
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

struct Ascii {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    atlas_bgl: wgpu::BindGroupLayout,
    fonts: Mutex<Fonts>,
}

impl Ascii {
    /// The font the config names, uploaded; the built-in set when it can't
    /// be loaded (the cache has warned).
    fn font<'f>(&self, frame: &StyleFrame<'_>, fonts: &'f mut Fonts) -> &'f LoadedFont {
        let search = Fonts::search_for(frame.scene_dir);
        if fonts.search != search {
            fonts.cache = GlyphSetCache::new(search.clone());
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
        let cell = cell_pixels(frame.param("cell"), frame.pixels_per_point, glyph);
        let uniforms = AsciiUniforms {
            cell,
            origin: grid_origin([frame.width, frame.height], cell),
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
            ramp: ramp_origins,
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
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_shader_layout() {
        // 80 bytes of scalars, then 64 vec4<u32>
        assert_eq!(std::mem::size_of::<AsciiUniforms>(), 80 + 64 * 16);
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
        assert_eq!(color_mode("plaid"), 0);
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
