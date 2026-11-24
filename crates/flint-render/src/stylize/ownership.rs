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
//! The cut is the mask's, pixel for pixel, unless a display style draws
//! the whole frame in cells and hands the framework its grid
//! ([`StylePass::ownership_cells`](super::StylePass::ownership_cells),
//! #534: whole-frame ASCII under `silhouette = "cell"`). Then each cell of
//! that grid at the edge of another style's entity goes wholly to one
//! side, by the share of its 16 lattice samples the other style owns
//! (`fs_cells`). A cell the snapping style holds is drawn whole: the other
//! style's pixels in it are the whole frame's, so pencil gives them back to
//! the glyph. A cell it doesn't hold is the other style's whole: its scene
//! pixels show what came into the stage, the plain scene, since the other
//! style draws only its own pixels. Every style of the stage cuts along
//! the same cells, in either chain order. A foreign entity's `blend` (the
//! mask's `.g`) mixes: at 0.3 it is 30 % back to the input, so an entity
//! fading out of its style fades into the whole frame's; the scene pixels
//! of a snapped cell take the blend of its other style's first sample.

use super::kit;
use super::variant_table::{storage_entry, GrowingStorage};
use super::{SnappedOwnership, StyleFrame, StyleInput};
use crate::gpu_counts::{CountedDevice, CountedQueue};
use bytemuck::{Pod, Zeroable};
use std::sync::Mutex;

const OWNERS_BG: &str = "Style Ownership Owners BG";

/// No style owns this mask id.
pub const NONE: u32 = u32::MAX;

/// The snapped cut's grid (#534), `Cells` in ownership.wgsl.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
struct CellsUniform {
    /// cell (xy) and origin (zw), output pixels
    grid: [f32; 4],
    /// Where another style's ids can be, output pixels (x0, y0, x1, y1)
    rect: [f32; 4],
    /// x: the snapping style's registry index
    style: [u32; 4],
    /// x: the least of a cell's 16 samples the snapping style must hold
    held: [f32; 4],
}

impl CellsUniform {
    fn of(s: &SnappedOwnership) -> Self {
        let c = s.cells;
        Self {
            grid: [c.cell[0], c.cell[1], c.origin[0], c.origin[1]],
            rect: s.rect,
            style: [s.style, 0, 0, 0],
            held: [c.share * 16.0, 0.0, 0.0, 0.0],
        }
    }
}

/// What rides the instance index into `fs_cells`: the style that just
/// drew, and whether it draws the whole frame (the low bit).
fn cells_instance(style: u32, covers_frame: bool) -> u32 {
    style * 2 + u32::from(covers_frame)
}

pub(crate) struct OwnershipPass {
    module: kit::Shader,
    owners_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    mask_bgl: wgpu::BindGroupLayout,
    cells_bgl: wgpu::BindGroupLayout,
    /// The snapped cut's grid (#534) and its bind group (the buffer never
    /// changes).
    cells: wgpu::Buffer,
    cells_bg: wgpu::BindGroup,
    /// The grid last written. Only the display stage snaps, so a frame
    /// holds one grid.
    written_cells: Mutex<Option<CellsUniform>>,
    /// Each mask id's owner, as long as the frame's ids (#740).
    owners: Mutex<GrowingStorage>,
    /// One pipeline per target format (the HDR stage's and the display
    /// stage's) and cut (to the pixel, or snapped to cells), made on first
    /// use.
    #[allow(clippy::type_complexity)]
    pipelines: Mutex<
        Vec<(
            wgpu::TextureFormat,
            bool,
            std::sync::Arc<wgpu::RenderPipeline>,
        )>,
    >,
    /// The owner table last written, so a frame writes it once for both
    /// stages and a still scene not at all.
    written: Mutex<Option<Vec<u32>>>,
}

impl OwnershipPass {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let cells_bgl = kit::uniform_layout(device, "Style Ownership Cells BGL");
        let cells = kit::uniform_buffer::<CellsUniform>(device, "Style Ownership Cells");
        let cells_bg = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
            label: Some("Style Ownership Cells BG"),
            layout: &cells_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: cells.as_entire_binding(),
            }],
        });
        Self {
            module: kit::shader(
                device,
                "Style Ownership Shader",
                kit::wgsl!("mask_read.wgsl", "ownership.wgsl"),
            ),
            owners_bgl: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Style Ownership Owners BGL"),
                entries: &[storage_entry(0)],
            }),
            input_bgl: kit::sampled_layout(device, "Style Ownership Input BGL"),
            mask_bgl: kit::mask_layout(device, "Style Ownership Mask BGL"),
            cells_bgl,
            cells,
            cells_bg,
            written_cells: Mutex::new(None),
            owners: Mutex::new(GrowingStorage::new(device, "Style Ownership Owners", 4)),
            pipelines: Mutex::new(Vec::new()),
            written: Mutex::new(None),
        }
    }

    fn pipeline(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        snapped: bool,
    ) -> std::sync::Arc<wgpu::RenderPipeline> {
        let mut pipelines = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, _, p)) = pipelines
            .iter()
            .find(|(f, s, _)| *f == format && *s == snapped)
        {
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
        let p: std::sync::Arc<wgpu::RenderPipeline> = if snapped {
            crate::pipeline_cache::in_scope(("Style Ownership Cells", format), || {
                kit::fullscreen_pipeline_with(
                    device,
                    "Style Ownership Cells Pipeline",
                    &self.module,
                    "fs_cells",
                    &[
                        &self.owners_bgl,
                        &self.input_bgl,
                        &self.mask_bgl,
                        &self.cells_bgl,
                    ],
                    format,
                    Some(blend),
                )
            })
        } else {
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
            })
        };
        pipelines.push((format, snapped, p.clone()));
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
        let Some(me) = super::registry_index(frame.styles, frame.descriptor) else {
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
                frame
                    .device
                    .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                        label: Some(OWNERS_BG),
                        layout: &self.owners_bgl,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        }],
                    })
            })
        };
        let snapped = frame.snapped_ownership;
        if let Some(s) = &snapped {
            let cells = CellsUniform::of(s);
            let mut written = self.written_cells.lock().unwrap_or_else(|e| e.into_inner());
            if written.as_ref() != Some(&cells) {
                frame
                    .queue
                    .write_buffer_counted(&self.cells, 0, bytemuck::bytes_of(&cells));
                *written = Some(cells);
            }
        }
        let pipeline = self.pipeline(frame.device, frame.format, snapped.is_some());
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
        // owner table serves every style of the frame; snapped, with
        // whether it draws the whole frame
        let instance = if snapped.is_some() {
            pass.set_bind_group(3, &self.cells_bg, &[]);
            cells_instance(me as u32, frame.covers_frame())
        } else {
            me as u32
        };
        pass.draw(0..3, instance..instance + 1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cells_uniform_is_four_vec4s() {
        // `Cells` in ownership.wgsl
        assert_eq!(std::mem::size_of::<CellsUniform>(), 64);
    }

    #[test]
    fn the_instance_carries_the_style_and_whether_it_covers_the_frame() {
        assert_eq!(cells_instance(0, false), 0);
        assert_eq!(cells_instance(3, true), 7);
        assert_eq!(cells_instance(3, true) >> 1, 3);
        assert_eq!(cells_instance(3, false) & 1, 0);
    }
}
