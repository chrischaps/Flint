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
#[derive(Debug, Clone)]
pub struct JointKeyframe {
    pub time: f64,
    /// 3 floats for translation/scale, 4 for rotation (quaternion xyzw)
    pub value: Vec<f32>,
}

/// A single track targeting one joint's property (translation, rotation, or scale)
#[derive(Debug, Clone)]
pub struct JointTrack {
    pub joint_index: usize,
    pub property: JointProperty,
    pub interpolation: Interpolation,
    pub keyframes: Vec<JointKeyframe>,
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

                let keyframes = ch
                    .keyframes
                    .iter()
                    .map(|kf| JointKeyframe {
                        time: kf.time as f64,
                        value: kf.value.clone(),
                    })
                    .collect();

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
