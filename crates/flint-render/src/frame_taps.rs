//! Each pass's output image (#1085, ADR 0126 decisions 2 and 5).
//!
//! A host asks with [`SceneRenderer::capture_next_frame`], which describes
//! the frame too (#1084). As the frame is recorded, each pass that wrote a
//! target has it copied, right after the pass, into a staging buffer made
//! for this frame: the copy goes into the frame's own encoder (#139's one
//! submission), and at the tap, not the frame's end, because the chain
//! reuses its targets (the style stage's ping-pong pair, bloom's mips, which
//! the upsample writes over). After the frame is submitted the host takes
//! the [`FrameCapture`], maps it, and polls ([`FrameCapture::poll`], never
//! waits) or, headless, waits ([`FrameCapture::wait`]); then
//! [`FrameCapture::read`] copies the texels out and the writer
//! (`frame_capture::write_capture`, native) stores them.
//!
//! **Costs only the frame that asked.** Nothing is made or copied on any
//! other frame: every tap site checks one `Option` first, and the staging
//! buffers are dropped with the capture.
//!
//! What is read: a multisampled target only through its resolve (the HDR
//! buffer), each shadow map layer that was drawn as its own image, each
//! bloom step, each style's output and every target of the style chain's
//! table ([`StyleTargets`](crate::stylize::StyleTargets)) the style asked for
//! while it drew (a user style's own targets included), each texture a style
//! holds itself and noted as it drew ([`HeldTargets`](crate::stylize::HeldTargets),
//! #1161), and the editor's selection mask. A target a pass
//! wrote that isn't read (a buffer, a multisampled texture) is named with
//! why in [`CaptureDescription::untapped`](crate::frame_capture::CaptureDescription::untapped).
//!
//! **What the copies cost on the GPU** (#1160): each copy is bracketed by
//! two timestamps of the capture's own query set (`CopyTimer`), not the
//! frame's [`GpuTimer`](crate::GpuTimer), whose 32 scopes a frame belong to
//! the passes; a capture makes 17-29 copies. Each pair is resolved in the
//! frame's encoder right after its copy, so no submission is added (#139),
//! and read back with the images. A device without `TIMESTAMP_QUERY`
//! copies untimed.
//!
//! [`SceneRenderer::capture_next_frame`]: crate::SceneRenderer::capture_next_frame

use crate::frame_capture::{ImageDescription, LoggedPass};
use crate::gpu_counts::CountedDevice;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use web_time::Instant;

/// The host's textures a capture reads, which the renderer draws into but
/// doesn't hold: its output (`surface`) and depth buffer (`depth`). Each
/// needs `COPY_SRC`; one left `None`, or without it, is named in the
/// capture's `untapped`.
#[derive(Default, Clone)]
pub struct CaptureTargets {
    pub surface: Option<wgpu::Texture>,
    pub depth: Option<wgpu::Texture>,
}

/// How a format's texels are stored (ADR 0126 decision 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Storage {
    /// 8-bit integers as PNG: 1, 2 (grey and alpha) or 4 channels, `bgra`
    /// swizzled to RGBA.
    Png8 { channels: u8, bgra: bool },
    /// 16-bit integers as a 16-bit PNG.
    Png16 { channels: u8 },
    /// Half floats as OpenEXR half.
    ExrHalf { channels: u8 },
    /// 32-bit floats (depth among them) as OpenEXR f32.
    ExrF32 { channels: u8, depth: bool },
    /// 32-bit unsigned integers as OpenEXR u32.
    ExrU32 { channels: u8 },
    /// The texels as copied.
    Raw,
}

impl Storage {
    pub(crate) fn of(format: wgpu::TextureFormat) -> Self {
        use wgpu::TextureFormat as F;
        match format {
            F::R8Unorm | F::R8Uint => Self::Png8 {
                channels: 1,
                bgra: false,
            },
            F::Rg8Unorm | F::Rg8Uint => Self::Png8 {
                channels: 2,
                bgra: false,
            },
            F::Rgba8Unorm | F::Rgba8UnormSrgb | F::Rgba8Uint => Self::Png8 {
                channels: 4,
                bgra: false,
            },
            F::Bgra8Unorm | F::Bgra8UnormSrgb => Self::Png8 {
                channels: 4,
                bgra: true,
            },
            F::R16Uint | F::R16Unorm => Self::Png16 { channels: 1 },
            F::Rg16Uint | F::Rg16Unorm => Self::Png16 { channels: 2 },
            F::Rgba16Uint | F::Rgba16Unorm => Self::Png16 { channels: 4 },
            F::R16Float => Self::ExrHalf { channels: 1 },
            F::Rg16Float => Self::ExrHalf { channels: 2 },
            F::Rgba16Float => Self::ExrHalf { channels: 4 },
            F::R32Float => Self::ExrF32 {
                channels: 1,
                depth: false,
            },
            F::Rg32Float => Self::ExrF32 {
                channels: 2,
                depth: false,
            },
            F::Rgba32Float => Self::ExrF32 {
                channels: 4,
                depth: false,
            },
            F::Depth32Float => Self::ExrF32 {
                channels: 1,
                depth: true,
            },
            F::R32Uint => Self::ExrU32 { channels: 1 },
            F::Rg32Uint => Self::ExrU32 { channels: 2 },
            F::Rgba32Uint => Self::ExrU32 { channels: 4 },
            _ => Self::Raw,
        }
    }

