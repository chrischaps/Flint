//! The framework's cross-fade (#396): every mask-reading style gets its
//! entities' `blend` for free.
//!
//! An entity's `blend` (#278) rides in the style mask's `.g`, along with
//! the coverage of anything unstyled in front of it (the veil, #290). A
//! style that reads the mask and doesn't
//! [`blend itself`](super::StyleDescriptor::blends_itself) has this pass
//! drawn over its output right after its own: one full-screen triangle,
//! alpha-blended into the output, that brings the style's input back by
//! `1 - blend` on the style's own entities' pixels and is discarded
//! everywhere else. The chain's ping-pong targets already hold both, so it
//! needs no target of its own, and it is drawn only while some mask pixel
//! may be below 1 ([`StyleFrame::mask_fades`](super::StyleFrame::mask_fades)).
//!
//! Which style a mask id belongs to is a storage table, id -> registry
//! index + 1, written once a frame; which style just drew is the draw's
//! instance index, so one table serves every style in both stages.
//!
//! The mix is the blend unit's: in linear light on an sRGB target and in
//! the HDR stage, in display values on a plain one. ASCII mixes in display
//! values itself and opts out.

use super::{StyleDescriptor, StyleFrame, StyleInput};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What `--gpu-time` and the debugger call the pass.
pub(crate) const FADE_LABEL: &str = "Style cross-fade";

const OWNERS_BG: &str = "style fade owners BG";
const INPUT_BG: &str = "style fade input BG";
const MASK_BG: &str = "style fade mask BG";

/// The owner table's buffer, grown when a frame has more ids than it holds.
struct Owners {
    buffer: wgpu::Buffer,
    /// How many `u32`s it holds.
    capacity: usize,
    /// Bumped on each growth, keying the bind group over it.
    generation: u64,
}

pub(crate) struct FadePass {
    module: wgpu::ShaderModule,
    owners_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    mask_bgl: wgpu::BindGroupLayout,
    pipelines: Mutex<HashMap<wgpu::TextureFormat, Arc<wgpu::RenderPipeline>>>,
    owners: Mutex<Owners>,
    runs: AtomicU64,
}

impl FadePass {
    pub fn new(device: &wgpu::Device) -> Self {
        let owners_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Style Fade Owners BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        Self {
            module: super::kit::shader(device, "Style Fade Shader", include_str!("fade.wgsl")),
            input_bgl: super::kit::texture_layout(device, "Style Fade Input BGL"),
            mask_bgl: super::kit::mask_layout(device, "Style Fade Mask BGL"),
            owners_bgl,
            pipelines: Mutex::new(HashMap::new()),
            owners: Mutex::new(Owners {
                buffer: owners_buffer(device, 64),
                capacity: 64,
                generation: 0,
            }),
            runs: AtomicU64::new(0),
        }
    }

    /// How many times the cross-fade has been drawn.
    pub fn runs(&self) -> u64 {
        self.runs.load(Ordering::Relaxed)
    }

    /// Whether `descriptor`'s pass, just drawn, gets the cross-fade this
    /// frame: it reads the mask and doesn't blend itself, the mask is drawn
    /// and may hold a blend below 1, and some entity asks for the style.
    pub fn applies(frame: &StyleFrame<'_>, descriptor: &'static StyleDescriptor) -> bool {
        frame.mask_fades
            && frame.mask.is_some()
            && descriptor.framework_fades(frame.config, frame.style_ids)
            && frame.style_ids.requests(descriptor.name)
    }

