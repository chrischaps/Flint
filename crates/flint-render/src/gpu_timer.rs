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

use crate::gpu_counts::CountedDevice;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Scopes one frame can hold; more in a frame are not timed.
pub const MAX_SCOPES: u32 = 32;

/// Read-backs in flight at once. A frame that finds them all busy is not
/// timed, so the figures come from at most every frame but never stall.
const READBACKS: usize = 3;

/// Frames [`GpuTimer::tally`] keeps at most: an hour at 60 fps, about
/// 80 MB at twenty scopes a frame.
pub const TALLY_FRAMES: usize = 216_000;

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

/// One frame's scopes as measured, for a trace (#1097): each scope's start
/// from the frame's first timestamp and its length, unsmoothed, under the
/// tag the host gave the frame ([`GpuTimer::set_frame_tag`]), so a trace
/// can lay a read-back a frame or two late beside the frame that drew it.
#[derive(Debug, Clone, PartialEq)]
pub struct TracedGpuFrame {
    pub tag: u64,
    pub passes: Vec<TracedPass>,
}

/// One scope of a [`TracedGpuFrame`], ms.
#[derive(Debug, Clone, PartialEq)]
pub struct TracedPass {
    pub label: &'static str,
    /// From the frame's first timestamp to this scope's first.
    pub start_ms: f32,
    pub ms: f32,
}

/// Traced frames kept for the host to take; more and the oldest go.
const TRACED_KEPT: usize = 16;

/// One frame's GPU busy time, ms and unsmoothed, under the index the host
/// gave the frame when it was resolved ([`GpuTimer::set_frame_index`]), so
/// the host can lay it on the frame it timed rather than the one it came
/// back in (#1104). Read-backs come back a frame or two late, and not
/// always in the order the frames were drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuFrameTime {
    /// The host's index for the frame; `None` when it gave none.
    pub index: Option<u64>,
    pub ms: f32,
}

/// Frame times kept for the host to take; more and the oldest go.
const FRAME_TIMES_KEPT: usize = 16;

/// Frames read back before [`TimestampCheck`] gives a verdict, and again
/// for each one after: a second at 60 fps.
pub const TIMESTAMP_WINDOW: u32 = 60;

/// Whether an adapter's timestamps read as pass times (#1316).
///
/// On the Pixel 8 Pro (Mali-G715, Chrome) every scope after `Scene` reads
/// exactly zero ticks, frame after frame, so a readout would list zeros
/// that look like an answer. A real pass is never exactly zero ticks for a
/// whole window: an empty one is ~2 us (#507), thousands of ticks, and
/// even a browser that rounds timestamps to 100 us reads a small pass as
/// non-zero on the frames it straddles a step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimestampHealth {
    /// Fewer than [`TIMESTAMP_WINDOW`] frames read back yet: the times are
    /// shown as they are.
    #[default]
    Unchecked,
    /// The last window's scopes read as times.
    Usable,
    /// In the last window, `zero` of its `scopes` read exactly zero ticks
    /// in every frame that timed them: at least two, and at least half.
    Unusable { zero: u32, scopes: u32 },
}

impl TimestampHealth {
    /// Whether the pass times are known to be empty, so a reader should say
    /// so instead of listing them.
    pub fn unusable(self) -> bool {
        matches!(self, TimestampHealth::Unusable { .. })
    }

    /// Why, for a reader with room for a clause: `None` while the times
    /// can be shown.
    pub fn why(self) -> Option<String> {
        match self {
            TimestampHealth::Unusable { zero, scopes } => Some(format!(
                "{zero} of {scopes} scopes read 0 ticks for {TIMESTAMP_WINDOW} frames"
            )),
            _ => None,
        }
    }
}

/// What every reader says in place of the pass times (#1316).
pub const TIMESTAMPS_UNAVAILABLE: &str = "GPU pass times unavailable on this adapter";

