//! GPU time per pass, from timestamp queries (ADR 0108).
//!
//! A [`GpuTimer`] brackets parts of a frame with timestamps written on the
//! GPU, resolves them into a buffer at the end of the frame and reads it
//! back a frame or more later without ever waiting, as the grass count is
//! read (ADR 0092). Each bracket is a [`GpuScope`]: [`GpuTimer::begin`] and
//! [`GpuTimer::end`] in one encoder, or [`GpuTimer::begin_submit`] and
//! [`GpuTimer::end_submit`] around work spread over several submissions.
//!
//! A timestamp is written by an empty compute pass carrying it, so only
//! `Features::TIMESTAMP_QUERY` is needed, not timestamps inside encoders,
//! and no pass descriptor changes. A scope measures from its first
//! timestamp to its second on the queue's clock: the GPU's time on the work
//! between them, plus any time the GPU sat idle waiting for the next
//! submission inside the scope.
//!
//! [`SceneRenderer`](crate::SceneRenderer) owns one when timing is on and
//! scopes its own phases; hosts add theirs through
//! [`SceneRenderer::gpu_timer_mut`](crate::SceneRenderer::gpu_timer_mut).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Scopes one frame can hold; more in a frame are not timed.
pub const MAX_SCOPES: u32 = 32;

/// Read-backs in flight at once. A frame that finds them all busy is not
/// timed, so the figures come from at most every frame but never stall.
const READBACKS: usize = 3;

/// How much of each new frame's time goes into the smoothed figure.
const SMOOTHING: f32 = 0.1;

/// One scope's GPU time, smoothed over recent frames.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuPassTime {
    pub label: &'static str,
    pub ms: f32,
}

/// The smoothed GPU times of the last frames read back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuTimings {
    /// Each scope, in the order the latest frame began them.
    pub passes: Vec<GpuPassTime>,
    /// The scopes' total: the GPU's busy time in the frame. Time between
    /// scopes, such as a wait for the swapchain image under vsync, isn't in
    /// it, and a scope inside another counts twice.
    pub frame_ms: f32,
}

/// A scope [`GpuTimer::begin`] opened, to hand back to [`GpuTimer::end`].
/// One past the frame's capacity times nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "a scope is timed only once ended"]
pub struct GpuScope(Option<u32>);

struct Scope {
    label: &'static str,
    /// Its first query; the second is the next.
    first: u32,
    ended: bool,
}

struct Readback {
    buffer: wgpu::Buffer,
    labels: Vec<&'static str>,
    /// Queries copied in.
    count: u32,
    in_flight: bool,
    /// 0 while the map is pending, 1 mapped, 2 failed
    state: Arc<AtomicU8>,
}

/// Brackets a frame's work with GPU timestamps; see the module docs.
pub struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readbacks: Vec<Readback>,
    /// Nanoseconds per timestamp tick.
    period_ns: f32,
    scopes: Vec<Scope>,
    timings: GpuTimings,
    /// The read-back [`resolve_into`](Self::resolve_into) filled, to map
    /// once its submission is made.
    resolved: Option<usize>,
}

impl GpuTimer {
    /// Whether `device` can time: it must have been created with
    /// `Features::TIMESTAMP_QUERY`.
    pub fn supported(device: &wgpu::Device) -> bool {
        device.features().contains(wgpu::Features::TIMESTAMP_QUERY)
    }

