//! Skeletal animation clip — per-joint keyframe tracks with quaternion rotation

use crate::clip::Interpolation;

/// Which joint property a track animates
#[derive(Debug, Clone, PartialEq)]
pub enum JointProperty {
    Translation,
    Rotation,
    Scale,
}

/// A keyframe for a single joint property
#[derive(Debug, Clone, Default)]
pub struct JointKeyframe {
    pub time: f64,
    /// 3 floats for translation/scale, 4 for rotation (quaternion xyzw)
    pub value: Vec<f32>,
    /// Incoming tangent for cubic-spline tracks (empty for step/linear)
    pub in_tangent: Vec<f32>,
    /// Outgoing tangent for cubic-spline tracks (empty for step/linear)
    pub out_tangent: Vec<f32>,
}

/// A single track targeting one joint's property (translation, rotation, or scale)
#[derive(Debug, Clone)]
pub struct JointTrack {
    pub joint_index: usize,
    pub property: JointProperty,
    pub interpolation: Interpolation,
    pub keyframes: Vec<JointKeyframe>,
}

/// Make a rotation track's quaternions hemisphere-continuous.
///
/// `q` and `-q` are the same rotation, but interpolating between them passes
/// through zero. Slerp guards against this per-sample; cubic Hermite does not,
/// so a sign flip between adjacent keys collapses the curve mid-span and the
/// renormalised result snaps to a wrong pose (Blender's glTF exporter emits
/// such flips on large joint rotations, e.g. thighs in a walk cycle). Negating
/// the key — value and both tangents — when its dot with the previous key is
/// negative makes the path continuous without changing any keyed rotation.
pub fn make_rotation_track_continuous(keyframes: &mut [JointKeyframe]) {
    for i in 1..keyframes.len() {
        let (prev, cur) = keyframes.split_at_mut(i);
        let prev = &prev[i - 1];
        let cur = &mut cur[0];
        if prev.value.len() < 4 || cur.value.len() < 4 {
            continue;
        }
        let dot: f32 = prev.value.iter().zip(&cur.value).map(|(a, b)| a * b).sum();
        if dot < 0.0 {
            for c in cur.value.iter_mut() {
                *c = -*c;
            }
            for c in cur.in_tangent.iter_mut() {
                *c = -*c;
            }
            for c in cur.out_tangent.iter_mut() {
                *c = -*c;
            }
        }
    }
}

/// Slide a clip's keys so its earliest one is at 0, and return the clip's
/// length from there (`end` is its last key's time as the file wrote it).
///
/// A glTF clip's keys need not start at 0. Blender writes frame N at N / fps,
/// so an action on frames 1..250 (every Mixamo clip through Blender) has its
/// first key at 1/30 s. The sampler clamps before the first key, so playback
/// from 0 held the first pose for that long (#872): a `--frame-step` capture
/// drew the same pose on frames 0 and 1 while physics moved, and every loop
/// paused on its seam. A clip plays from its first key instead, as Blender's
/// "slide to zero" export would have written it. Every track moves by the same
/// amount, so they stay in step with each other. A clip already at 0 is
/// untouched.
pub(crate) fn slide_to_first_key<'a>(
    tracks: impl IntoIterator<Item = &'a mut Vec<JointKeyframe>>,
    end: f64,
) -> f64 {
    let mut tracks: Vec<&mut Vec<JointKeyframe>> = tracks.into_iter().collect();
    let start = tracks
        .iter()
        .flat_map(|keys| keys.first())
        .map(|key| key.time)
        .fold(f64::INFINITY, f64::min);
    if !(start.is_finite() && start > 0.0) {
        return end;
    }
    for keys in &mut tracks {
        for key in keys.iter_mut() {
            key.time -= start;
        }
    }
    (end - start).max(0.0)
}

/// A complete skeletal animation clip with per-joint tracks
#[derive(Debug, Clone)]
pub struct SkeletalClip {
    pub name: String,
    pub duration: f64,
    pub joint_tracks: Vec<JointTrack>,
}

