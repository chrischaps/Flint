use super::transform_aabb;

fn translate_scale(t: [f32; 3], s: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [s[0], 0.0, 0.0, 0.0],
        [0.0, s[1], 0.0, 0.0],
        [0.0, 0.0, s[2], 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

#[test]
fn aabb_follows_translation_and_scale() {
    let m = translate_scale([10.0, 0.0, -5.0], [2.0, 1.0, -3.0]);
    let (min, max) = transform_aabb(&m, [-1.0, 0.0, 0.0], [1.0, 1.0, 2.0]);
    assert_eq!(min, [8.0, 0.0, -11.0]);
    assert_eq!(max, [12.0, 1.0, -5.0]);
}

#[test]
fn aabb_encloses_a_rotated_box() {
    // 45 degrees about Y: a unit box's corners reach sqrt(2) on X and Z.
    let (s, c) = std::f32::consts::FRAC_PI_4.sin_cos();
    let m = [
        [c, 0.0, -s, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [s, 0.0, c, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let (min, max) = transform_aabb(&m, [-1.0; 3], [1.0; 3]);
    let r = 2.0_f32.sqrt();
    for (got, want) in min.iter().zip([-r, -1.0, -r]) {
        assert!((got - want).abs() < 1e-5, "{min:?}");
    }
    for (got, want) in max.iter().zip([r, 1.0, r]) {
        assert!((got - want).abs() < 1e-5, "{max:?}");
    }
}

/// A sheared tile (#625): its culling bounds hold every corner the
/// matrix moves, and its normals, through the inverse transpose, stay
/// perpendicular to the sheared surface rather than leaning with it.
#[test]
fn a_sheared_entity_is_bounded_and_lit_by_its_own_matrix() {
    use super::helpers::mat4_inv_transpose;
    use flint_core::{Transform, Vec3};
    // An 8 m tile, 20 m wide, leaning 0.5 m sideways per metre along z
    // and climbing 0.25 m per metre along z, then yawed 30 degrees.
    let t = Transform::default()
        .with_position(Vec3::new(3.0, 0.0, 1.0))
        .with_rotation(Vec3::new(0.0, 30.0, 0.0))
        .with_shear([0.0, 0.5, 0.0, 0.25, 0.0, 0.0]);
    let m = t.to_matrix();
    let apply = |p: [f32; 3]| {
        let mut out = [m[3][0], m[3][1], m[3][2]];
        for (row, o) in out.iter_mut().enumerate() {
            for (col, v) in p.iter().enumerate() {
                *o += m[col][row] * v;
            }
        }
        out
    };
    let (lo, hi) = ([-10.0, 0.0, -4.0], [10.0, 0.1, 4.0]);
    let (min, max) = transform_aabb(&m, lo, hi);
    for corner in 0..8 {
        let p = [
            if corner & 1 == 0 { lo[0] } else { hi[0] },
            if corner & 2 == 0 { lo[1] } else { hi[1] },
            if corner & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let w = apply(p);
        for axis in 0..3 {
            assert!(w[axis] >= min[axis] - 1e-4 && w[axis] <= max[axis] + 1e-4);
        }
    }

    // Two edges of the tile's top face, in world space.
    let o = apply([0.0, 0.0, 0.0]);
    let along_x = apply([1.0, 0.0, 0.0]);
    let along_z = apply([0.0, 0.0, 1.0]);
    let ex: Vec<f32> = (0..3).map(|i| along_x[i] - o[i]).collect();
    let ez: Vec<f32> = (0..3).map(|i| along_z[i] - o[i]).collect();
    let it = mat4_inv_transpose(&m);
    let n: Vec<f32> = (0..3).map(|row| it[1][row]).collect(); // local +Y
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    assert!(dot(&n, &ex).abs() < 1e-5, "normal leans along x");
    assert!(dot(&n, &ez).abs() < 1e-5, "normal leans along z");
    // The plain model matrix would have tilted it with the shear.
    let naive: Vec<f32> = (0..3).map(|row| m[1][row]).collect();
    assert!(dot(&naive, &ez).abs() > 0.1);
}

/// A render-scale step keeps the surface's size, so it keeps the
/// display-stage style targets (ASCII's cell history, #277) and the
/// history in them (#270); a real resize still drops them.
#[test]
fn a_render_scale_step_keeps_the_style_targets() {
    let Ok(ctx) = pollster::block_on(crate::HeadlessContext::new(64, 64)) else {
        return; // no adapter here
    };
    let mut renderer = super::SceneRenderer::new_headless(
        &ctx.device,
        &ctx.queue,
        ctx.format,
        64,
        64,
        super::RendererConfig {
            show_grid: false,
            sample_count: 1,
        },
    );
    let probe = |r: &super::SceneRenderer| {
        r.postprocess_resources.as_ref().unwrap().styles.get(
            &ctx.device,
            "probe",
            wgpu::TextureFormat::Rgba16Uint,
            8,
            8,
        )
    };
    let before = probe(&renderer);
    renderer.set_render_scale(&ctx.device, 0.5);
    assert_eq!(renderer.scene_size(), (32, 32));
    assert!(std::sync::Arc::ptr_eq(&before, &probe(&renderer)));
    renderer.set_render_scale(&ctx.device, 1.0);
    assert!(std::sync::Arc::ptr_eq(&before, &probe(&renderer)));
    renderer.resize_postprocess(&ctx.device, 128, 64);
    assert!(!std::sync::Arc::ptr_eq(&before, &probe(&renderer)));
}