    /// A timer, or `None` when the device can't time.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !Self::supported(device) {
            return None;
        }
        let queries = MAX_SCOPES * 2;
        let size = queries as u64 * wgpu::QUERY_SIZE as u64;
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("GPU Timer Queries"),
            ty: wgpu::QueryType::Timestamp,
            count: queries,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("GPU Timer Resolve"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readbacks = (0..READBACKS)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("GPU Timer Read-back"),
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                labels: Vec::new(),
                count: 0,
                in_flight: false,
                state: Default::default(),
            })
            .collect();
        Some(Self {
            query_set,
            resolve,
            readbacks,
            period_ns: queue.get_timestamp_period(),
            scopes: Vec::new(),
            timings: GpuTimings::default(),
            resolved: None,
        })
    }

    /// Open a scope named `label` at this point in `encoder`.
    pub fn begin(&mut self, encoder: &mut wgpu::CommandEncoder, label: &'static str) -> GpuScope {
        if self.scopes.len() as u32 >= MAX_SCOPES {
            return GpuScope(None);
        }
        let first = self.scopes.len() as u32 * 2;
        self.write(encoder, first);
        self.scopes.push(Scope {
            label,
            first,
            ended: false,
        });
        GpuScope(Some(first))
    }

    /// Close `scope` at this point in `encoder`.
    pub fn end(&mut self, encoder: &mut wgpu::CommandEncoder, scope: GpuScope) {
        let Some(first) = scope.0 else { return };
        let Some(open) = self
            .scopes
            .iter_mut()
            .find(|s| s.first == first && !s.ended)
        else {
            return;
        };
        open.ended = true;
        self.write(encoder, first + 1);
    }

    /// [`begin`](Self::begin) in a submission of its own, for a scope round
    /// work that makes its own encoders.
    pub fn begin_submit(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
    ) -> GpuScope {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("GPU Timer Begin"),
        });
        let scope = self.begin(&mut encoder, label);
        queue.submit(std::iter::once(encoder.finish()));
        scope
    }

    /// [`end`](Self::end) in a submission of its own.
    pub fn end_submit(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scope: GpuScope) {
        if scope.0.is_none() {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("GPU Timer End"),
        });
        self.end(&mut encoder, scope);
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// End the frame: pick up any frame read back since the last call, then
    /// send this frame's timestamps back. Call once a frame, after its last
    /// scope's submission. A scope left open ends here.
    pub fn finish_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("GPU Timer Resolve"),
        });
        if self.resolve_into(device, &mut encoder) {
            queue.submit(std::iter::once(encoder.finish()));
            self.start_readback();
        }
    }

    /// [`finish_frame`](Self::finish_frame) in the frame's own encoder, so
    /// timing adds no submission (#139): record the resolve after the
    /// frame's last scope, submit, then call
    /// [`start_readback`](Self::start_readback). False when this frame goes
    /// untimed and there is nothing to read back.
    pub fn resolve_into(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> bool {
        self.poll(device);
        if self.scopes.is_empty() {
            return false;
        }
        let scopes = std::mem::take(&mut self.scopes);
        let Some(slot) = self.readbacks.iter().position(|r| !r.in_flight) else {
            // All three still on their way: this frame goes untimed.
            return false;
        };
        let count = scopes.len() as u32 * 2;
        for scope in scopes.iter().filter(|s| !s.ended) {
            self.write(encoder, scope.first + 1);
        }
        let readback = &mut self.readbacks[slot];
        encoder.resolve_query_set(&self.query_set, 0..count, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            0,
            &readback.buffer,
            0,
            count as u64 * wgpu::QUERY_SIZE as u64,
        );
        readback.labels = scopes.into_iter().map(|s| s.label).collect();
        readback.count = count;
        readback.in_flight = true;
        readback.state.store(0, Ordering::Release);
        self.resolved = Some(slot);
        true
    }

    /// Map the read-back [`resolve_into`](Self::resolve_into) recorded, now
    /// that its submission is made.
    pub fn start_readback(&mut self) {
        let Some(slot) = self.resolved.take() else {
            return;
        };
        let readback = &self.readbacks[slot];
        let state = readback.state.clone();
        readback
            .buffer
            .slice(..readback.count as u64 * wgpu::QUERY_SIZE as u64)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
            });
    }

    /// The smoothed times of the frames read back so far.
    pub fn timings(&self) -> &GpuTimings {
        &self.timings
    }

    /// Fold in every read-back that has come in. Polls without blocking.
    fn poll(&mut self, device: &wgpu::Device) {
        if !self.readbacks.iter().any(|r| r.in_flight) {
            return;
        }
        device.poll(wgpu::Maintain::Poll);
        for readback in &mut self.readbacks {
            if !readback.in_flight {
                continue;
            }
            match readback.state.load(Ordering::Acquire) {
                1 => {
                    let bytes = readback.count as u64 * wgpu::QUERY_SIZE as u64;
                    let ticks: Vec<u64> = {
                        let data = readback.buffer.slice(..bytes).get_mapped_range();
                        bytemuck::pod_collect_to_vec(&data)
                    };
                    readback.buffer.unmap();
                    readback.in_flight = false;
                    let frame = frame_times(&readback.labels, &ticks, self.period_ns);
                    smooth(&mut self.timings, frame);
                }
                2 => readback.in_flight = false,
                _ => {}
            }
        }
    }

    fn write(&self, encoder: &mut wgpu::CommandEncoder, query: u32) {
        encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("GPU Timestamp"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &self.query_set,
                beginning_of_pass_write_index: Some(query),
                end_of_pass_write_index: None,
            }),
        });
    }
}

