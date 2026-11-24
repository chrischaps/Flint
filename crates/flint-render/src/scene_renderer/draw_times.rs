//! Each scene draw's GPU time, on a replay (#1203).
//!
//! The frame's [`GpuTimer`](crate::gpu_timer::GpuTimer) times whole passes
//! (32 scopes, from empty compute passes), so it can't see inside the scene
//! pass. A replay that asks ([`DrawControls::time_draws`](super::DrawControls))
//! gets a query set of its own and a timestamp written inside the pass
//! before and after each draw it names, counted as the frame's description
//! lists the scene pass's draws (`log_scene_draws`: the list's items through
//! the main pass's selector, grass after the terrain, each 2D sprite batch
//! after the billboards). A draw's time is its second timestamp less its
//! first: on a GPU that writes a timestamp when the work before it is done,
//! the time from the draws before it finishing to it finishing, so a draw
//! overlapping the one before it is charged only for what it adds.
//!
//! Timestamps inside a pass need `TIMESTAMP_QUERY_INSIDE_PASSES` (native
//! Vulkan, DX12, Metal; not WebGPU), which the replay's device asks for.
//! A query set holds [`BATCH`] draws' pairs; a frame with more is timed a
//! batch a draw, and the times gather here until
//! [`SceneRenderer::reset_draw_times`](super::SceneRenderer::reset_draw_times).
//! No frame a game draws names any, and then nothing here runs but one
//! `Option` check a pass.

use crate::gpu_counts::CountedDevice;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Draws one query set times: two timestamps each.
pub const BATCH: usize = (wgpu::QUERY_SET_MAX_QUERIES / 2) as usize;

/// The features a device needs for draw times.
pub const FEATURES: wgpu::Features =
    wgpu::Features::TIMESTAMP_QUERY.union(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES);

struct Timer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
}

/// What the last timed frame left, once read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrawTimesRead {
    /// The scene pass's draws in that frame, as the description lists them.
    pub drawn: usize,
    /// How many of them that frame timed.
    pub timed: usize,
}

#[derive(Default)]
pub(super) struct DrawTimes {
    /// The draws this frame times, by their place in the scene pass's list.
    range: Option<Range<usize>>,
    timer: Option<Timer>,
    /// Why nothing is timed though a range was asked for.
    refused: Option<&'static str>,
    /// Nanoseconds a tick, from the queue.
    period_ns: f32,
    /// The next draw's place in the list, counted as the pass draws.
    next: AtomicUsize,
    /// The frame's timed pairs, resolved and waiting to be read.
    pending: Option<(usize, usize)>,
    /// The list's length as the last timed frame drew it.
    drawn: usize,
    /// Every time read since the last reset, by place in the list.
    times: Vec<Option<f64>>,
}

impl DrawTimes {
    /// Take the frame's range (`prepare`): clamped to one batch, and made
    /// ready on a device that can time inside passes.
    pub fn arm(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, range: Option<Range<usize>>) {
        self.pending = None;
        self.range = range.map(|r| r.start..r.end.min(r.start + BATCH));
        if self.range.is_none() {
            return;
        }
        if !device.features().contains(FEATURES) {
            self.refused = Some("the device has no TIMESTAMP_QUERY_INSIDE_PASSES");
            self.range = None;
            return;
        }
        self.refused = None;
        self.period_ns = queue.get_timestamp_period();
        if self.timer.is_none() {
            let count = (BATCH * 2) as u32;
            let size = u64::from(count) * 8;
            self.timer = Some(Timer {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("Draw Time Queries"),
                    ty: wgpu::QueryType::Timestamp,
                    count,
                }),
                resolve: device.create_buffer_counted(&wgpu::BufferDescriptor {
                    label: Some("Draw Time Resolve"),
                    size,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer_counted(&wgpu::BufferDescriptor {
                    label: Some("Draw Time Read-back"),
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
            });
        }
    }

    /// Whether this frame times any draw.
    pub fn on(&self) -> bool {
        self.range.is_some() && self.timer.is_some()
    }

    /// Why a range asked for times nothing.
    pub fn refused(&self) -> Option<&'static str> {
        self.refused
    }

    /// The scene pass begins (again, under a repeat knob): count from 0.
    pub fn begin_pass(&self) {
        self.next.store(0, Ordering::Relaxed);
    }

    /// The next draw of the list begins: its first timestamp when it is
    /// timed, and the pair's slot to hand [`after`](Self::after).
    pub fn before(&self, pass: &mut wgpu::RenderPass<'_>) -> Option<u32> {
        let (Some(range), Some(timer)) = (&self.range, &self.timer) else {
            return None;
        };
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        if !range.contains(&n) {
            return None;
        }
        let slot = (n - range.start) as u32;
        pass.write_timestamp(&timer.queries, slot * 2);
        Some(slot)
    }

    /// The draw [`before`](Self::before) began has been recorded.
    pub fn after(&self, pass: &mut wgpu::RenderPass<'_>, slot: Option<u32>) {
        if let (Some(slot), Some(timer)) = (slot, &self.timer) {
            pass.write_timestamp(&timer.queries, slot * 2 + 1);
        }
    }

    /// `n` draws of the list the pass counts but can't draw (a layer whose
    /// pipeline this GPU lacks): untimed, so the ones after keep their
    /// places.
    pub fn skip(&self, n: usize) {
        if self.on() {
            self.next.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// The scene pass ended: send its pairs toward the read-back.
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(range), Some(timer)) = (&self.range, &self.timer) else {
            return;
        };
        let drawn = self.next.load(Ordering::Relaxed);
        let timed = range.end.min(drawn).saturating_sub(range.start);
        self.drawn = drawn;
        self.pending = Some((range.start, timed));
        if timed == 0 {
            return;
        }
        let queries = (timed * 2) as u32;
        encoder.resolve_query_set(&timer.queries, 0..queries, &timer.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &timer.resolve,
            0,
            &timer.readback,
            0,
            u64::from(queries) * 8,
        );
    }

    /// Read what the submitted frame timed (waiting for it), into the
    /// gathered times. `None` when the frame timed nothing.
    pub fn read(&mut self, device: &wgpu::Device) -> Option<DrawTimesRead> {
        let (start, timed) = self.pending.take()?;
        let read = DrawTimesRead {
            drawn: self.drawn,
            timed,
        };
        if self.times.len() < self.drawn {
            self.times.resize(self.drawn, None);
        }
        if timed == 0 {
            return Some(read);
        }
        let timer = self.timer.as_ref()?;
        let bytes = (timed * 2 * 8) as u64;
        let slice = timer.readback.slice(..bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = crate::wait_for_gpu(device);
        if !matches!(rx.recv(), Ok(Ok(()))) {
            return None;
        }
        {
            let Ok(data) = slice.get_mapped_range() else {
                timer.readback.unmap();
                return None;
            };
            let ticks: Vec<u64> = bytemuck::pod_collect_to_vec(&data);
            for k in 0..timed {
                let (a, b) = (ticks[k * 2], ticks[k * 2 + 1]);
                let ms =
                    (a != 0 && b >= a).then(|| (b - a) as f64 * f64::from(self.period_ns) / 1e6);
                if let Some(t) = self.times.get_mut(start + k) {
                    *t = ms;
                }
            }
        }
        timer.readback.unmap();
        Some(read)
    }

    /// Every time gathered, by place in the scene pass's list.
    pub fn times(&self) -> &[Option<f64>] {
        &self.times
    }

    /// Forget the gathered times.
    pub fn reset(&mut self) {
        self.times.clear();
        self.drawn = 0;
        self.pending = None;
    }
}
