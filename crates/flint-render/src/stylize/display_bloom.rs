//! Display bloom (#321): a bloom over the displayed picture, after the
//! styles that draw past the HDR bloom. ASCII's glyphs are drawn at the
//! display stage, so they can't feed the scene's bloom, and its own `glow`
//! is a halo inside each cell. This one spreads across cells: a bright
//! glyph lights its neighbours and a bright run of cells blooms as one.
//!
//! ```toml
//! [post_process.styles.display_bloom]
//! intensity = 1.5    # how much halo is added
//! threshold = 0.55   # display value (0-1) a pixel's brightest channel must pass
//! knee = 0.25        # softness of that threshold, as a share of it
//! levels = 5         # how far it reaches: each level doubles it (in points)
//! saturation = 1.0   # 1 keeps the glyph's hue, 0 glows white
//! ```
//!
//! It runs after ASCII and pencil by registration order (`order` moves
//! it). The chain is the HDR bloom's shape: a threshold into a half-size
//! mip, a 13-tap downsample per level, a tent upsample added back up, and
//! a composite that adds mip 0 over the input. Its mips are the first mip
//! chain a display style keeps, each a [`StyleFrame::target_sized`] in
//! `Rgba16Float` (linear light, whatever the surface is). `levels` is in
//! points: on a 2x display it takes one more level, so the halo keeps its
//! size against the glyph cells beside it.

use super::kit;
use super::{
    StyleDescriptor, StyleFrame, StyleHint, StyleInput, StyleParam, StylePass, StyleStage,
};
use bytemuck::{Pod, Zeroable};

/// The most levels the chain draws.
pub const MAX_LEVELS: usize = 8;

/// The mips' names in [`StyleFrame::target_sized`].
const MIP_NAMES: [&str; MAX_LEVELS] = [
    "display_bloom.mip0",
    "display_bloom.mip1",
    "display_bloom.mip2",
    "display_bloom.mip3",
    "display_bloom.mip4",
    "display_bloom.mip5",
    "display_bloom.mip6",
    "display_bloom.mip7",
];

const MIP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Bytes between uniform slots: the uniform offset alignment.
const SLOT_STRIDE: u64 = 256;
/// Threshold, a downsample and an upsample per further level, composite.
const SLOTS: usize = 2 * MAX_LEVELS;

pub(super) static STYLE: StyleDescriptor = StyleDescriptor {
    name: "display_bloom",
    label: "Display bloom",
    stage: StyleStage::Display,
    params: &[
        StyleParam {
            name: "intensity",
            default: 1.5,
            min: 0.0,
            max: 4.0,
        },
        StyleParam {
            name: "threshold",
            default: 0.55,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "knee",
            default: 0.25,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "levels",
            default: 5.0,
            min: 1.0,
            max: 7.0,
        },
        StyleParam {
            name: "saturation",
            default: 1.0,
            min: 0.0,
            max: 1.0,
        },
    ],
    options: &[],
    uses_mask: false,
    needs: &[],
    hints: &[
        StyleHint::new(
            "intensity",
            "Intensity",
            "How much halo is added over the picture",
        ),
        StyleHint::new(
            "threshold",
            "Threshold",
            "The display value (0-1) a pixel's brightest channel must pass to glow",
        ),
        StyleHint::new(
            "knee",
            "Knee",
            "How softly the threshold starts, as a share of it",
        ),
        StyleHint::new(
            "levels",
            "Reach",
            "Mip levels: each doubles how far the halo spreads (in points)",
        )
        .integer(),
        StyleHint::new(
            "saturation",
            "Saturation",
            "1 keeps the glowing pixel's hue, 0 glows white",
        ),
    ],
    component: None,
    create: |d| {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "Display Bloom Uniform BGL");
        let sampled_bgl = kit::sampled_layout(device, "Display Bloom Source BGL");
        let module = kit::shader(
            device,
            "Display Bloom Shader",
            include_str!("display_bloom.wgsl"),
        );
        let chain_layouts = [&uniform_bgl, &sampled_bgl];
        let pipeline = |label: &str, entry: &str, format, blend, layouts: &[_]| {
            kit::fullscreen_pipeline_with(device, label, &module, entry, layouts, format, blend)
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        Box::new(DisplayBloom {
            threshold: pipeline(
                "Display Bloom Threshold",
                "fs_threshold",
                MIP_FORMAT,
                None,
                &chain_layouts,
            ),
            down: pipeline(
                "Display Bloom Down",
                "fs_down",
                MIP_FORMAT,
                None,
                &chain_layouts,
            ),
            up: pipeline(
                "Display Bloom Up",
                "fs_up",
                MIP_FORMAT,
                Some(additive),
                &chain_layouts,
            ),
            composite: pipeline(
                "Display Bloom Composite",
                "fs_composite",
                d.format,
                None,
                &[&uniform_bgl, &sampled_bgl, &sampled_bgl],
            ),
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Display Bloom Uniforms"),
                size: SLOTS as u64 * SLOT_STRIDE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            uniform_bgl,
            sampled_bgl,
        })
    },
};