/// One frame's times from its ticks, two per scope. A scope whose ticks
/// run backwards, or were never written (0), is left out.
fn frame_times(labels: &[&'static str], ticks: &[u64], period_ns: f32) -> GpuTimings {
    let ms = |ticks: u64| (ticks as f64 * period_ns as f64 / 1e6) as f32;
    let mut passes = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let (Some(&start), Some(&end)) = (ticks.get(2 * i), ticks.get(2 * i + 1)) else {
            break;
        };
        if start == 0 || end < start {
            continue;
        }
        passes.push(GpuPassTime {
            label,
            ms: ms(end - start),
        });
    }
    let frame_ms = passes.iter().map(|p| p.ms).sum();
    GpuTimings { passes, frame_ms }
}

/// Move `timings` toward `frame`. A scope new to it starts at its time; one
/// the frame lacks is dropped; the order is the frame's.
fn smooth(timings: &mut GpuTimings, frame: GpuTimings) {
    let ease = |old: f32, new: f32| old + (new - old) * SMOOTHING;
    let passes = frame
        .passes
        .into_iter()
        .map(|pass| {
            let ms = match timings.passes.iter().find(|p| p.label == pass.label) {
                Some(old) => ease(old.ms, pass.ms),
                None => pass.ms,
            };
            GpuPassTime { ms, ..pass }
        })
        .collect();
    timings.frame_ms = if timings.frame_ms > 0.0 {
        ease(timings.frame_ms, frame.frame_ms)
    } else {
        frame.frame_ms
    };
    timings.passes = passes;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_times_each_scope_and_totals_them() {
        // 1 ns a tick: scope a 1..3 ms, b 4..8 ms, idle between them.
        let ticks = [1_000_000, 3_000_000, 4_000_000, 8_000_000];
        let frame = frame_times(&["a", "b"], &ticks, 1.0);
        assert_eq!(
            frame.passes,
            [
                GpuPassTime {
                    label: "a",
                    ms: 2.0
                },
                GpuPassTime {
                    label: "b",
                    ms: 4.0
                }
            ]
        );
        assert_eq!(frame.frame_ms, 6.0, "the idle millisecond isn't counted");
    }

    #[test]
    fn unwritten_or_backward_scopes_are_left_out() {
        let ticks = [0, 5, 9, 7, 10, 30];
        let frame = frame_times(&["unwritten", "backward", "ok"], &ticks, 2.0);
        assert_eq!(frame.passes.len(), 1);
        assert_eq!(frame.passes[0].label, "ok");
        assert!((frame.passes[0].ms - 40e-6).abs() < 1e-9);
    }

    #[test]
    fn smoothing_eases_toward_each_frame_in_its_order() {
        let mut timings = GpuTimings::default();
        let frame = |a: f32, b: Option<f32>| GpuTimings {
            passes: [Some(("a", a)), b.map(|b| ("b", b))]
                .into_iter()
                .flatten()
                .map(|(label, ms)| GpuPassTime { label, ms })
                .collect(),
            frame_ms: a + b.unwrap_or(0.0),
        };
        smooth(&mut timings, frame(10.0, None));
        assert_eq!(timings.passes[0].ms, 10.0, "the first frame is taken whole");
        assert_eq!(timings.frame_ms, 10.0);

        smooth(&mut timings, frame(20.0, Some(4.0)));
        assert_eq!(timings.passes[0].ms, 11.0, "a tenth of the way");
        assert_eq!(timings.passes[1].ms, 4.0, "a new scope starts at its time");
        assert!((timings.frame_ms - 11.4).abs() < 1e-5);

        smooth(&mut timings, frame(11.0, None));
        assert_eq!(
            timings.passes.len(),
            1,
            "a scope the frame lacks is dropped"
        );
    }
}