    /// [`ImageDescription::stored`]'s word for it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Png8 { .. } => "png",
            Self::Png16 { .. } => "png16",
            Self::ExrHalf { .. } => "exr-half",
            Self::ExrF32 { .. } => "exr-f32",
            Self::ExrU32 { .. } => "exr-u32",
            Self::Raw => "raw",
        }
    }
}

/// Copies a capture times at most; one past it is copied untimed.
pub const MAX_TIMED_COPIES: u32 = 256;

/// Bytes of one copy's two timestamps.
const PAIR: u64 = 2 * wgpu::QUERY_SIZE as u64;

/// The capture's own timestamps (#1160): two a copy, one before and one
/// after, each pair resolved right after its copy into a 256-byte slot of
/// `resolve` (wgpu's `QUERY_RESOLVE_BUFFER_ALIGNMENT`) and copied from there
/// into 16 bytes of `readback`, which is mapped with the images.
struct CopyTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// Copies timed so far this frame.
    used: u32,
}

impl CopyTimer {
    /// A timer, or `None` when the device has no `TIMESTAMP_QUERY`.
    fn new(device: &wgpu::Device) -> Option<Self> {
        if !crate::gpu_timer::GpuTimer::supported(device) {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Frame Capture Copy Queries"),
            ty: wgpu::QueryType::Timestamp,
            count: MAX_TIMED_COPIES * 2,
        });
        let resolve = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Frame Capture Copy Resolve"),
            size: u64::from(MAX_TIMED_COPIES) * wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Frame Capture Copy Read-back"),
            size: u64::from(MAX_TIMED_COPIES) * PAIR,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Some(Self {
            queries,
            resolve,
            readback,
            used: 0,
        })
    }

    /// The next copy's slot, or `None` once [`MAX_TIMED_COPIES`] are used.
    fn slot(&mut self) -> Option<u32> {
        (self.used < MAX_TIMED_COPIES).then(|| {
            self.used += 1;
            self.used - 1
        })
    }

    /// A timestamp at this point in `encoder`, written as the frame's timer
    /// writes one: by an empty compute pass carrying it.
    fn write(&self, encoder: &mut wgpu::CommandEncoder, query: u32) {
        encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Frame Capture Timestamp"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &self.queries,
                beginning_of_pass_write_index: Some(query),
                end_of_pass_write_index: None,
            }),
        });
    }

    /// Send `slot`'s two timestamps toward the read-back.
    fn resolve(&self, encoder: &mut wgpu::CommandEncoder, slot: u32) {
        let at = u64::from(slot) * wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
        encoder.resolve_query_set(&self.queries, slot * 2..slot * 2 + 2, &self.resolve, at);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            at,
            &self.readback,
            u64::from(slot) * PAIR,
            PAIR,
        );
    }
}

/// One copy's GPU time from the read-back's ticks, ms: `None` for a pair
/// never written (0) or running backwards, as the frame's timer reads them.
fn copy_ms(ticks: &[u64], slot: u32, period_ns: f32) -> Option<f64> {
    let at = slot as usize * 2;
    let (&start, &end) = (ticks.get(at)?, ticks.get(at + 1)?);
    (start != 0 && end >= start).then(|| (end - start) as f64 * f64::from(period_ns) / 1e6)
}

/// One target's copy, recorded and not yet read.
struct Tap {
    target: String,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    /// Bytes per row in the buffer (aligned) and per row of texels.
    padded_row: u32,
    row: u32,
    buffer: wgpu::Buffer,
    /// Its slot in the capture's `CopyTimer`, when it was timed.
    timed: Option<u32>,
}

