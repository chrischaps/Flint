//! Samples every track of a real CUBICSPLINE glTF export and checks nothing collapses.
//!
//! Opt-in: set `FLINT_CUBIC_MODEL=/path/to/model.gltf`. Skipped otherwise so CI
//! doesn't depend on game assets.

use flint_animation::skeletal_clip::{JointProperty, SkeletalClip};
use flint_animation::skeletal_sampler::sample_joint_track;

#[test]
fn real_cubicspline_model_stays_sane() {
    let Ok(path) = std::env::var("FLINT_CUBIC_MODEL") else {
        eprintln!("FLINT_CUBIC_MODEL not set; skipping");
        return;
    };
    let imported = flint_import::import_gltf(&path).expect("import");
    assert!(!imported.skeletal_clips.is_empty(), "no skeletal clips");

    for ic in &imported.skeletal_clips {
        let clip = SkeletalClip::from_imported(ic);
        let mut cubic_tracks = 0;
        for track in &clip.joint_tracks {
            if track.interpolation == flint_animation::clip::Interpolation::CubicSpline {
                cubic_tracks += 1;
                for kf in &track.keyframes {
                    assert_eq!(
                        kf.in_tangent.len(),
                        kf.value.len(),
                        "{}: tangent arity",
                        ic.name
                    );
                }
            }
            let steps = 200;
            let mut prev_rot: Option<Vec<f32>> = None;
            for i in 0..=steps {
                let t = clip.duration * i as f64 / steps as f64;
                let v = sample_joint_track(track, t);
                if track.property == JointProperty::Rotation {
                    // No rotation should swing more than 45° in 1/200 of a clip —
                    // catches hemisphere flips that collapse the Hermite curve.
                    if let Some(p) = &prev_rot {
                        let dot: f32 = p.iter().zip(&v).map(|(a, b)| a * b).sum::<f32>().abs();
                        let deg = 2.0 * dot.min(1.0).acos().to_degrees();
                        assert!(
                            deg < 45.0,
                            "{} joint {} jumped {deg:.1}° at t={t:.3}",
                            ic.name,
                            track.joint_index
                        );
                    }
                    prev_rot = Some(v.clone());
                }
                match track.property {
                    JointProperty::Scale => {
                        for c in &v {
                            assert!(
                                (0.5..2.0).contains(c),
                                "{} joint {} scale {:?} at t={t}",
                                ic.name,
                                track.joint_index,
                                v
                            );
                        }
                    }
                    JointProperty::Rotation => {
                        let len: f32 = v.iter().map(|c| c * c).sum::<f32>().sqrt();
                        assert!(
                            (len - 1.0).abs() < 1e-3,
                            "{} quat len {len} at t={t}",
                            ic.name
                        );
                    }
                    JointProperty::Translation => {
                        for c in &v {
                            assert!(c.abs() < 10.0, "{} translation {:?} at t={t}", ic.name, v);
                        }
                    }
                }
            }
        }
        println!(
            "{}: {} tracks ({} cubic) sampled OK over {:.2}s",
            ic.name,
            clip.joint_tracks.len(),
            cubic_tracks,
            clip.duration
        );
    }
}
