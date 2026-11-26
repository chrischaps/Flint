//! The texture ops' tests, moved whole from the old `ops.rs` (#1026).

use super::*;
use crate::algorithms::noise::{MusgraveType, VoronoiFeature, VoronoiMetric};
use crate::SeededRng;

use super::cells::cell_noise_offset;
use crate::generators::texture::field::TextureField;

#[test]
fn brick_grid_creates_cells() {
    let mut field = TextureField::new(64, 64);
    let mut rng = SeededRng::new(42);
    let op = BrickGridOp {
        columns: 4,
        rows: 8,
        stagger: 0.5,
        gap_width: 0.04,
        width_variation: 0.0,
        row_height_variation: 0.0,
        warp_x: None,
        warp_y: None,
        warp_strength: 0.0,
    };
    op.apply(&mut field, &mut rng);

    // Should have cell_id and edge_dist channels
    assert!(field.has_channel("cell_id"));
    assert!(field.has_channel("edge_dist"));

    // Expected unique cell count: 4 * 8 = 32
    let mut ids = std::collections::HashSet::new();
    for y in 0..64 {
        for x in 0..64 {
            ids.insert(field.get("cell_id", x, y) as u32);
        }
    }
    assert_eq!(ids.len(), 32, "expected 32 unique cells, got {}", ids.len());
}

#[test]
fn brick_grid_edge_dist_range() {
    let mut field = TextureField::new(128, 128);
    let mut rng = SeededRng::new(42);
    let op = BrickGridOp {
        columns: 8,
        rows: 16,
        stagger: 0.5,
        gap_width: 0.04,
        width_variation: 0.0,
        row_height_variation: 0.0,
        warp_x: None,
        warp_y: None,
        warp_strength: 0.0,
    };
    op.apply(&mut field, &mut rng);

    for y in 0..128 {
        for x in 0..128 {
            let e = field.get("edge_dist", x, y);
            assert!(e >= 0.0, "edge_dist negative at ({x},{y}): {e}");
            assert!(e <= 0.5, "edge_dist > 0.5 at ({x},{y}): {e}");
        }
    }
}

#[test]
fn brick_grid_stagger_offset() {
    let mut field = TextureField::new(128, 128);
    let mut rng = SeededRng::new(42);
    let op = BrickGridOp {
        columns: 4,
        rows: 4,
        stagger: 0.5,
        gap_width: 0.04,
        width_variation: 0.0,
        row_height_variation: 0.0,
        warp_x: None,
        warp_y: None,
        warp_strength: 0.0,
    };
    op.apply(&mut field, &mut rng);

    // Row 0 and row 1 should have different column offsets (additive stagger)
    let row0_mid = field.get("cell_id", 16, 8) as u32; // middle of first cell in row 0
    let row1_mid = field.get("cell_id", 16, 40) as u32; // same x, row 1
                                                        // They should be in different cells due to stagger
    assert_ne!(
        row0_mid % 4,
        row1_mid % 4,
        "stagger should offset rows progressively"
    );
}