#[repr(C)]
#[derive(Copy, Clone, Default, Pod, Zeroable)]
struct BloomUniforms {
    /// The step's source texel size
    texel: [f32; 2],
    threshold: f32,
    knee: f32,
    intensity: f32,
    /// 1 when the target is sRGB: threshold on display values
    srgb: f32,
    saturation: f32,
    _pad: f32,
}

struct DisplayBloom {
    threshold: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    sampled_bgl: wgpu::BindGroupLayout,
}

/// Mip `i`'s size for an output of `width` x `height`: half the output at
/// mip 0, halving each level after.
pub fn mip_size(width: u32, height: u32, i: usize) -> (u32, u32) {
    ((width >> (i + 1)).max(1), (height >> (i + 1)).max(1))
}

/// How many levels to draw: the `levels` asked for, one more per doubling
/// of the display's scale (so the reach is in points), and no more than
/// fit while the smallest mip keeps two texels a side.
pub fn level_count(levels: f32, pixels_per_point: f32, width: u32, height: u32) -> usize {
    let dpi = pixels_per_point.max(1.0).log2().round();
    let wanted = (levels.round() + dpi).clamp(1.0, MAX_LEVELS as f32) as usize;
    let mut fit = 1;
    while fit < wanted {
        let (w, h) = mip_size(width, height, fit);
        if w.min(h) < 2 {
            break;
        }
        fit += 1;
    }
    fit
}

impl StylePass for DisplayBloom {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let (width, height) = (frame.width, frame.height);
        let levels = level_count(frame.param("levels"), frame.pixels_per_point, width, height);
        let mips: Vec<_> = (0..levels)
            .map(|i| {
                let (w, h) = mip_size(width, height, i);
                frame.target_sized(MIP_NAMES[i], MIP_FORMAT, w, h)
            })
            .collect();
        let texel = |w: u32, h: u32| [1.0 / w as f32, 1.0 / h as f32];