/// Judges [`TimestampHealth`] from the raw frames read back, a window of
/// [`TIMESTAMP_WINDOW`] at a time; once warm it allocates nothing.
#[derive(Debug, Clone, Default)]
pub struct TimestampCheck {
    /// Frames observed in the window under way.
    frames: u32,
    /// Each scope (label, which of that label in its frame) seen in the
    /// window, and whether every frame that timed it read exactly zero.
    scopes: Vec<(&'static str, usize, bool)>,
    verdict: TimestampHealth,
}

impl TimestampCheck {
    /// Fold in one frame as measured (unsmoothed: a smoothed time decays
    /// toward zero but never reaches it). A frame's time is zero only when
    /// its two ticks are the same, so this is a test on raw ticks.
    pub fn observe(&mut self, frame: &GpuTimings) {
        if frame.passes.is_empty() {
            return;
        }
        for (i, pass) in frame.passes.iter().enumerate() {
            let nth = frame.passes[..i]
                .iter()
                .filter(|p| p.label == pass.label)
                .count();
            let zero = pass.ms == 0.0;
            match self
                .scopes
                .iter_mut()
                .find(|(l, n, _)| *l == pass.label && *n == nth)
            {
                Some((_, _, all_zero)) => *all_zero &= zero,
                None => self.scopes.push((pass.label, nth, zero)),
            }
        }
        self.frames += 1;
        if self.frames >= TIMESTAMP_WINDOW {
            let scopes = self.scopes.len() as u32;
            let zero = self.scopes.iter().filter(|s| s.2).count() as u32;
            self.verdict = if zero >= 2 && zero * 2 >= scopes {
                TimestampHealth::Unusable { zero, scopes }
            } else {
                TimestampHealth::Usable
            };
            self.frames = 0;
            self.scopes.clear();
        }
    }