    /// Draw the cross-fade for `descriptor` over `output`, which its pass
    /// just drew from `input`. Call only when [`applies`](Self::applies).
    pub fn encode(
        &self,
        frame: &StyleFrame<'_>,
        descriptor: &'static StyleDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let Some(mask) = frame.mask else {
            return;
        };
        let registry = super::registry();
        let Some(me) = registry.iter().position(|d| d.name == descriptor.name) else {
            return;
        };
        let me = me as u32 + 1;
        let table = owner_table(frame.style_ids, registry);
        let owners_bind = {
            let mut owners = self.owners.lock().unwrap_or_else(|e| e.into_inner());
            if owners.capacity < table.len() {
                let capacity = table.len().next_power_of_two();
                owners.buffer = owners_buffer(frame.device, capacity);
                owners.capacity = capacity;
                owners.generation += 1;
            }
            frame
                .queue
                .write_buffer(&owners.buffer, 0, bytemuck::cast_slice(&table));
            let buffer = &owners.buffer;
            frame.bind(OWNERS_BG, owners.generation, || {
                frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(OWNERS_BG),
                    layout: &self.owners_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                })
            })
        };
        let input_bind = frame.bind(INPUT_BG, input.id, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(INPUT_BG),
                layout: &self.input_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input.view),
                }],
            })
        });
        let mask_bind = frame.bind(MASK_BG, 0, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(MASK_BG),
                layout: &self.mask_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(mask),
                }],
            })
        });
        let pipeline = self.pipeline(frame.device, frame.format);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(FADE_LABEL),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &*owners_bind, &[]);
        pass.set_bind_group(1, &*input_bind, &[]);
        pass.set_bind_group(2, &*mask_bind, &[]);
        pass.draw(0..3, me..me + 1);
        self.runs.fetch_add(1, Ordering::Relaxed);
    }

    /// The pipeline drawing into `format`, made the first time.
    fn pipeline(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<wgpu::RenderPipeline> {
        let mut pipelines = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        pipelines
            .entry(format)
            .or_insert_with(|| {
                Arc::new(super::kit::fullscreen_pipeline_with(
                    device,
                    "Style Fade Pipeline",
                    &self.module,
                    "fs_main",
                    &[&self.owners_bgl, &self.input_bgl, &self.mask_bgl],
                    format,
                    Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        // The output's alpha stays the style's
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                ))
            })
            .clone()
    }
}

fn owners_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Style Fade Owners"),
        size: (capacity * std::mem::size_of::<u32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Each mask id's owner, indexed by id: the registry index + 1 of the
/// style its variant names, or 0 for no id, an opt-out (`enabled = false`
/// leaves its pixels alone) or a style that isn't registered.
pub(crate) fn owner_table(
    ids: &crate::style_mask::StyleIds,
    registry: &[&'static StyleDescriptor],
) -> Vec<u32> {
    let mut table = vec![0u32; ids.len() + 1];
    for (id, slot) in table.iter_mut().enumerate().skip(1) {
        let Some(v) = ids.variant(id as u16) else {
            continue;
        };
        if v.off {
            continue;
        }
        if let Some(i) = registry.iter().position(|d| d.name == v.style) {
            *slot = i as u32 + 1;
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style_mask::{StyleIds, StyleVariant};

    #[test]
    fn owners_name_the_style_each_id_draws_in() {
        let mut ids = StyleIds::default();
        let off = StyleVariant {
            off: true,
            ..StyleVariant::plain("pencil")
        };
        ids.set(vec![
            ("pencil".into(), StyleVariant::plain("pencil")),
            ("ascii".into(), StyleVariant::plain("ascii")),
            ("pencil{off}".into(), off),
            ("nonesuch".into(), StyleVariant::plain("nonesuch")),
        ]);
        let registry = super::super::registry();
        let index = |name: &str| registry.iter().position(|d| d.name == name).unwrap() as u32 + 1;
        let table = owner_table(&ids, registry);
        assert_eq!(table.len(), 5);
        assert_eq!(table[0], 0, "id 0 is unstyled");
        assert_eq!(table[ids.id("pencil").unwrap() as usize], index("pencil"));
        assert_eq!(table[ids.id("ascii").unwrap() as usize], index("ascii"));
        assert_eq!(table[ids.id("pencil{off}").unwrap() as usize], 0);
        assert_eq!(table[ids.id("nonesuch").unwrap() as usize], 0);
    }
}