        // Slots: 0 threshold, 1..levels downsamples (slot i reads mip i-1),
        // levels..2*levels-1 upsamples (levels + i reads mip i+1), and the
        // composite last. One write: every step's values land before the
        // frame's one submission (#139), so each step has a slot of its own
        let base = BloomUniforms {
            threshold: frame.param("threshold").clamp(0.0, 1.0),
            knee: frame.param("knee").clamp(0.0, 1.0),
            // The upsample sums the levels: divide them out, so the reach
            // doesn't change how bright the halo is
            intensity: frame.param("intensity").max(0.0) / levels as f32,
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
            saturation: frame.param("saturation").clamp(0.0, 1.0),
            ..Default::default()
        };
        let composite_slot = SLOTS - 1;
        let mut slots = vec![0u8; SLOTS * SLOT_STRIDE as usize];
        let mut put = |slot: usize, texel: [f32; 2]| {
            let at = slot * SLOT_STRIDE as usize;
            let u = BloomUniforms { texel, ..base };
            slots[at..at + std::mem::size_of::<BloomUniforms>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
        };
        put(0, texel(width, height));
        for i in 1..levels {
            put(i, texel(mips[i - 1].width, mips[i - 1].height));
        }
        for i in 0..levels.saturating_sub(1) {
            put(levels + i, texel(mips[i + 1].width, mips[i + 1].height));
        }
        put(composite_slot, texel(width, height));
        frame.queue.write_buffer(&self.uniforms, 0, &slots);

        let uniform = |slot: usize| {
            frame.bind("display_bloom.uniform", slot as u64, || {
                frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Display Bloom Uniform BG"),
                    layout: &self.uniform_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.uniforms,
                            offset: slot as u64 * SLOT_STRIDE,
                            size: wgpu::BufferSize::new(
                                std::mem::size_of::<BloomUniforms>() as u64
                            ),
                        }),
                    }],
                })
            })
        };
        // A mip's bind group. Kept with the chain: the mips' sizes follow
        // the output's alone, and a resize drops the chain's binds with
        // its targets
        let mip = |i: usize| {
            frame.bind("display_bloom.mip", i as u64, || {
                frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Display Bloom Mip BG"),
                    layout: &self.sampled_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&mips[i].view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(frame.linear_sampler),
                        },
                    ],
                })
            })
        };
        let source = frame.sampled_input("display_bloom.input", &self.sampled_bgl, input);
        let pass = |encoder: &mut wgpu::CommandEncoder,
                    label: &str,
                    target: &wgpu::TextureView,
                    clear: bool,
                    pipeline: &wgpu::RenderPipeline,
                    groups: &[&wgpu::BindGroup]| {
            crate::postprocess::fullscreen_pass(
                encoder,
                label,
                target,
                clear.then_some(wgpu::Color::BLACK),
                pipeline,
                groups,
            );
        };

        pass(
            encoder,
            "Display Bloom Threshold",
            &mips[0].view,
            true,
            &self.threshold,
            &[&uniform(0), &source],
        );
        for (i, target) in mips.iter().enumerate().skip(1) {
            pass(
                encoder,
                "Display Bloom Down",
                &target.view,
                true,
                &self.down,
                &[&uniform(i), &mip(i - 1)],
            );
        }
        for i in (0..levels - 1).rev() {
            pass(
                encoder,
                "Display Bloom Up",
                &mips[i].view,
                false,
                &self.up,
                &[&uniform(levels + i), &mip(i + 1)],
            );
        }
        pass(
            encoder,
            "Display Bloom Composite",
            output,
            true,
            &self.composite,
            &[&uniform(composite_slot), &source, &mip(0)],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_in_points_and_fit_the_output() {
        assert_eq!(level_count(5.0, 1.0, 1920, 1080), 5);
        // One more on a 2x display, two more at 4x
        assert_eq!(level_count(5.0, 2.0, 3840, 2160), 6);
        assert_eq!(level_count(5.0, 4.0, 7680, 4320), 7);
        // A tiny output takes what fits: 16 px tall halves to 8, 4, 2
        assert_eq!(level_count(7.0, 1.0, 64, 16), 3);
        assert_eq!(level_count(0.0, 1.0, 640, 360), 1);
        assert!(level_count(99.0, 8.0, 1 << 14, 1 << 14) <= MAX_LEVELS);
    }

    #[test]
    fn mips_halve_from_half_size() {
        assert_eq!(mip_size(1920, 1080, 0), (960, 540));
        assert_eq!(mip_size(1920, 1080, 3), (120, 67));
        assert_eq!(mip_size(3, 3, 4), (1, 1));
    }

    #[test]
    fn uniforms_fit_a_slot() {
        assert!(std::mem::size_of::<BloomUniforms>() as u64 <= SLOT_STRIDE);
        // Threshold, a down and an up per further level, composite
        assert!(1 + 2 * (MAX_LEVELS - 1) < SLOTS);
    }
}
