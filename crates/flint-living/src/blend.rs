//! Each frame's blend: a prop's bones, its echoes', its tints and its
//! drift-only shadow, computed only where something moved.

use flint_core::EntityId;
use flint_wooze::{Cycle, WoozeSample};

use super::layers::LivingShadows;
use super::ring::PoseRing;
use super::{LivingPropId, LivingProps, TINT_UNSENT};
use flint_procgen::living::{
    boil_bones, LivingClock, LivingJitter, LivingPose, LivingRig, LivingSkeleton, Segment,
    WarpedTime, JITTER_CHANNELS,
};
use flint_procgen::EchoCopy;

/// Room for one prop's blend at a time, kept across props and frames so that
/// blending a grove allocates nothing.
#[derive(Debug, Default)]
pub struct BlendFrame {
    pub(super) times: Vec<WarpedTime>,
    pub(super) pose: Option<LivingPose>,
    pub(super) posed: Vec<Segment>,
    /// The bone matrices to write, from [`PropBlend::first_bone`].
    pub bones: Vec<[[f32; 4]; 4]>,
    /// The shadow's bone matrices, whole, if [`PropBlend::shadow_changed`].
    pub shadow_bones: Vec<[[f32; 4]; 4]>,
    /// Each leaf part whose tint has changed, with the tint.
    pub tints: Vec<(usize, [f32; 3])>,
}

/// What one prop needs uploaded this frame; see [`LivingProps::blend_prop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropBlend {
    /// The entity whose buffers these are.
    pub entity: EntityId,
    /// Where [`BlendFrame::bones`] goes; `None` if the bones haven't changed.
    pub first_bone: Option<usize>,
    /// Whether to write [`BlendFrame::shadow_bones`].
    pub shadow_changed: bool,
    /// How many echoes it draws now, if that may have changed since last frame.
    pub echo_count: Option<usize>,
}

/// What a prop's bones and materials last held, so that nothing moved means
/// nothing computed and nothing uploaded: a prop holding still is free.
#[derive(Debug, Clone, Default)]
pub(super) struct DrawnState {
    pub(super) bones: DrawnBones,
    pub(super) tints: Vec<i32>,
    pub(super) shadow: Option<(u64, f32)>,
}

/// What one bone buffer last held: a prop's, or one of its echoes'.
#[derive(Debug, Clone, Default)]
pub(super) struct DrawnBones {
    pub(super) times: Vec<WarpedTime>,
    pub(super) turns: [f32; JITTER_CHANNELS],
    /// How much of the poses' slide it showed: it changes with an echo's
    /// offset while the clock holds, and must be drawn when it does.
    pub(super) slide: f32,
    /// Where it was moved to (`[living] echo_places`): like the slide, it
    /// changes with an echo's place while the clock holds.
    pub(super) place: [f32; 2],
    /// Each part's cycle, by segment id. Holds what the part started it with, for
    /// this buffer alone: an echo reaches each beat at a moment of its own.
    pub(super) parts: Vec<Cycle>,
    /// The row time its drifting words were drawn at (#576).
    pub(super) row_time: f64,
    /// The segment bones it last wrote, kept only when its rig has drifting
    /// words: they move every frame, and each rides its segment's bone.
    pub(super) segments: Vec<[[f32; 4]; 4]>,
}

/// One faint copy of a prop, and what its own bones last held.
#[derive(Debug, Clone)]
pub(super) struct PropEcho {
    pub(super) copy: EchoCopy,
    pub(super) drawn: DrawnBones,
}

