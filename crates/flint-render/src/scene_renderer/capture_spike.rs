//! SPIKE (#1082, throwaway): read back every target the frame keeps, to
//! measure what a capture's images cost. Not for main.

use super::SceneRenderer;
use web_time::Instant;

/// One target read back, its bytes tightly packed.
pub struct SpikeTap {
    pub name: String,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// What the read-back cost, in ms of the calling thread.
#[derive(Debug, Default, Clone)]
pub struct SpikeTiming {
    pub targets: usize,
    pub bytes: u64,
    /// Making staging buffers and recording the copies.
    pub encode_ms: f64,
    /// `submit` returning.
    pub submit_ms: f64,
    /// `map_async` + `wait_for_gpu`: the GPU finishing the frame and the copies.
    pub wait_ms: f64,
    /// Copying mapped memory out, unpadding rows.
    pub unmap_ms: f64,
}

struct Pending {
    name: String,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    padded: u32,
    unpadded: u32,
    buffer: wgpu::Buffer,
}

impl SceneRenderer {
    /// Every target this renderer keeps that a capture would tap, plus the
    /// host's (`extra`: the output and its depth), read back in one
    /// submission. The frame's work must already be submitted.
    pub fn spike_tap_all(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        extra: &[(&str, &wgpu::Texture)],
    ) -> (Vec<SpikeTap>, SpikeTiming) {
        let t0 = Instant::now();
        let mut list: Vec<(String, wgpu::Texture, u32)> = Vec::new();
        for (name, t) in extra {
            list.push((name.to_string(), (*t).clone(), 0));
        }
        if let Some(pp) = &self.postprocess_resources {
            list.push(("hdr_scene".into(), pp.hdr_texture.clone(), 0));
            list.push(("ssao".into(), pp.ssao_texture.clone(), 0));
            list.push(("ssao_blur".into(), pp.ssao_blur_texture.clone(), 0));
            list.push(("volumetric".into(), pp.volumetric_texture.clone(), 0));
            list.push((
                "volumetric_blur".into(),
                pp.volumetric_blur_texture.clone(),
                0,
            ));
            for (i, mip) in pp.bloom_mips.iter().enumerate() {
                list.push((format!("bloom_{i}"), mip.texture.clone(), 0));
            }
            for (name, t) in pp.styles.spike_textures() {
                list.push((format!("style.{name}"), t, 0));
            }
            if let Some(fx) = &pp.fxaa {
                list.push(("fxaa".into(), fx.texture.clone(), 0));
            }
        }
        if let Some(sp) = &self.shadow_pass {
            if sp.enabled {
                for layer in 0..sp.shadow_texture.depth_or_array_layers() {
                    list.push((format!("shadow_{layer}"), sp.shadow_texture.clone(), layer));
                }
            }
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Spike Capture Encoder"),
        });
        let mut pending = Vec::new();
        for (name, texture, layer) in list {
            let format = texture.format();
            let (w, h) = (texture.width(), texture.height());
            let aspect = if format.is_depth_stencil_format() {
                wgpu::TextureAspect::DepthOnly
            } else {
                wgpu::TextureAspect::All
            };
            let Some(texel) = format.block_copy_size(Some(aspect)) else {
                continue;
            };
            if texture.sample_count() != 1 {
                continue;
            }
            let unpadded = w * texel;
            let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let padded = unpadded.div_ceil(align) * align;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Spike Tap"),
                size: u64::from(padded) * u64::from(h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            pending.push(Pending {
                name,
                format,
                width: w,
                height: h,
                padded,
                unpadded,
                buffer,
            });
        }
        let t1 = Instant::now();
        queue.submit(Some(encoder.finish()));
        let t2 = Instant::now();
        for p in &pending {
            p.buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        crate::wait_for_gpu(device);
        let t3 = Instant::now();
        let mut taps = Vec::with_capacity(pending.len());
        let mut total = 0u64;
        for p in pending {
            let data = p.buffer.slice(..).get_mapped_range().expect("mapped");
            let mut bytes = Vec::with_capacity((p.unpadded * p.height) as usize);
            for row in 0..p.height {
                let s = (row * p.padded) as usize;
                bytes.extend_from_slice(&data[s..s + p.unpadded as usize]);
            }
            drop(data);
            p.buffer.unmap();
            total += bytes.len() as u64;
            taps.push(SpikeTap {
                name: p.name,
                format: p.format,
                width: p.width,
                height: p.height,
                bytes,
            });
        }
        let t4 = Instant::now();
        let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
        let timing = SpikeTiming {
            targets: taps.len(),
            bytes: total,
            encode_ms: ms(t0, t1),
            submit_ms: ms(t1, t2),
            wait_ms: ms(t2, t3),
            unmap_ms: ms(t3, t4),
        };
        (taps, timing)
    }
}
