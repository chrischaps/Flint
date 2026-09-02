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

/// A complete skeletal animation clip with per-joint tracks
#[derive(Debug, Clone)]
pub struct SkeletalClip {
    pub name: String,
    pub duration: f64,
    pub joint_tracks: Vec<JointTrack>,
}

impl SkeletalClip {
    /// Convert from imported glTF data
    pub fn from_imported(imported: &flint_import::ImportedSkeletalClip) -> Self {
        let joint_tracks = imported
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

        Self {
            name: imported.name.clone(),
            duration: imported.duration as f64,
            joint_tracks,
        }
    }
}