impl LivingProps {
    /// One prop's blend this frame: fills `frame` with what has changed and
    /// says where it goes. Call after [`tick`](Self::tick) has moved the clock.
    ///
    /// A part reads its own place on the warped clock, and the prop's jitter
    /// bones and leaf tints read theirs. If none of that has moved, as when
    /// the game is paused or every part is holding, this computes no bones and
    /// asks for no upload.
    ///
    /// A pose the clock needs that isn't ready leaves the prop as it was, and
    /// [`LivingStats::blends_starved`] counts it. It can only happen if the
    /// budget has run behind or poses are still coming in under `--procgen-defer`.
    ///
    /// Each part reads the field's glide and amplitude when its own cycle begins,
    /// through its [`Cycle`], so a change reaches a staggered tree a part at a
    /// time and nothing has to be redrawn until it does: a prop holding through a
    /// change costs no more than one holding through nothing.
    pub fn blend_prop(&mut self, id: LivingPropId, frame: &mut BlendFrame) -> PropBlend {
        let prop = &mut self.props[id.0];
        let mut blend = PropBlend {
            entity: prop.entity,
            first_bone: None,
            shadow_changed: false,
            echo_count: std::mem::take(&mut prop.echoes_changed).then_some(prop.echoes.len()),
        };
        frame.tints.clear();
        let (Some(ring), Some(built)) = (&prop.ring, &self.lineages[prop.lineage].built) else {
            return blend;
        };
        let (skeleton, rig) = (&built.skeleton, &built.rig);
        let t = prop.jitter_now();
        let jitter = prop.jitter.scaled(prop.wooze.jitter);

        let moved = blend_bones(
            &prop.clock,
            &jitter,
            prop.stream,
            ring,
            skeleton,
            rig,
            t,
            prop.row_time,
            prop.beats,
            &prop.wooze,
            flint_procgen::slide_shown(prop.echo_slide, 0.0),
            [0.0; 2],
            &mut prop.drawn.bones,
            frame,
        );
        blend.first_bone = moved.first_bone;
        self.stats.blends_starved += u64::from(moved.starved);

        // Each part's own beat, and what's new goes up as a material change. A
        // part's first blend always sends its tint, so nothing is assumed of
        // what the renderer holds: after an A/B flip (#238) it's the other side's.
        prop.drawn.tints.resize(rig.leaf_parts.len(), TINT_UNSENT);
        for (i, part) in rig.leaf_parts.iter().enumerate() {
            let step = jitter.tint_step(prop.stream, i, t);
            if step != prop.drawn.tints[i] {
                prop.drawn.tints[i] = step;
                frame.tints.push((*part, prop.jitter.tint(step)));
            }
        }

        if self.defaults.shadows == LivingShadows::Drift {
            let slow =
                prop.shadow_cycle
                    .read(prop.beats / 2.0, prop.wooze.glide, prop.wooze.amplitude);
            let (from, to) = LivingClock::slow_pair(slow.index);
            let weight = slow.weight;
            let shown = prop.shadow_cycle.shown(weight);
            if prop.drawn.shadow != Some((from, weight)) {
                let pose = frame.pose.get_or_insert_with(|| LivingPose {
                    shape_seed: 0,
                    segments: Vec::new(),
                });
                frame.times.clear();
                frame
                    .times
                    .resize(skeleton.segments.len(), WarpedTime { index: 0, weight });
                let found = pose.blend_from(&frame.times, |index| {
                    let wanted = if index == 0 { from } else { to };
                    ring.get(wanted).map(|p| &**p)
                });
                if found {
                    let slide = flint_procgen::slide_shown(prop.echo_slide, 0.0);
                    for segment in &mut pose.segments {
                        *segment = segment.pulled(shown);
                        segment.slide = segment.slide.map(|x| x * slide);
                    }
                }
                if found
                    && rig
                        .palette_into(skeleton, pose, &mut frame.posed, &mut frame.shadow_bones)
                        .is_ok()
                {
                    // Held still: no boil, and every word where it was written.
                    rig.still_bones_into(&mut frame.shadow_bones);
                    prop.drawn.shadow = Some((from, weight));
                    blend.shadow_changed = true;
                }
            }
        }
        blend
    }

    /// One echo's blend this frame: fills `frame.bones` if its bones have moved
    /// and returns where they go. It's the prop's own blend, at its own place
    /// on the clock, so a leaf card comes along with its bones and turns on the
    /// echo's beat. Its tints are the prop's.
    ///
    /// An echo has no rest pose to fall back on: its bones are written whole the
    /// first time, or not at all.
    pub fn blend_echo(
        &mut self,
        id: LivingPropId,
        echo: usize,
        frame: &mut BlendFrame,
    ) -> Option<usize> {
        let prop = &mut self.props[id.0];
        let (Some(ring), Some(built)) = (&prop.ring, &self.lineages[prop.lineage].built) else {
            return None;
        };
        let t = prop.echo_jitter_time(echo, prop.jitter_now());
        let rows = prop.echo_row_time(echo, prop.row_time);
        let beats = prop.echo_beats(echo);
        let jitter = prop.jitter.scaled(prop.wooze.jitter);
        let copy = prop.echoes[echo].copy;
        let slide = copy.slide_shown(prop.echo_slide);
        let drawn = &mut prop.echoes[echo].drawn;
        let never_posed = drawn.times.is_empty();
        let moved = blend_bones(
            &prop.clock,
            &jitter,
            prop.stream,
            ring,
            &built.skeleton,
            &built.rig,
            t,
            rows,
            beats,
            &prop.wooze,
            slide,
            copy.place,
            drawn,
            frame,
        );
        self.stats.blends_starved += u64::from(moved.starved);
        moved.first_bone.filter(|first| *first == 0 || !never_posed)
    }