    /// The last whole window's verdict.
    pub fn verdict(&self) -> TimestampHealth {
        self.verdict
    }
}

/// A scope [`GpuTimer::begin`] opened, to hand back to [`GpuTimer::end`].
/// One past the frame's capacity times nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "a scope is timed only once ended"]
pub struct GpuScope(Option<u32>);

impl GpuScope {
    /// Whether the frame had room to time it ([`MAX_SCOPES`]).
    pub fn is_timed(self) -> bool {
        self.0.is_some()
    }
}

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
    /// Whether recording was on when this frame was resolved, so a frame
    /// drawn before [`GpuTimer::record`] and read back after isn't kept.
    record: bool,
    /// The same for the run's tally ([`GpuTimer::tally`]).
    tally: bool,
    /// The host's tag for the frame, and whether tracing was on when it
    /// was resolved.
    tag: u64,
    trace: bool,
    /// The host's index for the frame ([`GpuTimer::set_frame_index`]).
    index: Option<u64>,
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
    /// Every frame read back, raw, while [`record`](Self::record) is on
    /// (#292); `None` keeps nothing, so a long-running player doesn't grow.
    recorded: Option<Vec<GpuTimings>>,
    /// Every frame read back while [`tally`](Self::tally) is on: a whole
    /// run's, for a perf report (#1093). Apart from `recorded`, which a
    /// frame capture turns on and off around its one frame.
    tallied: Option<Vec<GpuTimings>>,
    /// Each frame read back since the last
    /// [`take_frame_times`](Self::take_frame_times), oldest first.
    frame_times: std::collections::VecDeque<GpuFrameTime>,
    /// What [`set_frame_tag`](Self::set_frame_tag) said last.
    frame_tag: u64,
    /// What [`set_frame_index`](Self::set_frame_index) said last.
    frame_index: Option<u64>,
    /// Frames read back while [`trace`](Self::trace) is on, until
    /// [`take_traced`](Self::take_traced) takes them.
    traced: Option<std::collections::VecDeque<TracedGpuFrame>>,
    /// Whether the timestamps read as times at all (#1316).
    check: TimestampCheck,
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
        let resolve = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("GPU Timer Resolve"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readbacks = (0..READBACKS)
            .map(|_| Readback {
                buffer: device.create_buffer_counted(&wgpu::BufferDescriptor {
                    label: Some("GPU Timer Read-back"),
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                labels: Vec::new(),
                count: 0,
                in_flight: false,
                record: false,
                tally: false,
                tag: 0,
                trace: false,
                index: None,
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
            recorded: None,
            tallied: None,
            frame_times: std::collections::VecDeque::with_capacity(FRAME_TIMES_KEPT),
            frame_tag: 0,
            frame_index: None,
            traced: None,
            check: TimestampCheck::default(),
        })
    }

    /// Keep every frame's raw times from now on, or stop and drop them
    /// (#292). The smoothed figure suits an overlay; a benchmark wants the
    /// spread, which [`spread`] makes of [`recorded`](Self::recorded).
    pub fn record(&mut self, on: bool) {
        self.recorded = on.then(Vec::new);
    }

    /// Keep every frame's raw times from now on for the run's tally, or
    /// stop and drop them (#1093). Apart from [`record`](Self::record), so a
    /// frame capture in the middle of a run neither clears nor joins it. At
    /// most [`TALLY_FRAMES`] are kept; the frames after are left out.
    pub fn tally(&mut self, on: bool) {
        self.tallied = on.then(Vec::new);
    }

    /// Whether the run's tally is on.
    pub fn tallying(&self) -> bool {
        self.tallied.is_some()
    }

    /// The frames [`tally`](Self::tally) kept, oldest first.
    pub fn tallied(&self) -> &[GpuTimings] {
        self.tallied.as_deref().unwrap_or(&[])
    }

    /// Keep each frame read back from now on as a [`TracedGpuFrame`], for a
    /// trace (#1097), or stop. Turning it on again keeps what waits.
    pub fn trace(&mut self, on: bool) {
        if !on {
            self.traced = None;
        } else if self.traced.is_none() {
            self.traced = Some(Default::default());
        }
    }

    /// The tag the frames resolved from now on carry (the host's frame
    /// index), until it says another.
    pub fn set_frame_tag(&mut self, tag: u64) {
        self.frame_tag = tag;
    }

    /// The index the frames resolved from now on carry back with their
    /// times ([`GpuFrameTime::index`]): the host's number for the frame
    /// under way, said once a frame before
    /// [`resolve_into`](Self::resolve_into) or
    /// [`finish_frame`](Self::finish_frame), and kept until it says
    /// another. Apart from [`set_frame_tag`](Self::set_frame_tag), whose
    /// number is a trace's.
    pub fn set_frame_index(&mut self, index: Option<u64>) {
        self.frame_index = index;
    }

    /// The traced frames read back since the last take, oldest first: at
    /// most the last 16, so a host that takes once a frame misses none.
    pub fn take_traced(&mut self) -> Vec<TracedGpuFrame> {
        self.traced
            .as_mut()
            .map(|t| t.drain(..).collect())
            .unwrap_or_default()
    }

    /// The frames resolved since [`record`](Self::record) was turned on and
    /// read back so far, oldest first, each as measured; empty while it is
    /// off. [`flush`](Self::flush) brings in the last ones.
    pub fn recorded(&self) -> &[GpuTimings] {
        self.recorded.as_deref().unwrap_or(&[])
    }

    /// Wait for every read-back still on its way and fold it in, so the
    /// last frames drawn are counted. For a benchmark's end, never a frame
    /// loop: it blocks on the GPU.
    pub fn flush(&mut self, device: &wgpu::Device) {
        if self.readbacks.iter().any(|r| r.in_flight) {
            let _ = crate::wait_for_gpu(device);
        }
        self.poll(device);
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
        readback.record = self.recorded.is_some();
        readback.tally = self.tallied.is_some();
        readback.tag = self.frame_tag;
        readback.trace = self.traced.is_some();
        readback.index = self.frame_index;
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

    /// Every frame read back since the last take, in the order they came
    /// back, each with the index it was resolved under: a frame's own
    /// figure for the player's frame history (#1076, #1104). A read-back
    /// is a frame or two late, so each belongs to an earlier frame than the
    /// one taking it; none come on some frames and two on others. At most
    /// the last 16 wait, so a host that takes once a frame misses none.
    pub fn take_frame_times(&mut self) -> std::collections::vec_deque::Drain<'_, GpuFrameTime> {
        self.frame_times.drain(..)
    }

    /// The smoothed times of the frames read back so far.
    pub fn timings(&self) -> &GpuTimings {
        &self.timings
    }

    /// Whether those times can be read as pass times (#1316): a reader
    /// says [`TIMESTAMPS_UNAVAILABLE`] in place of a list of zeros when
    /// they can't.
    pub fn timestamps(&self) -> TimestampHealth {
        self.check.verdict()
    }

    /// Fold in every read-back that has come in. Polls without blocking.
    fn poll(&mut self, device: &wgpu::Device) {
        if !self.readbacks.iter().any(|r| r.in_flight) {
            return;
        }
        let _ = device.poll(wgpu::PollType::Poll);
        for readback in &mut self.readbacks {
            if !readback.in_flight {
                continue;
            }
            match readback.state.load(Ordering::Acquire) {
                1 => {
                    let bytes = readback.count as u64 * wgpu::QUERY_SIZE as u64;
                    let ticks: Vec<u64> = {
                        let data = readback
                            .buffer
                            .slice(..bytes)
                            .get_mapped_range()
                            .expect("mapped buffer range");
                        bytemuck::pod_collect_to_vec(&data)
                    };
                    readback.buffer.unmap();
                    readback.in_flight = false;
                    if let (Some(traced), true) = (&mut self.traced, readback.trace) {
                        if traced.len() >= TRACED_KEPT {
                            traced.pop_front();
                        }
                        traced.push_back(TracedGpuFrame {
                            tag: readback.tag,
                            passes: traced_times(&readback.labels, &ticks, self.period_ns),
                        });
                    }
                    let frame = frame_times(&readback.labels, &ticks, self.period_ns);
                    self.check.observe(&frame);
                    if self.frame_times.len() >= FRAME_TIMES_KEPT {
                        self.frame_times.pop_front();
                    }
                    self.frame_times.push_back(GpuFrameTime {
                        index: readback.index,
                        ms: frame.frame_ms,
                    });
                    if let (Some(recorded), true) = (&mut self.recorded, readback.record) {
                        recorded.push(frame.clone());
                    }
                    if let (Some(tallied), true) = (&mut self.tallied, readback.tally) {
                        if tallied.len() < TALLY_FRAMES {
                            tallied.push(frame.clone());
                        }
                    }
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

/// One frame's scopes laid from its first timestamp, as
/// [`frame_times`] reads them.
fn traced_times(labels: &[&'static str], ticks: &[u64], period_ns: f32) -> Vec<TracedPass> {
    let ms = |ticks: u64| (ticks as f64 * period_ns as f64 / 1e6) as f32;
    let pairs: Vec<(&'static str, u64, u64)> = labels
        .iter()
        .enumerate()
        .map_while(|(i, label)| Some((*label, *ticks.get(2 * i)?, *ticks.get(2 * i + 1)?)))
        .filter(|&(_, start, end)| start != 0 && end >= start)
        .collect();
    let Some(base) = pairs.iter().map(|&(_, start, _)| start).min() else {
        return Vec::new();
    };
    pairs
        .into_iter()
        .map(|(label, start, end)| TracedPass {
            label,
            start_ms: ms(start - base),
            ms: ms(end - start),
        })
        .collect()
}

/// One scope's times over many frames (#292), in milliseconds. A
/// percentile is the nearest rank: the time no more than that share of the
/// frames went over.
#[derive(Debug, Clone, PartialEq)]
pub struct PassSpread {
    pub label: &'static str,
    /// Which scope of that label in its frame: 0 for the first, 1 for a
    /// second scope named the same, and so on.
    pub nth: usize,
    pub min: f32,
    pub p50: f32,
    pub p90: f32,
    pub max: f32,
    pub mean: f32,
    /// Each frame's time, in the order the frames were drawn. A frame that
    /// didn't time the scope has no entry.
    pub samples: Vec<f32>,
}

impl PassSpread {
    /// Summarise `samples`; `None` when there are none.
    pub fn of(label: &'static str, nth: usize, samples: Vec<f32>) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f32::total_cmp);
        let rank =
            |p: f32| sorted[((p * sorted.len() as f32).ceil() as usize).clamp(1, sorted.len()) - 1];
        Some(Self {
            label,
            nth,
            min: sorted[0],
            p50: rank(0.5),
            p90: rank(0.9),
            max: sorted[sorted.len() - 1],
            mean: sorted.iter().sum::<f32>() / sorted.len() as f32,
            samples,
        })
    }

    /// The label, with ` (2)`, ` (3)`... on a second or later scope of it.
    pub fn name(&self) -> String {
        match self.nth {
            0 => self.label.to_string(),
            n => format!("{} ({})", self.label, n + 1),
        }
    }
}

impl PassSpread {
    /// The pass as JSON (#1093), the shape every Flint perf file shares
    /// (`flint render --gpu-time-json`, `flint-player --perf-report`):
    /// `label`, `frames`, `min_ms`, `p50_ms`, `p90_ms`, `max_ms`, `mean_ms`,
    /// and with `samples` each frame's time as `samples_ms`.
    pub fn to_json(&self, samples: bool) -> serde_json::Value {
        let mut v = serde_json::json!({
            "label": self.name(),
            "frames": self.samples.len(),
            "min_ms": self.min,
            "p50_ms": self.p50,
            "p90_ms": self.p90,
            "max_ms": self.max,
            "mean_ms": self.mean,
        });
        if samples {
            v["samples_ms"] = serde_json::json!(self.samples);
        }
        v
    }
}

/// Every scope's [`PassSpread`] over a run of frames, and the frame total's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuSpread {
    /// The frames summarised, after the dropped ones.
    pub frames: usize,
    /// Frames left out at the start, while caches and clocks warm up.
    pub dropped: usize,
    /// In the order the scopes first appeared.
    pub passes: Vec<PassSpread>,
    /// Each frame's total ([`GpuTimings::frame_ms`]); `None` with no frames.
    pub frame: Option<PassSpread>,
}

impl GpuSpread {
    /// The spread as JSON (#1093): `frames_kept`, `frames_dropped`,
    /// `passes` (each [`PassSpread::to_json`]) and `frame`, the total's.
    pub fn to_json(&self, samples: bool) -> serde_json::Value {
        serde_json::json!({
            "frames_kept": self.frames,
            "frames_dropped": self.dropped,
            "passes": self.passes.iter().map(|p| p.to_json(samples)).collect::<Vec<_>>(),
            "frame": self.frame.as_ref().map(|f| f.to_json(samples)),
        })
    }
}

/// The spread of `frames` (raw, from [`GpuTimer::recorded`]) after leaving
/// out the first `drop` of them.
pub fn spread(frames: &[GpuTimings], drop: usize) -> GpuSpread {
    let dropped = drop.min(frames.len());
    let kept = &frames[dropped..];
    // (label, nth) in first-seen order, with its samples
    let mut keys: Vec<(&'static str, usize, Vec<f32>)> = Vec::new();
    for frame in kept {
        let mut seen: Vec<&'static str> = Vec::new();
        for pass in &frame.passes {
            let nth = seen.iter().filter(|l| **l == pass.label).count();
            seen.push(pass.label);
            match keys
                .iter_mut()
                .find(|(l, n, _)| *l == pass.label && *n == nth)
            {
                Some((_, _, samples)) => samples.push(pass.ms),
                None => keys.push((pass.label, nth, vec![pass.ms])),
            }
        }
    }
    GpuSpread {
        frames: kept.len(),
        dropped,
        passes: keys
            .into_iter()
            .filter_map(|(label, nth, samples)| PassSpread::of(label, nth, samples))
            .collect(),
        frame: PassSpread::of("frame", 0, kept.iter().map(|f| f.frame_ms).collect()),
    }
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
    fn a_traced_frame_lays_each_scope_from_its_first_timestamp() {
        // b was never written (0): left out, as frame_times leaves it
        let ticks = [5_000_000, 7_000_000, 0, 0, 8_000_000, 9_500_000];
        let passes = traced_times(&["a", "b", "c"], &ticks, 1.0);
        assert_eq!(
            passes,
            [
                TracedPass {
                    label: "a",
                    start_ms: 0.0,
                    ms: 2.0
                },
                TracedPass {
                    label: "c",
                    start_ms: 3.0,
                    ms: 1.5
                }
            ]
        );
        assert!(traced_times(&["a"], &[0, 0], 1.0).is_empty());
    }

    #[test]
    fn unwritten_or_backward_scopes_are_left_out() {
        let ticks = [0, 5, 9, 7, 10, 30];
        let frame = frame_times(&["unwritten", "backward", "ok"], &ticks, 2.0);
        assert_eq!(frame.passes.len(), 1);
        assert_eq!(frame.passes[0].label, "ok");
        assert!((frame.passes[0].ms - 40e-6).abs() < 1e-9);
    }

    fn timed(passes: &[(&'static str, f32)]) -> GpuTimings {
        GpuTimings {
            passes: passes
                .iter()
                .map(|&(label, ms)| GpuPassTime { label, ms })
                .collect(),
            frame_ms: passes.iter().map(|p| p.1).sum(),
        }
    }

    #[test]
    fn a_spread_takes_nearest_rank_percentiles_after_the_dropped_frames() {
        // A slow first frame, then 1..=10 ms
        let mut frames = vec![timed(&[("a", 50.0)])];
        frames.extend((1..=10).map(|ms| timed(&[("a", ms as f32)])));
        let spread = spread(&frames, 1);
        assert_eq!((spread.frames, spread.dropped), (10, 1));
        let a = &spread.passes[0];
        assert_eq!((a.min, a.p50, a.p90, a.max), (1.0, 5.0, 9.0, 10.0));
        assert_eq!(a.mean, 5.5);
        assert_eq!(a.samples.len(), 10, "the warm-up frame isn't a sample");
        assert_eq!(spread.frame.as_ref().unwrap().max, 10.0);
    }

    #[test]
    fn one_sample_is_every_percentile() {
        let spread = spread(&[timed(&[("a", 2.0)])], 0);
        let a = &spread.passes[0];
        assert_eq!((a.min, a.p50, a.p90, a.max), (2.0, 2.0, 2.0, 2.0));
    }

    #[test]
    fn scopes_keep_their_order_a_missing_one_has_fewer_samples_and_a_repeat_its_own_row() {
        let frames = [
            timed(&[("a", 1.0), ("b", 2.0)]),
            timed(&[("a", 1.0), ("c", 3.0), ("a", 4.0)]),
        ];
        let spread = spread(&frames, 0);
        let rows: Vec<(String, usize)> = spread
            .passes
            .iter()
            .map(|p| (p.name(), p.samples.len()))
            .collect();
        assert_eq!(
            rows,
            [
                ("a".into(), 2),
                ("b".into(), 1),
                ("c".into(), 1),
                ("a (2)".into(), 1)
            ]
        );
    }

    #[test]
    fn dropping_every_frame_leaves_an_empty_spread() {
        let spread = spread(&[timed(&[("a", 1.0)])], 5);
        assert_eq!((spread.frames, spread.dropped), (0, 1));
        assert!(spread.passes.is_empty() && spread.frame.is_none());
    }

    /// The Pixel 8 Pro's frames (#1316): Shadows and Scene take time, every
    /// scope after them reads exactly zero ticks.
    fn mali_frame(scene_ms: f32) -> GpuTimings {
        timed(&[
            ("Shadows", 0.8),
            ("Scene", scene_ms),
            ("Bloom", 0.0),
            ("ASCII", 0.0),
            ("Composite", 0.0),
            ("World lines", 0.0),
            ("UI", 0.0),
        ])
    }

    fn checked(frames: impl IntoIterator<Item = GpuTimings>) -> TimestampHealth {
        let mut check = TimestampCheck::default();
        for frame in frames {
            check.observe(&frame);
        }
        check.verdict()
    }

    #[test]
    fn later_scopes_reading_zero_ticks_for_a_window_are_unusable() {
        let window = TIMESTAMP_WINDOW as usize;
        let mali = (0..window).map(|i| mali_frame(6.0 + i as f32 * 0.01));
        assert_eq!(
            checked(mali),
            TimestampHealth::Unusable { zero: 5, scopes: 7 }
        );
        let early = (0..window - 1).map(|_| mali_frame(6.0));
        assert_eq!(
            checked(early),
            TimestampHealth::Unchecked,
            "no verdict before a whole window"
        );
        let why = TimestampHealth::Unusable { zero: 5, scopes: 7 }.why();
        assert_eq!(
            why.as_deref(),
            Some("5 of 7 scopes read 0 ticks for 60 frames")
        );
        assert_eq!(TimestampHealth::Usable.why(), None);
    }

    #[test]
    fn real_times_are_usable_even_with_tiny_or_rounded_scopes() {
        let window = TIMESTAMP_WINDOW as usize;
        // The test is on raw ticks: only equal ticks read zero, one tick
        // apart does not
        let one_tick = frame_times(&["a", "b"], &[10, 10, 20, 21], 1.0);
        assert_eq!(one_tick.passes[0].ms, 0.0, "equal ticks read zero");
        assert!(one_tick.passes[1].ms > 0.0, "one tick reads above zero");
        // A desktop frame: an empty World lines pass is ~2 us
        let desktop = (0..window).map(|_| {
            timed(&[
                ("Shadows", 0.31),
                ("Scene", 1.92),
                ("Bloom", 0.12),
                ("Composite", 0.04),
                ("World lines", 0.002),
                ("UI", 0.05),
            ])
        });
        assert_eq!(checked(desktop), TimestampHealth::Usable);

        // A browser rounding to 100 us: the small scopes read 0 on most
        // frames, but not on every one of them, and an always-zero one
        // alone isn't most of the frame
        let rounded = (0..window).map(|i| {
            let step = |every: usize| if i % every == 0 { 0.1 } else { 0.0 };
            timed(&[
                ("Scene", 1.9),
                ("Bloom", step(4)),
                ("Composite", step(9)),
                ("World lines", 0.0),
                ("UI", step(3)),
            ])
        });
        assert_eq!(checked(rounded), TimestampHealth::Usable);

        // One non-zero read of the later scopes clears them for the window
        let mut frames: Vec<GpuTimings> = (0..window).map(|_| mali_frame(6.0)).collect();
        for p in &mut frames[30].passes[2..] {
            p.ms = 0.05;
        }
        assert_eq!(checked(frames), TimestampHealth::Usable);
    }

    #[test]
    fn each_window_judges_afresh() {
        let window = TIMESTAMP_WINDOW as usize;
        let mut check = TimestampCheck::default();
        for _ in 0..window {
            check.observe(&mali_frame(6.0));
        }
        assert!(check.verdict().unusable());
        // The scene changes and the passes read as times again
        for _ in 0..window {
            check.observe(&timed(&[("Scene", 2.0), ("Composite", 0.1), ("UI", 0.2)]));
        }
        assert_eq!(check.verdict(), TimestampHealth::Usable);
        // An untimed frame isn't counted
        check.observe(&GpuTimings::default());
        assert_eq!(check.frames, 0);
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