#[test]
fn cell_height_writes_height() {
    let mut field = TextureField::new(32, 32);
    // Pre-populate cell_id
    for y in 0..32 {
        for x in 0..32 {
            field.set("cell_id", x, y, ((y / 16) * 2 + x / 16) as f32);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = CellHeightOp { variation: 0.5 };
    op.apply(&mut field, &mut rng);

    assert!(field.has_channel("height"));
    for y in 0..32 {
        for x in 0..32 {
            let h = field.get("height", x, y);
            assert!(
                h >= 0.0 && h <= 1.0,
                "height out of range at ({x},{y}): {h}"
            );
        }
    }
}

#[test]
fn mortar_groove_lowers_height() {
    let mut field = TextureField::new(32, 32);
    // Set uniform height and varying edge_dist
    for y in 0..32 {
        for x in 0..32 {
            field.set("height", x, y, 0.5);
            let e = x as f32 / 32.0 * 0.5; // 0 at left, 0.5 at right
            field.set("edge_dist", x, y, e);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = MortarGrooveOp {
        depth: 0.2,
        width: 0.1,
    };
    op.apply(&mut field, &mut rng);

    // Left edge (edge_dist ≈ 0) should have lowered height
    let h_edge = field.get("height", 0, 0);
    assert!(
        h_edge < 0.5,
        "mortar groove should lower height at edge: {h_edge}"
    );

    // Right side (edge_dist > width) should be unchanged
    let h_interior = field.get("height", 31, 0);
    assert!(
        (h_interior - 0.5).abs() < 1e-5,
        "interior height should be unchanged: {h_interior}"
    );
}

#[test]
fn cell_color_preserves_hue() {
    let mut field = TextureField::new(32, 32);
    for y in 0..32 {
        for x in 0..32 {
            field.set("cell_id", x, y, (y * 32 + x) as f32);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = CellColorOp {
        base_color: [0.47, 0.28, 0.14, 1.0], // brown
        variation: 0.12,
    };
    op.apply(&mut field, &mut rng);

    // All colors should stay in valid range
    for y in 0..32 {
        for x in 0..32 {
            let r = field.get("r", x, y);
            let g = field.get("g", x, y);
            let b = field.get("b", x, y);
            assert!(r >= 0.0 && r <= 1.0);
            assert!(g >= 0.0 && g <= 1.0);
            assert!(b >= 0.0 && b <= 1.0);
        }
    }
}

#[test]
fn mortar_color_blends_near_edge() {
    let mut field = TextureField::new(32, 32);
    for y in 0..32 {
        for x in 0..32 {
            field.set("r", x, y, 0.5);
            field.set("g", x, y, 0.3);
            field.set("b", x, y, 0.1);
            field.set("edge_dist", x, y, x as f32 / 32.0 * 0.5);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = MortarColorOp {
        color: [0.2, 0.2, 0.2, 1.0],
        threshold: 0.1,
    };
    op.apply(&mut field, &mut rng);

    // Left edge (edge_dist ≈ 0) should be close to mortar color
    let r = field.get("r", 0, 0);
    assert!(
        (r - 0.2).abs() < 0.1,
        "edge should be near mortar color, got r={r}"
    );

    // Right side (edge_dist > threshold) should be unchanged
    let r = field.get("r", 31, 0);
    assert!(
        (r - 0.5).abs() < 1e-5,
        "interior should be unchanged, got r={r}"
    );
}

#[test]
fn derive_normal_z_positive() {
    let mut field = TextureField::new(32, 32);
    // Flat surface
    for y in 0..32 {
        for x in 0..32 {
            field.set("height", x, y, 0.5);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = DeriveNormalOp { strength: 1.5 };
    op.apply(&mut field, &mut rng);

    for y in 0..32 {
        for x in 0..32 {
            let nz = field.get("normal_z", x, y);
            assert!(nz > 0.0, "normal Z should be positive at ({x},{y}): {nz}");
        }
    }
}

#[test]
fn cell_roughness_range() {
    let mut field = TextureField::new(32, 32);
    for y in 0..32 {
        for x in 0..32 {
            field.set("cell_id", x, y, (x / 8) as f32);
            field.set("edge_dist", x, y, (y as f32 / 32.0) * 0.5);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = CellRoughnessOp {
        base: 0.75,
        variation: 0.05,
        mortar: 0.55,
        mortar_threshold: 0.06,
    };
    op.apply(&mut field, &mut rng);

    for y in 0..32 {
        for x in 0..32 {
            let r = field.get("roughness", x, y);
            assert!(
                r >= 0.0 && r <= 1.0,
                "roughness out of range at ({x},{y}): {r}"
            );
        }
    }
}

#[test]
fn noise_layer_writes_output_channel() {
    let mut field = TextureField::new(32, 32);
    let mut rng = SeededRng::new(42);
    let op = NoiseLayerOp {
        output: "my_signal".into(),
        frequency: 10.0,
        octaves: 4,
        noise_type: NoiseType::Perlin,
        scale_x: 1.0,
        scale_y: 1.0,
        cell_offset: false,
    };
    op.apply(&mut field, &mut rng);

    assert!(field.has_channel("my_signal"));
    // Values should be in 0–1 range with variation
    let mut any_varied = false;
    for y in 0..32 {
        for x in 0..32 {
            let v = field.get("my_signal", x, y);
            assert!((0.0..=1.0).contains(&v), "output out of range: {v}");
            if (v - 0.5).abs() > 0.01 {
                any_varied = true;
            }
        }
    }
    assert!(any_varied, "output channel should have variation");
}

#[test]
fn blend_add_modifies_target() {
    let mut field = TextureField::new(32, 32);
    for y in 0..32 {
        for x in 0..32 {
            field.set("height", x, y, 0.5);
            field.set("noise", x, y, 0.7); // above 0.5 center
        }
    }
    let mut rng = SeededRng::new(42);
    let op = BlendOp {
        source: "noise".into(),
        target: "height".into(),
        mode: BlendMode::Add,
        strength: 0.2,
        mask: None,
    };
    op.apply(&mut field, &mut rng);

    // height should have increased: 0.5 + (0.7 - 0.5) * 0.2 = 0.54
    let h = field.get("height", 0, 0);
    assert!((h - 0.54).abs() < 1e-4, "blend add result: {h}");
}

#[test]
fn full_pipeline_integration() {
    let mut field = TextureField::new(64, 64);
    let mut rng = SeededRng::new(42);

    // Run all ops in sequence
    let ops: Vec<Box<dyn TextureOp>> = vec![
        Box::new(BrickGridOp {
            columns: 4,
            rows: 8,
            stagger: 0.5,
            gap_width: 0.04,
            width_variation: 0.0,
            row_height_variation: 0.0,
            warp_x: None,
            warp_y: None,
            warp_strength: 0.0,
        }),
        Box::new(CellHeightOp { variation: 0.3 }),
        Box::new(MortarGrooveOp {
            depth: 0.15,
            width: 0.04,
        }),
        Box::new(CellColorOp {
            base_color: [0.47, 0.28, 0.14, 1.0],
            variation: 0.12,
        }),
        Box::new(MortarColorOp {
            color: [0.2, 0.18, 0.15, 1.0],
            threshold: 0.06,
        }),
        Box::new(DeriveNormalOp { strength: 1.5 }),
        Box::new(CellRoughnessOp {
            base: 0.75,
            variation: 0.05,
            mortar: 0.55,
            mortar_threshold: 0.06,
        }),
    ];

    for (i, op) in ops.iter().enumerate() {
        let mut op_rng = rng.fork(&format!("op_{i}"));
        op.apply(&mut field, &mut op_rng);
    }

    // Verify all output channels exist
    assert!(field.has_channel("r"));
    assert!(field.has_channel("g"));
    assert!(field.has_channel("b"));
    assert!(field.has_channel("normal_x"));
    assert!(field.has_channel("normal_y"));
    assert!(field.has_channel("normal_z"));
    assert!(field.has_channel("roughness"));

    // Verify output images are valid
    let albedo = field.to_albedo_image();
    assert!(albedo.validate().is_ok());
    let normal = field.to_normal_image();
    assert!(normal.validate().is_ok());
    let roughness = field.to_roughness_image();
    assert!(roughness.validate().is_ok());
}

// ── Phase 1 tests ────────────────────────────────────────────────────

#[test]
fn blend_screen_mode() {
    let mut field = TextureField::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            field.set("a", x, y, 0.5);
            field.set("b", x, y, 0.5);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = BlendOp {
        source: "b".into(),
        target: "a".into(),
        mode: BlendMode::Screen,
        strength: 1.0,
        mask: None,
    };
    op.apply(&mut field, &mut rng);
    let v = field.get("a", 0, 0);
    assert!(
        (v - 0.75).abs() < 0.01,
        "screen(0.5,0.5) should be ~0.75, got {v}"
    );
}

#[test]
fn math_op_power() {
    let mut field = TextureField::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            field.set("input", x, y, 0.5);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = MathOp {
        operation: MathOperation::Power,
        input_a: "input".into(),
        input_b: None,
        value_b: 2.0,
        output: "out".into(),
        clamp_output: false,
    };
    op.apply(&mut field, &mut rng);
    let v = field.get("out", 0, 0);
    assert!((v - 0.25).abs() < 0.01, "0.5^2 should be 0.25, got {v}");
}

#[test]
fn math_op_sin() {
    let mut field = TextureField::new(4, 4);
    field.set("input", 0, 0, 0.25); // sin(0.25 * TAU) = sin(PI/2) = 1.0
    let mut rng = SeededRng::new(42);
    let op = MathOp {
        operation: MathOperation::Sin,
        input_a: "input".into(),
        input_b: None,
        value_b: 0.0,
        output: "out".into(),
        clamp_output: false,
    };
    op.apply(&mut field, &mut rng);
    let v = field.get("out", 0, 0);
    assert!(
        (v - 1.0).abs() < 0.01,
        "sin(0.25*TAU) should be ~1.0, got {v}"
    );
}

#[test]
fn map_range_linear() {
    let mut field = TextureField::new(4, 4);
    field.set("input", 0, 0, 0.5);
    let mut rng = SeededRng::new(42);
    let op = MapRangeOp {
        input: "input".into(),
        output: "out".into(),
        from_min: 0.3,
        from_max: 0.7,
        to_min: 0.0,
        to_max: 1.0,
        interpolation: MapRangeInterp::Linear,
        clamp_output: true,
    };
    op.apply(&mut field, &mut rng);
    let v = field.get("out", 0, 0);
    assert!(
        (v - 0.5).abs() < 0.01,
        "map_range(0.5, 0.3..0.7 -> 0..1) = 0.5, got {v}"
    );
}

#[test]
fn color_ramp_two_stops() {
    let mut field = TextureField::new(4, 4);
    field.set("input", 0, 0, 0.5);
    let mut rng = SeededRng::new(42);
    let op = ColorRampOp {
        input: "input".into(),
        interpolation: ColorRampInterp::Linear,
        stops: vec![
            ColorStop {
                position: 0.0,
                color: [0.0, 0.0, 0.0],
            },
            ColorStop {
                position: 1.0,
                color: [1.0, 1.0, 1.0],
            },
        ],
    };
    op.apply(&mut field, &mut rng);
    let r = field.get("r", 0, 0);
    assert!(
        (r - 0.5).abs() < 0.01,
        "color_ramp midpoint should be 0.5, got {r}"
    );
}

#[test]
fn checker_texture_pattern() {
    let mut field = TextureField::new(8, 8);
    let mut rng = SeededRng::new(42);
    let op = CheckerTextureOp {
        output: "check".into(),
        scale_x: 2.0,
        scale_y: 2.0,
    };
    op.apply(&mut field, &mut rng);
    let v00 = field.get("check", 0, 0); // top-left quadrant
    let v40 = field.get("check", 4, 0); // top-right quadrant
    assert_ne!(v00, v40, "checker should alternate");
}

// ── Phase 2 tests ────────────────────────────────────────────────────

#[test]
fn gradient_texture_linear() {
    let mut field = TextureField::new(32, 32);
    let mut rng = SeededRng::new(42);
    let op = GradientTextureOp {
        output: "grad".into(),
        gradient_type: GradientType::Linear,
    };
    op.apply(&mut field, &mut rng);
    let left = field.get("grad", 0, 16);
    let right = field.get("grad", 31, 16);
    assert!(
        right > left,
        "linear gradient should increase left to right"
    );
}

#[test]
fn wave_texture_variation() {
    let mut field = TextureField::new(32, 32);
    let mut rng = SeededRng::new(42);
    let op = WaveTextureOp {
        output: "wave".into(),
        wave_type: WaveType::Sine,
        direction: WaveDirection::X,
        scale: 4.0,
        distortion: 0.0,
        detail: 0,
    };
    op.apply(&mut field, &mut rng);
    let mut has_variation = false;
    let first = field.get("wave", 0, 0);
    for x in 1..32 {
        if (field.get("wave", x, 0) - first).abs() > 0.01 {
            has_variation = true;
            break;
        }
    }
    assert!(has_variation, "wave should vary across x");
}

#[test]
fn white_noise_variation() {
    let mut field = TextureField::new(32, 32);
    let mut rng = SeededRng::new(42);
    let op = WhiteNoiseOp {
        output: "noise".into(),
    };
    op.apply(&mut field, &mut rng);
    let first = field.get("noise", 0, 0);
    let differs = (1..32).any(|x| (field.get("noise", x, 0) - first).abs() > 0.01);
    assert!(differs, "white noise should have variation");
}

#[test]
fn voronoi_texture_output() {
    let mut field = TextureField::new(32, 32);
    let mut rng = SeededRng::new(42);
    let op = VoronoiTextureOp {
        output: "vor".into(),
        cell_output: Some("cell".into()),
        scale: 4.0,
        randomness: 1.0,
        feature: VoronoiFeature::F1,
        metric: VoronoiMetric::Euclidean,
    };
    op.apply(&mut field, &mut rng);
    assert!(field.has_channel("vor"));
    assert!(field.has_channel("cell"));
    let differs = (1..32).any(|x| field.get("vor", x, 0) != field.get("vor", 0, 0));
    assert!(differs, "voronoi should have variation");
}

#[test]
fn musgrave_texture_output() {
    let mut field = TextureField::new(32, 32);
    let mut rng = SeededRng::new(42);
    let op = MusgraveTextureOp {
        output: "mus".into(),
        noise_type: NoiseType::Perlin,
        musgrave_type: MusgraveType::RidgedMultifractal,
        frequency: 4.0,
        octaves: 4,
        lacunarity: 2.0,
        dimension: 1.0,
        offset: 1.0,
        gain: 2.0,
        scale_x: 1.0,
        scale_y: 1.0,
        cell_offset: false,
    };
    op.apply(&mut field, &mut rng);
    assert!(field.has_channel("mus"));
    let differs = (1..32).any(|x| field.get("mus", x, 0) != field.get("mus", 0, 0));
    assert!(differs, "musgrave should have variation");
}

// ── Phase 3 tests ────────────────────────────────────────────────────

#[test]
fn invert_op() {
    let mut field = TextureField::new(4, 4);
    field.set("input", 0, 0, 0.3);
    let mut rng = SeededRng::new(42);
    let op = InvertOp {
        input: "input".into(),
        output: "out".into(),
        factor: 1.0,
    };
    op.apply(&mut field, &mut rng);
    let v = field.get("out", 0, 0);
    assert!((v - 0.7).abs() < 0.01, "invert(0.3) should be 0.7, got {v}");
}

#[test]
fn brightness_contrast_op() {
    let mut field = TextureField::new(4, 4);
    field.set("input", 0, 0, 0.5);
    let mut rng = SeededRng::new(42);
    let op = BrightnessContrastOp {
        input: "input".into(),
        output: "out".into(),
        brightness: 0.1,
        contrast: 0.0,
    };
    op.apply(&mut field, &mut rng);
    let v = field.get("out", 0, 0);
    assert!(
        (v - 0.6).abs() < 0.01,
        "brightness +0.1 on 0.5 should be 0.6, got {v}"
    );
}

#[test]
fn gamma_op() {
    let mut field = TextureField::new(4, 4);
    field.set("input", 0, 0, 0.25);
    let mut rng = SeededRng::new(42);
    let op = GammaOp {
        input: "input".into(),
        output: "out".into(),
        gamma: 0.5,
    };
    op.apply(&mut field, &mut rng);
    let v = field.get("out", 0, 0);
    assert!(
        (v - 0.5).abs() < 0.01,
        "gamma(0.25, 0.5) should be 0.5, got {v}"
    );
}

#[test]
fn clamp_op() {
    let mut field = TextureField::new(4, 4);
    field.set("input", 0, 0, 0.8);
    let mut rng = SeededRng::new(42);
    let op = ClampOp {
        input: "input".into(),
        output: "out".into(),
        min: 0.2,
        max: 0.6,
    };
    op.apply(&mut field, &mut rng);
    let v = field.get("out", 0, 0);
    assert!(
        (v - 0.6).abs() < 0.01,
        "clamp(0.8, 0.2, 0.6) should be 0.6, got {v}"
    );
}

#[test]
fn hsv_adjust_op() {
    let mut field = TextureField::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            field.set("r", x, y, 0.5);
            field.set("g", x, y, 0.3);
            field.set("b", x, y, 0.1);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = HsvAdjustOp {
        hue_offset: 0.0,
        saturation_factor: 1.0,
        value_factor: 1.0,
    };
    op.apply(&mut field, &mut rng);
    // Identity transform should preserve values
    let r = field.get("r", 0, 0);
    let g = field.get("g", 0, 0);
    let b = field.get("b", 0, 0);
    assert!((r - 0.5).abs() < 0.02, "r should be ~0.5, got {r}");
    assert!((g - 0.3).abs() < 0.02, "g should be ~0.3, got {g}");
    assert!((b - 0.1).abs() < 0.02, "b should be ~0.1, got {b}");
}

// ── Phase 4 tests ────────────────────────────────────────────────────

#[test]
fn blur_op_smooths() {
    let mut field = TextureField::new(16, 16);
    // Sharp edge
    for y in 0..16 {
        for x in 0..16 {
            field.set("input", x, y, if x < 8 { 0.0 } else { 1.0 });
        }
    }
    let mut rng = SeededRng::new(42);
    let op = BlurOp {
        input: "input".into(),
        output: "out".into(),
        radius: 2,
        sigma: 1.0,
    };
    op.apply(&mut field, &mut rng);
    // Middle should now be between 0 and 1 (blurred)
    let v = field.get("out", 8, 8);
    assert!(v > 0.1 && v < 0.9, "blur should smooth the edge, got {v}");
}

#[test]
fn edge_detect_on_flat() {
    let mut field = TextureField::new(16, 16);
    for y in 0..16 {
        for x in 0..16 {
            field.set("input", x, y, 0.5);
        }
    }
    let mut rng = SeededRng::new(42);
    let op = EdgeDetectOp {
        input: "input".into(),
        output: "out".into(),
    };
    op.apply(&mut field, &mut rng);
    // Flat surface should have ~0 edges
    for y in 0..16 {
        for x in 0..16 {
            let v = field.get("out", x, y);
            assert!(
                v < 0.01,
                "flat surface should have no edges at ({x},{y}): {v}"
            );
        }
    }
}

#[test]
fn sharpen_op_enhances() {
    let mut field = TextureField::new(16, 16);
    for y in 0..16 {
        for x in 0..16 {
            field.set("input", x, y, if x < 8 { 0.3 } else { 0.7 });
        }
    }
    let mut rng = SeededRng::new(42);
    let op = SharpenOp {
        input: "input".into(),
        output: "out".into(),
        strength: 1.0,
        radius: 1,
    };
    op.apply(&mut field, &mut rng);
    // Interior values should be preserved or enhanced
    let left = field.get("out", 2, 8);
    let right = field.get("out", 13, 8);
    assert!(right > left, "sharpen should maintain or enhance contrast");
}

// ─── CellBulgeOp tests ──────────────────────────────────────────────

#[test]
fn bulge_falloff_boundary_conditions() {
    for falloff in [
        BulgeFalloff::Linear,
        BulgeFalloff::Parabolic,
        BulgeFalloff::Cosine,
    ] {
        let at_zero = falloff.apply(0.0);
        let at_one = falloff.apply(1.0);
        assert!(
            at_zero.abs() < 1e-6,
            "{falloff:?}: expected ~0 at t=0, got {at_zero}"
        );
        assert!(
            (at_one - 1.0).abs() < 1e-6,
            "{falloff:?}: expected ~1 at t=1, got {at_one}"
        );
    }
}

#[test]
fn cell_bulge_raises_center_not_boundary() {
    let mut field = TextureField::new(64, 64);
    let mut rng = SeededRng::new(42);

    // Set up two cells: left half = cell 1, right half = cell 2
    // edge_dist = distance from nearest cell boundary (higher = more interior)
    field.ensure_channel("cell_id");
    field.ensure_channel("edge_dist");
    field.ensure_channel("height");
    for y in 0..64 {
        for x in 0..64 {
            let (cell_id, dist) = if x < 32 {
                (1.0, (x as f32).min(31.0 - x as f32) / 31.0)
            } else {
                (2.0, ((x - 32) as f32).min(63.0 - x as f32) / 31.0)
            };
            field.set("cell_id", x, y, cell_id);
            field.set("edge_dist", x, y, dist);
            field.set("height", x, y, 0.5);
        }
    }

    let op = CellBulgeOp {
        strength: 0.3,
        falloff: BulgeFalloff::Cosine,
    };
    op.apply(&mut field, &mut rng);

    // Center of cell 1 (x=16) should be raised
    let center_h = field.get("height", 16, 32);
    assert!(center_h > 0.5, "center should be raised, got {center_h}");

    // Boundary of cell 1 (x=0) should be ~unchanged
    let edge_h = field.get("height", 0, 32);
    assert!(
        (edge_h - 0.5).abs() < 0.01,
        "boundary should be ~unchanged, got {edge_h}"
    );
}

#[test]
fn cell_bulge_per_cell_normalization() {
    let mut field = TextureField::new(64, 64);
    let mut rng = SeededRng::new(42);

    field.ensure_channel("cell_id");
    field.ensure_channel("edge_dist");
    field.ensure_channel("height");

    // Cell 1 (top half): max edge_dist = 0.5
    // Cell 2 (bottom half): max edge_dist = 1.0
    for y in 0..64 {
        for x in 0..64 {
            let (cell_id, max_d) = if y < 32 { (1.0, 0.5) } else { (2.0, 1.0) };
            let fy = if y < 32 { y as f32 } else { (y - 32) as f32 };
            let dist = (fy / 31.0).min(1.0) * max_d;
            field.set("cell_id", x, y, cell_id);
            field.set("edge_dist", x, y, dist);
            field.set("height", x, y, 0.0);
        }
    }

    let op = CellBulgeOp {
        strength: 1.0,
        falloff: BulgeFalloff::Linear,
    };
    op.apply(&mut field, &mut rng);

    // Both cells should reach similar peak height at their center
    // despite different max edge_dist ranges
    let peak1 = field.get("height", 32, 31); // cell 1 center-ish
    let peak2 = field.get("height", 32, 63); // cell 2 center-ish
    let diff = (peak1 - peak2).abs();
    assert!(
        diff < 0.15,
        "peaks should be similar after normalization: cell1={peak1}, cell2={peak2}"
    );
}

#[test]
fn noise_layer_cell_offset_decorrelates() {
    // Two-cell field: left half = cell 0, right half = cell 1
    let size = 64;
    let mut field = TextureField::new(size, size);
    field.ensure_channel("cell_id");
    for y in 0..size {
        for x in 0..size {
            let cid = if x < size / 2 { 0.0 } else { 1.0 };
            field.set("cell_id", x, y, cid);
        }
    }

    // With cell_offset = true, noise should differ across cells
    let mut rng_on = SeededRng::new(99);
    let op_on = NoiseLayerOp {
        output: "n_on".into(),
        frequency: 10.0,
        octaves: 4,
        noise_type: NoiseType::Perlin,
        scale_x: 1.0,
        scale_y: 1.0,
        cell_offset: true,
    };
    op_on.apply(&mut field, &mut rng_on);

    // With cell_offset = false, noise should be the same at mirrored positions
    let mut rng_off = SeededRng::new(99);
    let op_off = NoiseLayerOp {
        output: "n_off".into(),
        frequency: 10.0,
        octaves: 4,
        noise_type: NoiseType::Perlin,
        scale_x: 1.0,
        scale_y: 1.0,
        cell_offset: false,
    };
    op_off.apply(&mut field, &mut rng_off);

    // Compare a sample point in each half — with offset they should differ
    let y = size / 2;
    let x_left = size / 4;
    let x_right = size / 4 + size / 2;
    let on_left = field.get("n_on", x_left, y);
    let on_right = field.get("n_on", x_right, y);
    let off_left = field.get("n_off", x_left, y);
    let off_right = field.get("n_off", x_right, y);

    // Without offset, mirrored positions sample same UV → same value
    assert!(
        (off_left - off_right).abs() < 0.01,
        "without cell_offset, mirrored positions should match: {off_left} vs {off_right}"
    );
    // With offset, cells should be decorrelated
    assert!(
        (on_left - on_right).abs() > 0.01,
        "with cell_offset, different cells should differ: {on_left} vs {on_right}"
    );
}

#[test]
fn cell_offset_no_cell_id_fallback() {
    // No cell_id channel — cell_offset=true should not panic and match cell_offset=false
    let size = 32;
    let mut field_on = TextureField::new(size, size);
    let mut rng_on = SeededRng::new(55);
    let op_on = NoiseLayerOp {
        output: "sig".into(),
        frequency: 8.0,
        octaves: 3,
        noise_type: NoiseType::Perlin,
        scale_x: 1.0,
        scale_y: 1.0,
        cell_offset: true,
    };
    op_on.apply(&mut field_on, &mut rng_on);

    let mut field_off = TextureField::new(size, size);
    let mut rng_off = SeededRng::new(55);
    let op_off = NoiseLayerOp {
        output: "sig".into(),
        frequency: 8.0,
        octaves: 3,
        noise_type: NoiseType::Perlin,
        scale_x: 1.0,
        scale_y: 1.0,
        cell_offset: false,
    };
    op_off.apply(&mut field_off, &mut rng_off);

    for y in 0..size {
        for x in 0..size {
            let a = field_on.get("sig", x, y);
            let b = field_off.get("sig", x, y);
            assert!(
                (a - b).abs() < 1e-6,
                "without cell_id channel, results should match: ({x},{y}) {a} vs {b}"
            );
        }
    }
}

#[test]
fn musgrave_cell_offset_decorrelates() {
    let size = 64;
    let mut field = TextureField::new(size, size);
    field.ensure_channel("cell_id");
    for y in 0..size {
        for x in 0..size {
            let cid = if x < size / 2 { 0.0 } else { 1.0 };
            field.set("cell_id", x, y, cid);
        }
    }

    let mut rng_on = SeededRng::new(77);
    let op_on = MusgraveTextureOp {
        output: "m_on".into(),
        noise_type: NoiseType::Perlin,
        musgrave_type: MusgraveType::HybridMultifractal,
        frequency: 8.0,
        octaves: 4,
        lacunarity: 2.0,
        dimension: 1.0,
        offset: 1.0,
        gain: 2.0,
        scale_x: 1.0,
        scale_y: 1.0,
        cell_offset: true,
    };
    op_on.apply(&mut field, &mut rng_on);

    let mut rng_off = SeededRng::new(77);
    let op_off = MusgraveTextureOp {
        output: "m_off".into(),
        noise_type: NoiseType::Perlin,
        musgrave_type: MusgraveType::HybridMultifractal,
        frequency: 8.0,
        octaves: 4,
        lacunarity: 2.0,
        dimension: 1.0,
        offset: 1.0,
        gain: 2.0,
        scale_x: 1.0,
        scale_y: 1.0,
        cell_offset: false,
    };
    op_off.apply(&mut field, &mut rng_off);

    let y = size / 2;
    let x_left = size / 4;
    let x_right = size / 4 + size / 2;
    let off_left = field.get("m_off", x_left, y);
    let off_right = field.get("m_off", x_right, y);
    let on_left = field.get("m_on", x_left, y);
    let on_right = field.get("m_on", x_right, y);

    assert!(
        (off_left - off_right).abs() < 0.01,
        "without cell_offset, mirrored musgrave should match: {off_left} vs {off_right}"
    );
    assert!(
        (on_left - on_right).abs() > 0.01,
        "with cell_offset, different cells should differ: {on_left} vs {on_right}"
    );
}

#[test]
fn cell_noise_offset_determinism() {
    // Same inputs → same outputs
    let (a1, b1) = cell_noise_offset(5, 12345);
    let (a2, b2) = cell_noise_offset(5, 12345);
    assert_eq!(a1, a2);
    assert_eq!(b1, b2);

    // Different cell_ids → different offsets
    let (a3, b3) = cell_noise_offset(6, 12345);
    assert!(
        a1 != a3 || b1 != b3,
        "different cell_ids should give different offsets"
    );

    // Different seeds → different offsets
    let (a4, b4) = cell_noise_offset(5, 99999);
    assert!(
        a1 != a4 || b1 != b4,
        "different seeds should give different offsets"
    );
}

#[test]
fn edge_erode_modifies_boundaries_not_interiors() {
    let mut field = TextureField::new(64, 64);
    let mut rng = SeededRng::new(42);

    // Create a simple brick grid
    let grid = BrickGridOp {
        columns: 4,
        rows: 4,
        stagger: 0.5,
        gap_width: 0.04,
        width_variation: 0.0,
        row_height_variation: 0.0,
        warp_x: None,
        warp_y: None,
        warp_strength: 0.0,
    };
    grid.apply(&mut field, &mut rng);

    // Snapshot cell_id before erosion
    let pre_cell_id: Vec<f32> = (0..64 * 64)
        .map(|i| field.get("cell_id", i % 64, i / 64))
        .collect();

    let erode = EdgeErodeOp {
        noise_frequency: 40.0,
        strength: 0.02,
        edge_width: 0.06,
        noise_type: NoiseType::Perlin,
        octaves: 1,
        threshold: 0.0, // erode everywhere for test coverage
    };
    erode.apply(&mut field, &mut rng);

    // Interior pixels (edge_dist >= 0.06) should be unchanged
    let mut interior_unchanged = 0u32;
    let mut interior_total = 0u32;
    // Boundary pixels (edge_dist < 0.06) should have at least some changes
    let mut boundary_changed = 0u32;
    let mut boundary_total = 0u32;

    for y in 0..64u32 {
        for x in 0..64u32 {
            let idx = (y * 64 + x) as usize;
            let pre_ed = {
                // Use the pre-erosion edge_dist from the snapshot
                // Re-run grid to get a clean snapshot of edge_dist
                // (we already have pre_cell_id, but need edge_dist too)
                // Instead, just check post-erosion state vs pre cell_id
                let post_cid = field.get("cell_id", x, y);
                let pre_cid = pre_cell_id[idx];
                // Pixels far from edges have large edge_dist and were skipped
                if (post_cid - pre_cid).abs() < 0.001 {
                    interior_unchanged += 1;
                }
                if field.get("edge_dist", x, y) < 0.06 {
                    boundary_total += 1;
                    if (post_cid - pre_cid).abs() > 0.001 {
                        boundary_changed += 1;
                    }
                } else {
                    interior_total += 1;
                }
            };
            let _ = pre_ed;
        }
    }

    // All interior pixels should be unchanged
    assert_eq!(
        interior_unchanged,
        interior_total + boundary_total - boundary_changed,
        "some interior pixels were modified unexpectedly"
    );
    // At least some boundary pixels should have changed cell_id
    assert!(
            boundary_changed > 0,
            "edge_erode should modify at least some boundary pixels, but changed 0 out of {boundary_total}"
        );
}