    /// Note one frame's blending: how long it took and how many buffer writes.
    pub fn record_blend(&mut self, spent_ms: f64, writes: u64) {
        let stats = &mut self.stats;
        stats.blend_frames += 1;
        stats.blend_total_ms += spent_ms;
        stats.blend_worst_ms = stats.blend_worst_ms.max(spent_ms);
        stats.blend_frames_over_1ms += u64::from(spent_ms > 1.0);
        stats.bone_writes += writes;
    }
}

/// What [`blend_bones`] did.
pub(super) struct MovedBones {
    /// Where [`BlendFrame::bones`] goes; `None` if the bones haven't changed.
    pub(super) first_bone: Option<usize>,
    /// Whether a pose it needed wasn't ready, which left the bones as they were.
    pub(super) starved: bool,
}

/// One bone buffer's blend, a prop's or an echo's: every part reads its own place
/// on the warped clock at `beats`, the jitter reads `t` in seconds, and drifting
/// words (#576) `row_time`. If none of that has moved, this computes nothing.
#[allow(clippy::too_many_arguments)]
pub(super) fn blend_bones(
    clock: &LivingClock,
    jitter: &LivingJitter,
    stream: u64,
    ring: &PoseRing,
    skeleton: &LivingSkeleton,
    rig: &LivingRig,
    t: f64,
    row_time: f64,
    beats: f64,
    wooze: &WoozeSample,
    slide: f32,
    place: [f32; 2],
    drawn: &mut DrawnBones,
    frame: &mut BlendFrame,
) -> MovedBones {
    let mut moved = MovedBones {
        first_bone: None,
        starved: false,
    };
    let mut turns = [0.0; JITTER_CHANNELS];
    for (channel, turn) in turns.iter_mut().enumerate() {
        *turn = jitter.card_turn(stream, channel, t);
    }
    drawn
        .parts
        .resize(skeleton.segments.len(), Cycle::default());
    frame.times.clear();
    for (segment, part) in skeleton.segments.iter().zip(&mut drawn.parts) {
        let now = part.read(beats + clock.phase(segment), wooze.glide, wooze.amplitude);
        frame.times.push(WarpedTime {
            index: now.index,
            weight: now.weight,
        });
    }
    let turned = turns != drawn.turns;
    let rows = rig.rows.is_some();
    let rowed = rows && row_time != drawn.row_time;
    let drifted = frame.times != drawn.times || slide != drawn.slide || place != drawn.place;

    frame.bones.clear();
    if drifted {
        let pose = frame.pose.get_or_insert_with(|| LivingPose {
            shape_seed: 0,
            segments: Vec::new(),
        });
        let found = pose.blend_from(&frame.times, |index| ring.get(index).map(|p| &**p));
        if found {
            for ((segment, part), now) in
                pose.segments.iter_mut().zip(&drawn.parts).zip(&frame.times)
            {
                let shown = part.shown(now.weight);
                if shown < 1.0 {
                    *segment = segment.pulled(shown);
                }
            }
            skeleton.show_at(pose, slide, place);
        }
        let posed = found
            && rig
                .palette_into(skeleton, pose, &mut frame.posed, &mut frame.bones)
                .is_ok();
        if posed {
            std::mem::swap(&mut drawn.times, &mut frame.times);
            drawn.slide = slide;
            drawn.place = place;
            if rows {
                drawn.segments.clear();
                drawn.segments.extend_from_slice(&frame.bones);
            }
            rig.fast_bones_into(&turns, row_time, &mut frame.bones);
            drawn.turns = turns;
            drawn.row_time = row_time;
            moved.first_bone = Some(0);
        } else {
            frame.bones.clear();
            moved.starved = true;
        }
    }
    if moved.first_bone.is_none() && rows && (turned || rowed) && !drawn.segments.is_empty() {
        // Each word rides its segment's bone, so the whole buffer goes up: a
        // few segments more than the words alone.
        frame.bones.extend_from_slice(&drawn.segments);
        rig.fast_bones_into(&turns, row_time, &mut frame.bones);
        drawn.turns = turns;
        drawn.row_time = row_time;
        moved.first_bone = Some(0);
    } else if moved.first_bone.is_none() && turned && !rows {
        boil_bones(&turns, &mut frame.bones);
        drawn.turns = turns;
        moved.first_bone = Some(rig.bone_segments.len());
    }
    moved
}