/// The capture being recorded: what the frame has copied so far.
pub(crate) struct FrameTaps {
    host: CaptureTargets,
    /// What `surface` and `depth` name in the view being drawn: the host's,
    /// or a comic panel's picture and depth (#880).
    pub view: CaptureTargets,
    taps: Vec<Tap>,
    /// Target ids read this frame, so a style's table isn't read again for a
    /// target an earlier pass already gave.
    tapped: Vec<String>,
    untapped: Vec<String>,
    record_ms: f64,
    /// The copies' timestamps (#1160), made at the first copy on a device
    /// that can time.
    timer: Option<CopyTimer>,
    /// Whether the device has been asked: one that can't time is asked once.
    timer_asked: bool,
    /// Nanoseconds a timestamp tick, from the queue the frame is encoded
    /// for ([`times_in`](Self::times_in)); 0 until it says, and then nothing
    /// is timed.
    period_ns: f32,
}

impl FrameTaps {
    pub fn new(host: CaptureTargets) -> Self {
        Self {
            view: host.clone(),
            host,
            taps: Vec::new(),
            tapped: Vec::new(),
            untapped: Vec::new(),
            record_ms: 0.0,
            timer: None,
            timer_asked: false,
            period_ns: 0.0,
        }
    }

    /// Start the frame again: a `prepare` begins the frame being described.
    pub fn reset(&mut self) {
        self.view = self.host.clone();
        self.taps.clear();
        self.tapped.clear();
        self.untapped.clear();
        self.record_ms = 0.0;
        if let Some(timer) = &mut self.timer {
            timer.used = 0;
        }
    }

    /// Read the copies' timestamps in `queue`'s period: the frame is
    /// encoded for it (`encode_to`, which every captured frame goes
    /// through, whether or not its `prepare` came after the capture was
    /// armed).
    pub fn times_in(&mut self, queue: &wgpu::Queue) {
        self.period_ns = queue.get_timestamp_period();
    }

    /// The timer slot for the next copy: `None` on a device that can't
    /// time, before `times_in` gave a period, or past [`MAX_TIMED_COPIES`].
    fn timer_slot(&mut self, device: &wgpu::Device) -> Option<u32> {
        if self.period_ns <= 0.0 {
            return None;
        }
        if !self.timer_asked {
            self.timer_asked = true;
            self.timer = CopyTimer::new(device);
        }
        self.timer.as_mut()?.slot()
    }

    /// The host's own targets, for a view drawn into them.
    pub fn host(&self) -> &CaptureTargets {
        &self.host
    }

    /// Whether a target of this id has been read this frame.
    pub fn was_tapped(&self, id: &str) -> bool {
        self.tapped.iter().any(|t| t == id)
    }

    /// Say a target `pass` wrote isn't read, and why.
    pub fn untapped(&mut self, pass: &str, id: &str, why: &str) {
        let line = format!("{pass}: {id} ({why})");
        if !self.untapped.contains(&line) {
            self.untapped.push(line);
        }
    }

    /// Copy `texture` (its array `layer`, or a plain texture's only one) into
    /// a staging buffer of its own in `encoder`, and name the image on `pass`.
    #[allow(clippy::too_many_arguments)]
    pub fn copy(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        pass: &mut LoggedPass,
        id: &str,
        step: Option<&str>,
        texture: &wgpu::Texture,
        layer: Option<u32>,
    ) {
        let started = Instant::now();
        let name = pass.pass.name.clone();
        if texture.sample_count() > 1 {
            self.untapped(&name, id, "multisampled: its resolve is read");
            return;
        }
        if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
            self.untapped(&name, id, "made without COPY_SRC");
            return;
        }
        let format = texture.format();
        let aspect = if format.is_depth_stencil_format() {
            wgpu::TextureAspect::DepthOnly
        } else {
            wgpu::TextureAspect::All
        };
        let Some(texel) = format
            .block_copy_size(Some(aspect))
            .filter(|_| format.block_dimensions() == (1, 1))
        else {
            self.untapped(&name, id, "its format can't be copied out");
            return;
        };
        let (width, height) = (texture.width(), texture.height());
        let row = width * texel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = row.div_ceil(align) * align;
        let buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Frame Capture Tap"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let timed = self.timer_slot(device);
        if let (Some(slot), Some(timer)) = (timed, &self.timer) {
            timer.write(encoder, slot * 2);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer.unwrap_or(0),
                },
                aspect,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        if let (Some(slot), Some(timer)) = (timed, &self.timer) {
            timer.write(encoder, slot * 2 + 1);
            timer.resolve(encoder, slot);
        }
        pass.pass.images.push(ImageDescription {
            target: id.to_string(),
            step: step.map(str::to_string),
            layer,
            format: format!("{format:?}"),
            size: [width, height],
            file: String::new(),
            stored: Storage::of(format).name().to_string(),
            gpu_copy_ms: None,
            tap: Some(self.taps.len()),
        });
        self.taps.push(Tap {
            target: id.to_string(),
            format,
            width,
            height,
            padded_row,
            row,
            buffer,
            timed,
        });
        if !self.was_tapped(id) {
            self.tapped.push(id.to_string());
        }
        self.record_ms += started.elapsed().as_secs_f64() * 1000.0;
    }

    /// The recorded copies, for the host to read once the frame is
    /// submitted.
    pub fn finish(self) -> FrameCapture {
        FrameCapture {
            timer: self.timer.filter(|t| t.used > 0),
            period_ns: self.period_ns,
            taps: self.taps,
            untapped: self.untapped,
            record_ms: self.record_ms,
            mapped: Arc::new(AtomicUsize::new(0)),
            failed: Arc::new(AtomicBool::new(false)),
            mapping: false,
            wait_ms: None,
        }
    }
}

