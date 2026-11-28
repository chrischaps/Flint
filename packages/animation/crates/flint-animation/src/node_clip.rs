//! Node-level animation clip — per-node keyframe tracks for transform animation
//!
//! Mirrors the skeletal clip types but targets named scene-graph nodes instead of
//! skeleton joints. Used for glTF node animations where entire objects move/rotate
//! via their transforms (as opposed to vertex skinning).

use crate::clip::Interpolation;
use crate::skeletal_clip::{
    make_rotation_track_continuous, slide_to_first_key, JointKeyframe, JointProperty,
};

/// A single track targeting one node's property (translation, rotation, or scale)
#[derive(Debug, Clone)]
pub struct NodeTrack {
    pub node_name: String,
    pub property: JointProperty,
    pub interpolation: Interpolation,
    pub keyframes: Vec<JointKeyframe>,
}

/// A complete node-level animation clip with per-node tracks
#[derive(Debug, Clone)]
pub struct NodeClip {
    pub name: String,
    pub duration: f64,
    pub node_tracks: Vec<NodeTrack>,
}

impl NodeClip {
    /// Convert from imported glTF node animation data. The clip starts at its
    /// first key, as a skeletal clip does (`slide_to_first_key`, #872): one
    /// glTF animation can drive joints and plain nodes, and the two halves
    /// must stay in step.
    pub fn from_imported(imported: &flint_import::ImportedNodeClip) -> Self {
        let mut node_tracks: Vec<NodeTrack> = imported
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

                NodeTrack {
                    node_name: ch.node_name.clone(),
                    property,
                    interpolation,
                    keyframes,
                }
            })
            .collect();
        let duration = slide_to_first_key(
            node_tracks.iter_mut().map(|t| &mut t.keyframes),
            imported.duration as f64,
        );

        Self {
            name: imported.name.clone(),
            duration,
            node_tracks,
        }
    }
}
