//! Style ownership (#384): each pixel is one style's.
//!
//! An entity that asks for a style (`style_mask { style = "pencil" }`, a
//! `pencil_style`) owns its pixels for that style. A style switched on for
//! the whole frame used to draw over them anyway, so a pencil cask in an
//! ASCII frame was turned into glyphs and then sketched from the glyphs.
//! Now, after every style that [`yields`](super::StyleDescriptor::yields)
//! (all but the debug views and the display glow) draws, the framework
//! puts the style's input back on the pixels another running style owns,
//! [`StyleFrame::foreign_ids`](super::StyleFrame::foreign_ids): the cask
//! reaches pencil as the scene drew it, whichever of the two runs first. A
//! style gets this with no code of its own.
//!
//! The cut is the mask's, pixel for pixel: a glyph cell that straddles the
//! cask's outline is cut along it, not snapped to whole cells (#338's
//! `silhouette = "cell"` is a per-entity ASCII setting; whole-frame cells
//! at a foreign edge are a follow-up). A foreign entity's `blend` (the
//! mask's `.g`) mixes: at 0.3 it is 30 % back to the input, so an entity
//! fading out of its style fades into the whole frame's.

use super::kit;
use super::variant_table::{storage_entry, GrowingStorage};
use super::{StyleFrame, StyleInput};
use std::sync::Mutex;

const OWNERS_BG: &str = "Style Ownership Owners BG";

/// No style owns this mask id.
pub const NONE: u32 = u32::MAX;

pub(crate) struct OwnershipPass {
    module: kit::Shader,
    owners_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    mask_bgl: wgpu::BindGroupLayout,
    /// Each mask id's owner, as long as the frame's ids (#740).
    owners: Mutex<GrowingStorage>,
    /// One pipeline per target format (the HDR stage's and the display
    /// stage's), made on first use.
    pipelines: Mutex<Vec<(wgpu::TextureFormat, std::sync::Arc<wgpu::RenderPipeline>)>>,
    /// The owner table last written, so a frame writes it once for both
    /// stages and a still scene not at all.
    written: Mutex<Option<Vec<u32>>>,
}

impl OwnershipPass {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            module: kit::shader(
                device,
                "Style Ownership Shader",
                kit::wgsl!("ownership.wgsl"),
            ),
            owners_bgl: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Style Ownership Owners BGL"),
                entries: &[storage_entry(0)],
            }),
            input_bgl: kit::sampled_layout(device, "Style Ownership Input BGL"),
            mask_bgl: kit::mask_layout(device, "Style Ownership Mask BGL"),
            owners: Mutex::new(GrowingStorage::new(device, "Style Ownership Owners", 4)),
            pipelines: Mutex::new(Vec::new()),
            written: Mutex::new(None),
        }
    }

    fn pipeline(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> std::sync::Arc<wgpu::RenderPipeline> {
        let mut pipelines = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, p)) = pipelines.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        // The input over what the style drew by the mask's blend; the
        // target's alpha kept
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let p: std::sync::Arc<wgpu::RenderPipeline> =
            crate::pipeline_cache::in_scope(("Style Ownership", format), || {
                kit::fullscreen_pipeline_with(
                    device,
                    "Style Ownership Pipeline",
                    &self.module,
                    "fs_main",
                    &[&self.owners_bgl, &self.input_bgl, &self.mask_bgl],
                    format,
                    Some(blend),
                )
            });
        pipelines.push((format, p.clone()));
        p
    }

    /// After the style `frame.descriptor` drew `input` into `output`, put
    /// `input` back on the pixels another style owns. Draws nothing (and
    /// returns false) when the style doesn't yield, there's no mask, or no
    /// pixel is another style's.
    pub(crate) fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) -> bool {
        let Some(table) = frame.owner_table() else {
            return false;
        };
        let Some(me) = super::registry_index(frame.descriptor) else {
            return false;
        };
        let Some(mask) = frame.mask_bind("Style Ownership Mask BG", &self.mask_bgl) else {
            return false;
        };
        let uniform = {
            let mut owners = self.owners.lock().unwrap_or_else(|e| e.into_inner());
            let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
            if written.as_deref() != Some(&table[..]) {
                owners.write_words(frame.device, frame.queue, &table, NONE);
                *written = Some(table);
            }
            let buffer = owners.buffer();
            frame.bind(OWNERS_BG, owners.generation(), || {
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
        let pipeline = self.pipeline(frame.device, frame.format);
        let input_bg = frame.sampled_input("Style Ownership Input BG", &self.input_bgl, input);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Style Ownership Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &*uniform, &[]);
        pass.set_bind_group(1, &*input_bg, &[]);
        pass.set_bind_group(2, &*mask, &[]);
        // The style that just drew rides in the instance index, so one
        // owner table serves every style of the frame
        pass.draw(0..3, me as u32..me as u32 + 1);
        true
    }
}