/// A frame's copies, submitted with it and read back after (ADR 0126
/// decision 5). [`map`](Self::map) once the frame is submitted, then
/// [`poll`](Self::poll) on later frames until it says ready, or
/// [`wait`](Self::wait) where nothing has a frame to keep (`flint render`);
/// then [`read`](Self::read).
pub struct FrameCapture {
    taps: Vec<Tap>,
    untapped: Vec<String>,
    record_ms: f64,
    /// The copies' timestamps when any copy was timed (#1160): mapped with
    /// the images, one more buffer in `mapped`.
    timer: Option<CopyTimer>,
    period_ns: f32,
    mapped: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
    mapping: bool,
    wait_ms: Option<f64>,
}

impl FrameCapture {
    /// How many images it holds.
    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }

    /// The staging it holds: every image's rows as the GPU aligns them.
    pub fn staging_bytes(&self) -> u64 {
        self.taps.iter().map(|t| t.buffer.size()).sum()
    }

    /// Buffers [`map`](Self::map) maps: every image's, and the copies'
    /// timestamps when any was timed.
    fn buffers(&self) -> usize {
        self.taps.len() + usize::from(self.timer.is_some())
    }

    /// Ask for every copy to be mapped. Call once its frame is submitted.
    pub fn map(&mut self) {
        if self.mapping {
            return;
        }
        self.mapping = true;
        for tap in &self.taps {
            let (mapped, failed) = (self.mapped.clone(), self.failed.clone());
            tap.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    mapped.fetch_add(1, Ordering::Release);
                });
        }
        if let Some(timer) = &self.timer {
            let (mapped, failed) = (self.mapped.clone(), self.failed.clone());
            timer
                .readback
                .slice(..u64::from(timer.used) * PAIR)
                .map_async(wgpu::MapMode::Read, move |result| {
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    mapped.fetch_add(1, Ordering::Release);
                });
        }
    }

    /// Whether every copy has been mapped.
    pub fn ready(&self) -> bool {
        self.mapping && self.mapped.load(Ordering::Acquire) == self.buffers()
    }

    /// Let the device run its callbacks without waiting, and say whether
    /// the copies are ready: what a window calls on the frames after.
    pub fn poll(&mut self, device: &wgpu::Device) -> bool {
        self.map();
        if !self.ready() {
            let _ = device.poll(wgpu::PollType::Poll);
        }
        self.ready()
    }

    /// Wait for the copies (through [`wait_for_gpu`](crate::wait_for_gpu)):
    /// for a host with no frame to keep.
    pub fn wait(&mut self, device: &wgpu::Device) {
        self.map();
        let started = Instant::now();
        while !self.ready() {
            let _ = crate::wait_for_gpu(device);
        }
        self.wait_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    }

    /// Copy every image out of mapped memory, rows unpadded. `Err` while
    /// a copy isn't mapped yet, or when one failed to map.
    pub fn read(self) -> Result<CapturedFrame, String> {
        if !self.ready() {
            return Err(format!(
                "{} of {} frame capture copies mapped: poll until ready",
                self.mapped.load(Ordering::Acquire),
                self.buffers()
            ));
        }
        if self.failed.load(Ordering::Relaxed) {
            return Err("a frame capture copy failed to map".to_string());
        }
        let started = Instant::now();
        let ticks: Vec<u64> = match &self.timer {
            Some(timer) => {
                let ticks = {
                    let data = timer
                        .readback
                        .slice(..u64::from(timer.used) * PAIR)
                        .get_mapped_range()
                        .map_err(|e| format!("frame capture copy times: {e}"))?;
                    bytemuck::pod_collect_to_vec(&data)
                };
                timer.readback.unmap();
                ticks
            }
            None => Vec::new(),
        };
        let mut images = Vec::with_capacity(self.taps.len());
        for (index, tap) in self.taps.into_iter().enumerate() {
            let data = tap
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| format!("frame capture copy {index}: {e}"))?;
            let mut bytes = Vec::with_capacity(tap.row as usize * tap.height as usize);
            for y in 0..tap.height as usize {
                let start = y * tap.padded_row as usize;
                bytes.extend_from_slice(&data[start..start + tap.row as usize]);
            }
            drop(data);
            tap.buffer.unmap();
            images.push(CapturedImage {
                tap: index,
                target: tap.target,
                format: tap.format,
                width: tap.width,
                height: tap.height,
                bytes,
                gpu_ms: tap
                    .timed
                    .and_then(|slot| copy_ms(&ticks, slot, self.period_ns)),
            });
        }
        let timed: Vec<f64> = images.iter().filter_map(|i| i.gpu_ms).collect();
        Ok(CapturedFrame {
            gpu_copy_ms: (!timed.is_empty()).then(|| timed.iter().sum()),
            gpu_copies_timed: timed.len() as u32,
            images,
            untapped: self.untapped,
            record_ms: self.record_ms,
            wait_ms: self.wait_ms,
            copy_out_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

/// One image read back: a target's texels as a pass left them, tightly
/// packed.
pub struct CapturedImage {
    /// Which of the frame's images it is ([`ImageDescription`]'s, in memory).
    pub tap: usize,
    pub target: String,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
    /// What its copy took on the GPU, ms (#1160); `None` untimed.
    pub gpu_ms: Option<f64>,
}

/// A frame's images, read back, with what reading them cost.
pub struct CapturedFrame {
    pub images: Vec<CapturedImage>,
    /// Every timed copy's GPU time together, ms (#1160); `None` when none
    /// was timed.
    pub gpu_copy_ms: Option<f64>,
    /// How many of the images' copies were timed: all of them on a device
    /// that can time, up to [`MAX_TIMED_COPIES`].
    pub gpu_copies_timed: u32,
    /// Targets passes wrote that weren't read, each with why.
    pub untapped: Vec<String>,
    /// Recording the copies, on the frame thread, ms.
    pub record_ms: f64,
    /// Waiting for them, ms, when the host waited.
    pub wait_ms: Option<f64>,
    /// Copying them out of mapped memory, ms.
    pub copy_out_ms: f64,
}

impl CapturedFrame {
    /// The image a description's image names.
    pub fn image(&self, described: &ImageDescription) -> Option<&CapturedImage> {
        described.tap.and_then(|i| self.images.get(i))
    }
}

/// `text` as a file name's part: letters, digits and `_`.
pub(crate) fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_format_is_stored_exactly() {
        use wgpu::TextureFormat as F;
        assert_eq!(Storage::of(F::Rgba16Float).name(), "exr-half");
        assert_eq!(Storage::of(F::Depth32Float).name(), "exr-f32");
        assert_eq!(Storage::of(F::R8Unorm).name(), "png");
        assert_eq!(Storage::of(F::Rg16Uint).name(), "png16");
        assert_eq!(
            Storage::of(F::Bgra8UnormSrgb),
            Storage::Png8 {
                channels: 4,
                bgra: true
            }
        );
        assert_eq!(Storage::of(F::Rgb10a2Unorm).name(), "raw");
    }

    #[test]
    fn a_copys_time_is_its_two_ticks_apart() {
        // 1 ns a tick: slot 1 is 2.5 ms
        let ticks = [10, 20, 1_000, 2_501_000, 0, 7, 9, 3];
        assert_eq!(copy_ms(&ticks, 1, 1.0), Some(2.5));
        // never written, or running backwards: untimed, as the frame's
        // timer reads them
        assert_eq!(copy_ms(&ticks, 2, 1.0), None);
        assert_eq!(copy_ms(&ticks, 3, 1.0), None);
        // past what was read back
        assert_eq!(copy_ms(&ticks, 4, 1.0), None);
        // the period scales it: 10 ticks of 1 ms
        assert_eq!(copy_ms(&ticks, 0, 1_000_000.0), Some(10.0));
    }

    #[test]
    fn a_slug_is_a_file_name() {
        assert_eq!(slug("style hdr ping 0"), "style_hdr_ping_0");
        assert_eq!(slug("Shadow cascade 2"), "shadow_cascade_2");
        assert_eq!(slug("kuwahara.tensor"), "kuwahara_tensor");
    }
}