impl SkeletalClip {
    /// Convert from imported glTF data. The clip starts at its first key
    /// (`slide_to_first_key`), so its `duration` is from there to its last.
    pub fn from_imported(imported: &flint_import::ImportedSkeletalClip) -> Self {
        let mut joint_tracks: Vec<JointTrack> = imported
            .channels
            .iter()
            .map(|ch| {
                let property = match ch.property {
                    flint_import::JointProperty::Translation => JointProperty::Translation,
                    flint_import::JointProperty::Rotation => JointProperty::Rotation,
                    flint_import::JointProperty::Scale => JointProperty::Scale,
                };

                let interpolation = match ch.interpolation.as_str() {
                    "STEP" => Interpolation::Step,
                    "CUBICSPLINE" => Interpolation::CubicSpline,
                    _ => Interpolation::Linear,
                };

                let mut keyframes: Vec<JointKeyframe> = ch
                    .keyframes
                    .iter()
                    .map(|kf| JointKeyframe {
                        time: kf.time as f64,
                        value: kf.value.clone(),
                        in_tangent: kf.in_tangent.clone(),
                        out_tangent: kf.out_tangent.clone(),
                    })
                    .collect();
                if property == JointProperty::Rotation {
                    make_rotation_track_continuous(&mut keyframes);
                }

                JointTrack {
                    joint_index: ch.joint_index,
                    property,
                    interpolation,
                    keyframes,
                }
            })
            .collect();
        let duration = slide_to_first_key(
            joint_tracks.iter_mut().map(|t| &mut t.keyframes),
            imported.duration as f64,
        );

        Self {
            name: imported.name.clone(),
            duration,
            joint_tracks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeletal_sampler::sample_joint_track;

    /// A clip as Blender writes an action on frames 1..4 at 30 fps: the
    /// first key at 1/30 s, not 0.
    fn from_frame_one() -> flint_import::ImportedSkeletalClip {
        let key = |frame: f32, x: f32| flint_import::ImportedKeyframe {
            time: frame / 30.0,
            value: vec![x, 0.0, 0.0],
            in_tangent: Vec::new(),
            out_tangent: Vec::new(),
        };
        flint_import::ImportedSkeletalClip {
            name: "walk".into(),
            duration: 4.0 / 30.0,
            channels: vec![
                flint_import::ImportedChannel {
                    joint_index: 0,
                    property: flint_import::JointProperty::Translation,
                    interpolation: "LINEAR".into(),
                    keyframes: vec![key(1.0, 0.0), key(2.0, 1.0), key(4.0, 3.0)],
                },
                // A second track that starts a frame later keeps its place
                flint_import::ImportedChannel {
                    joint_index: 1,
                    property: flint_import::JointProperty::Translation,
                    interpolation: "LINEAR".into(),
                    keyframes: vec![key(2.0, 5.0), key(4.0, 7.0)],
                },
            ],
        }
    }

    #[test]
    fn a_clip_starts_at_its_first_key() {
        let clip = SkeletalClip::from_imported(&from_frame_one());
        let first: Vec<f64> = clip
            .joint_tracks
            .iter()
            .map(|t| t.keyframes[0].time)
            .collect();
        assert_eq!(first[0], 0.0);
        assert!((first[1] - 1.0 / 30.0).abs() < 1e-6, "{first:?}");
        assert!(
            (clip.duration - 3.0 / 30.0).abs() < 1e-6,
            "{}",
            clip.duration
        );
    }

    /// #872: the pose one 1/48 s step in is not the pose at 0. Before the
    /// slide, both sampled before the first key and drew the same pose.
    #[test]
    fn the_first_step_moves() {
        let clip = SkeletalClip::from_imported(&from_frame_one());
        let track = &clip.joint_tracks[0];
        let at_zero = sample_joint_track(track, 0.0);
        let one_step = sample_joint_track(track, 1.0 / 48.0);
        assert_eq!(at_zero[0], 0.0);
        assert!(one_step[0] > 0.5, "{one_step:?}");
    }

    #[test]
    fn a_clip_already_at_zero_is_untouched() {
        let mut imported = from_frame_one();
        for ch in &mut imported.channels {
            for kf in &mut ch.keyframes {
                kf.time -= 1.0 / 30.0;
            }
        }
        imported.duration = 3.0 / 30.0;
        let clip = SkeletalClip::from_imported(&imported);
        for (ch, track) in imported.channels.iter().zip(&clip.joint_tracks) {
            for (a, b) in ch.keyframes.iter().zip(&track.keyframes) {
                assert_eq!(a.time as f64, b.time);
            }
        }
        assert_eq!(clip.duration, imported.duration as f64);
    }
}
