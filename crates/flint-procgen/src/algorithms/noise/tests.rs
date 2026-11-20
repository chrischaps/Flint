//! The noise functions' tests, moved whole from the old `noise.rs` (#1026).

use super::cellular::{cell_hash, unit, voronoi_distance};
use super::*;

/// 100 deterministic sample points spread over several lattice cells,
/// including negative coordinates.
fn points() -> impl Iterator<Item = (f64, f64, f64)> {
    (0..100).map(|i| {
        let t = i as f64;
        (t * 0.173 - 8.0, t * 0.071 + 0.3, t * -0.113)
    })
}

fn assert_deterministic(make: impl Fn() -> Box<dyn NoiseSource>) {
    let a = make();
    let b = make();
    for (x, y, z) in points() {
        assert_eq!(a.sample_2d(x, y).to_bits(), b.sample_2d(x, y).to_bits());
        assert_eq!(
            a.sample_3d(x, y, z).to_bits(),
            b.sample_3d(x, y, z).to_bits()
        );
    }
}

// -- Determinism ----------------------------------------------------------

#[test]
fn perlin_determinism() {
    assert_deterministic(|| Box::new(PerlinNoise::new(42)));
}

#[test]
fn simplex_determinism() {
    assert_deterministic(|| Box::new(SimplexNoise::new(42)));
}

#[test]
fn value_determinism() {
    assert_deterministic(|| Box::new(ValueNoise::new(42)));
}

#[test]
fn worley_determinism() {
    assert_deterministic(|| Box::new(WorleyNoise::new(42)));
    assert_deterministic(|| Box::new(WorleyNoise::new(42).with_return_distance(true)));
    let a = WorleyNoise::new(42);
    let b = WorleyNoise::new(42);
    for (x, y, _) in points() {
        assert_eq!(a.f1_f2(x, y), b.f1_f2(x, y));
    }
}

#[test]
fn fbm_determinism() {
    assert_deterministic(|| Box::new(perlin_fbm(42, 6, 1.0)));
}

#[test]
fn permutation_table_is_a_permutation() {
    let t = PermutationTable::new(7);
    let mut seen = [false; 256];
    for &b in &t.perm[..256] {
        seen[b as usize] = true;
    }
    assert!(
        seen.iter().all(|&s| s),
        "table must contain every byte once"
    );
    assert_eq!(
        t.perm[..256],
        t.perm[256..],
        "second half mirrors the first"
    );
}

// -- Different seeds differ -----------------------------------------------

#[test]
fn different_seeds_differ() {
    let pairs: Vec<(Box<dyn NoiseSource>, Box<dyn NoiseSource>)> = vec![
        (Box::new(PerlinNoise::new(1)), Box::new(PerlinNoise::new(2))),
        (
            Box::new(SimplexNoise::new(1)),
            Box::new(SimplexNoise::new(2)),
        ),
        (Box::new(ValueNoise::new(1)), Box::new(ValueNoise::new(2))),
        (Box::new(WorleyNoise::new(1)), Box::new(WorleyNoise::new(2))),
    ];
    for (a, b) in &pairs {
        let differs = points().any(|(x, y, _)| a.sample_2d(x, y) != b.sample_2d(x, y));
        assert!(differs, "different seeds should produce different output");
    }
}

// -- Output range ---------------------------------------------------------

/// 10,000 points across many cells, both signs.
fn range_points() -> impl Iterator<Item = (f64, f64, f64)> {
    (0..10_000).map(|i| {
        let t = i as f64;
        (
            t * 0.037 - 150.0,
            t * 0.023 - 90.0,
            (t * 0.719).sin() * 40.0,
        )
    })
}

fn assert_unit_range(name: &str, n: &dyn NoiseSource) {
    for (x, y, z) in range_points() {
        let v = n.sample_2d(x, y);
        assert!(
            (-1.0..=1.0).contains(&v),
            "{name} 2d out of range: {v} at ({x}, {y})"
        );
        let v3 = n.sample_3d(x, y, z);
        assert!(
            (-1.0..=1.0).contains(&v3),
            "{name} 3d out of range: {v3} at ({x}, {y}, {z})"
        );
    }
}

#[test]
fn perlin_output_range() {
    assert_unit_range("perlin", &PerlinNoise::new(99));
}

#[test]
fn simplex_output_range() {
    assert_unit_range("simplex", &SimplexNoise::new(99));
}

#[test]
fn value_output_range() {
    assert_unit_range("value", &ValueNoise::new(99));
}

#[test]
fn worley_output_range() {
    assert_unit_range("worley", &WorleyNoise::new(99));
    assert_unit_range(
        "worley distance",
        &WorleyNoise::new(99).with_return_distance(true),
    );
}

#[test]
fn fbm_normalization() {
    assert_unit_range("fbm", &perlin_fbm(99, 8, 2.0));
}

/// The fields should actually use their range, not hug zero.
#[test]
fn gradient_noise_spans_its_range() {
    let sources: Vec<(&str, Box<dyn NoiseSource>)> = vec![
        ("perlin", Box::new(PerlinNoise::new(5))),
        ("simplex", Box::new(SimplexNoise::new(5))),
    ];
    for (name, n) in &sources {
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for (x, y, _) in range_points() {
            let v = n.sample_2d(x, y);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(
            lo < -0.6 && hi > 0.6,
            "{name} range too narrow: [{lo}, {hi}]"
        );
    }
}

// -- Structural properties ------------------------------------------------

#[test]
fn perlin_is_zero_on_the_lattice() {
    let n = PerlinNoise::new(3);
    for i in -5..5 {
        for j in -5..5 {
            let (x, y) = (i as f64, j as f64);
            assert_eq!(n.sample_2d(x, y), 0.0);
            assert_eq!(n.sample_3d(x, y, (i + j) as f64), 0.0);
        }
    }
}

/// Shifting by a whole period on either axis lands on the same value, and
/// inside one period the field is the ordinary Perlin it was built from.
#[test]
fn periodic_perlin_repeats_on_its_period() {
    let n = PerlinNoise::new(11);
    for (x, y, _) in points() {
        let a = n.sample_2d_periodic(x, y, 6, 3);
        for (dx, dy) in [(6.0, 0.0), (0.0, -3.0), (-12.0, 9.0)] {
            let b = n.sample_2d_periodic(x + dx, y + dy, 6, 3);
            assert!(
                (a - b).abs() < 1e-9,
                "({x}, {y}) + ({dx}, {dy}): {a} vs {b}"
            );
        }
    }
    assert_eq!(n.sample_2d_periodic(2.3, 1.7, 8, 8), n.sample_2d(2.3, 1.7));
}

/// Small steps produce small changes: catches seams at cell or simplex
/// boundaries (e.g. a simplex kernel radius that doesn't fall to zero).
#[test]
fn noise_is_continuous() {
    let sources: Vec<(&str, Box<dyn NoiseSource>)> = vec![
        ("perlin", Box::new(PerlinNoise::new(11))),
        ("simplex", Box::new(SimplexNoise::new(11))),
        ("value", Box::new(ValueNoise::new(11))),
    ];
    let h = 1e-4;
    for (name, n) in &sources {
        for (x, y, z) in range_points() {
            let d2 = (n.sample_2d(x + h, y + h) - n.sample_2d(x, y)).abs();
            let d3 = (n.sample_3d(x + h, y + h, z + h) - n.sample_3d(x, y, z)).abs();
            assert!(d2 < 0.01, "{name} 2d jumps by {d2} at ({x}, {y})");
            assert!(d3 < 0.01, "{name} 3d jumps by {d3} at ({x}, {y}, {z})");
        }
    }
}

#[test]
fn noise_tiles_every_256_units() {
    let n = PerlinNoise::new(8);
    for (x, y, z) in points() {
        approx_eq(n.sample_2d(x, y), n.sample_2d(x + 256.0, y - 256.0));
        approx_eq(n.sample_3d(x, y, z), n.sample_3d(x, y + 256.0, z));
    }
}

// -- Worley ---------------------------------------------------------------

#[test]
fn worley_f1_never_exceeds_f2() {
    for metric in [VoronoiMetric::Euclidean, VoronoiMetric::Manhattan] {
        let n = WorleyNoise::new(17).with_metric(metric);
        for (x, y, _) in range_points() {
            let s = n.f1_f2(x, y);
            assert!(
                s.f1 <= s.f2,
                "{metric:?}: f1 {} > f2 {} at ({x}, {y})",
                s.f1,
                s.f2
            );
            assert!((0.0..=1.0).contains(&s.f1) && (0.0..=1.0).contains(&s.f2));
        }
    }
}

/// The ring search must agree with a brute-force scan of a wide window.
#[test]
fn worley_matches_brute_force() {
    for metric in [VoronoiMetric::Euclidean, VoronoiMetric::Manhattan] {
        let n = WorleyNoise::new(23).with_metric(metric);
        for (x, y, _) in range_points().step_by(10) {
            let (ix, fx) = split(x);
            let (iy, fy) = split(y);
            let mut d: Vec<f64> = Vec::new();
            for dy in -4..=4 {
                for dx in -4..=4 {
                    let h = cell_hash(23, &[ix + dx, iy + dy]);
                    let px = dx as f64 + unit(h, 0);
                    let py = dy as f64 + unit(h, 1);
                    d.push(voronoi_distance(px - fx, py - fy, metric));
                }
            }
            d.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let got = n.nearest_2d(x, y, (None, None));
            assert_eq!(got.f1, d[0], "{metric:?} f1 at ({x}, {y})");
            assert_eq!(got.f2, d[1], "{metric:?} f2 at ({x}, {y})");
        }
    }
}

#[test]
fn worley_metrics_differ() {
    let e = WorleyNoise::new(4);
    let m = WorleyNoise::new(4).with_metric(VoronoiMetric::Manhattan);
    assert!(points().any(|(x, y, _)| e.f1_f2(x, y) != m.f1_f2(x, y)));
}

#[test]
fn worley_f1_is_zero_at_a_feature_point() {
    let n = WorleyNoise::new(31);
    let h = cell_hash(31, &[3, -2]);
    let (px, py) = (3.0 + unit(h, 0), -2.0 + unit(h, 1));
    assert!(n.f1_f2(px, py).f1 < 1e-6);
}

// -- FBM ------------------------------------------------------------------

#[test]
fn fbm_one_octave_matches_base() {
    let sources: Vec<(Box<dyn NoiseSource>, Box<dyn NoiseSource>)> = vec![
        (
            Box::new(PerlinNoise::new(9)),
            Box::new(Fbm::new(PerlinNoise::new(9)).with_octaves(1)),
        ),
        (
            Box::new(SimplexNoise::new(9)),
            Box::new(Fbm::new(SimplexNoise::new(9)).with_octaves(1)),
        ),
        (
            Box::new(WorleyNoise::new(9)),
            Box::new(Fbm::new(WorleyNoise::new(9)).with_octaves(1)),
        ),
    ];
    for (base, fbm) in &sources {
        for (x, y, z) in points() {
            assert_eq!(base.sample_2d(x, y), fbm.sample_2d(x, y));
            assert_eq!(base.sample_3d(x, y, z), fbm.sample_3d(x, y, z));
        }
    }
}

#[test]
fn fbm_more_octaves_adds_detail() {
    let one = Fbm::new(PerlinNoise::new(9)).with_octaves(1);
    let four = Fbm::new(PerlinNoise::new(9)).with_octaves(4);
    let differs = points().filter(|&(x, y, _)| one.sample_2d(x, y) != four.sample_2d(x, y));
    assert!(
        differs.count() > 90,
        "4 octaves should differ almost everywhere"
    );

    // Detail = wiggles: count turning points along a line. Each octave
    // adds features at twice the frequency of the last.
    let turns = |n: &dyn NoiseSource| {
        let v: Vec<f64> = (0..4000)
            .map(|i| n.sample_2d(i as f64 * 0.005, 0.37))
            .collect();
        v.windows(3)
            .filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0)
            .count()
    };
    let (t1, t4) = (turns(&one), turns(&four));
    assert!(
        t4 > t1 * 3,
        "4 octaves should turn far more often: {t4} vs {t1}"
    );
}

#[test]
fn fbm_amplitude_scales_output() {
    let base = Fbm::new(SimplexNoise::new(2)).with_octaves(3);
    let half = Fbm::new(SimplexNoise::new(2))
        .with_octaves(3)
        .with_amplitude(0.5);
    for (x, y, _) in points() {
        approx_eq(base.sample_2d(x, y) * 0.5, half.sample_2d(x, y));
    }
}

#[test]
fn fbm_zero_octaves_is_silent() {
    let n = Fbm::new(PerlinNoise::new(1)).with_octaves(0);
    assert_eq!(n.sample_2d(0.3, 0.7), 0.0);
}

// -- from_rng determinism -------------------------------------------------

#[test]
fn from_rng_determinism() {
    let mut rng_a = SeededRng::new(100);
    let mut rng_b = SeededRng::new(100);
    let pa = PerlinNoise::from_rng(&mut rng_a);
    let pb = PerlinNoise::from_rng(&mut rng_b);
    assert_eq!(pa.sample_2d(1.5, 2.5), pb.sample_2d(1.5, 2.5));

    let sa = SimplexNoise::from_rng(&mut rng_a);
    let sb = SimplexNoise::from_rng(&mut rng_b);
    assert_eq!(sa.sample_2d(1.5, 2.5), sb.sample_2d(1.5, 2.5));

    let wa = WorleyNoise::from_rng(&mut rng_a);
    let wb = WorleyNoise::from_rng(&mut rng_b);
    assert_eq!(wa.sample_2d(1.5, 2.5), wb.sample_2d(1.5, 2.5));

    let ta = PermutationTable::from_rng(&mut rng_a);
    let tb = PermutationTable::from_rng(&mut rng_b);
    assert_eq!(ta.perm, tb.perm);
}

// -- Known-value snapshots ------------------------------------------------

/// Pinned outputs for seed 42. These change only if the algorithms, the
/// permutation shuffle or the Worley hash change — any of which alters
/// every procedural texture and mesh built on them, so update deliberately.
#[test]
fn known_value_snapshots() {
    let perlin = PerlinNoise::new(42);
    let simplex = SimplexNoise::new(42);
    let value = ValueNoise::new(42);
    let worley = WorleyNoise::new(42);

    let cases: [(&str, f64, f64); 9] = [
        ("perlin 2d", perlin.sample_2d(0.5, 0.5), SNAP_PERLIN_2D),
        (
            "perlin 3d",
            perlin.sample_3d(1.25, -3.5, 7.75),
            SNAP_PERLIN_3D,
        ),
        ("simplex 2d", simplex.sample_2d(0.5, 0.5), SNAP_SIMPLEX_2D),
        (
            "simplex 3d",
            simplex.sample_3d(1.25, -3.5, 7.75),
            SNAP_SIMPLEX_3D,
        ),
        ("value 2d", value.sample_2d(0.5, 0.5), SNAP_VALUE_2D),
        ("value 3d", value.sample_3d(1.25, -3.5, 7.75), SNAP_VALUE_3D),
        ("worley 2d", worley.sample_2d(0.5, 0.5), SNAP_WORLEY_2D),
        (
            "worley f1",
            worley.f1_f2(2.3, -1.7).f1 as f64,
            SNAP_WORLEY_F1,
        ),
        (
            "worley f2",
            worley.f1_f2(2.3, -1.7).f2 as f64,
            SNAP_WORLEY_F2,
        ),
    ];
    let drift: Vec<String> = cases
        .iter()
        .filter(|(_, got, want)| (got - want).abs() >= 1e-9)
        .map(|(name, got, want)| format!("{name}: got {got:.12}, pinned {want:.12}"))
        .collect();
    assert!(
        drift.is_empty(),
        "noise output drifted:
{}",
        drift.join(
            "
"
        )
    );
}

const SNAP_PERLIN_2D: f64 = -0.353553390593;
const SNAP_PERLIN_3D: f64 = 0.227206768376;
const SNAP_SIMPLEX_2D: f64 = -0.434384907343;
const SNAP_SIMPLEX_3D: f64 = 0.115576850043;
const SNAP_VALUE_2D: f64 = 0.260784313725;
const SNAP_VALUE_3D: f64 = -0.026770842309;
const SNAP_WORLEY_2D: f64 = -0.673696482294;
const SNAP_WORLEY_F1: f64 = 0.277442604303;
const SNAP_WORLEY_F2: f64 = 0.392840534449;

fn approx_eq(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "values differ: {a} vs {b}");
}

// -- noise_to_image -------------------------------------------------------

#[test]
fn noise_to_image_dimensions() {
    let perlin = PerlinNoise::new(1);
    let img = noise_to_image(&perlin, 64, 32, 4.0);
    assert_eq!(img.width, 64);
    assert_eq!(img.height, 32);
    assert_eq!(img.channel_semantics, ChannelSemantics::Height);
    assert!(img.validate().is_ok());
}

#[test]
fn noise_to_image_not_uniform() {
    let perlin = PerlinNoise::new(7);
    let img = noise_to_image(&perlin, 32, 32, 4.0);
    // Not all pixels should be the same value.
    let first = img.pixels[0];
    let has_variation = img.pixels.iter().step_by(4).any(|&p| p != first);
    assert!(has_variation, "noise image should not be uniform");
}

// -- 01 remappers ---------------------------------------------------------

#[test]
fn remap_01_bounds() {
    let n = PerlinNoise::new(42);
    for i in 0..1000 {
        let x = i as f64 * 0.05;
        let y = i as f64 * 0.03;
        let v = n.sample_2d_01(x, y);
        assert!((0.0..=1.0).contains(&v), "01 remap out of range: {v}");
    }
}

// -- Visual debug ---------------------------------------------------------

/// F1, F2 or F2 − F1 of a Worley field as a [`NoiseSource`] in \[-1, 1\],
/// for the debug images only.
struct WorleyFeature(WorleyNoise, fn(WorleySample) -> f32);

impl NoiseSource for WorleyFeature {
    fn sample_2d(&self, x: f64, y: f64) -> f64 {
        (self.1)(self.0.f1_f2(x, y)) as f64 * 2.0 - 1.0
    }
    fn sample_3d(&self, x: f64, y: f64, _z: f64) -> f64 {
        self.sample_2d(x, y)
    }
}

/// Writes a 256×256 PNG of every noise type to `target/test-output/noise/`
/// for manual inspection. Not an assertion — a visual aid.
#[test]
fn noise_visual_debug_pngs() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-output/noise");
    std::fs::create_dir_all(&dir).unwrap();

    let size = 256;
    let scale = 8.0;
    let manhattan = || WorleyNoise::new(42).with_metric(VoronoiMetric::Manhattan);

    let cases: Vec<(&str, Box<dyn NoiseSource>)> = vec![
        ("perlin", Box::new(PerlinNoise::new(42))),
        ("simplex", Box::new(SimplexNoise::new(42))),
        ("value", Box::new(ValueNoise::new(42))),
        ("worley_value", Box::new(WorleyNoise::new(42))),
        (
            "worley_distance",
            Box::new(WorleyNoise::new(42).with_return_distance(true)),
        ),
        (
            "worley_f1",
            Box::new(WorleyFeature(WorleyNoise::new(42), |s| s.f1)),
        ),
        (
            "worley_f2",
            Box::new(WorleyFeature(WorleyNoise::new(42), |s| s.f2)),
        ),
        (
            "worley_f2_minus_f1",
            Box::new(WorleyFeature(WorleyNoise::new(42), |s| s.f2 - s.f1)),
        ),
        (
            "worley_f1_manhattan",
            Box::new(WorleyFeature(manhattan(), |s| s.f1)),
        ),
        ("perlin_fbm_1", Box::new(perlin_fbm(42, 1, 2.0))),
        ("perlin_fbm", Box::new(perlin_fbm(42, 6, 2.0))),
        ("simplex_fbm", Box::new(simplex_fbm(42, 6, 2.0))),
    ];

    for (name, source) in &cases {
        let img = noise_to_image(source.as_ref(), size, size, scale);
        let path = dir.join(format!("{name}.png"));
        image::save_buffer(
            &path,
            &img.pixels,
            img.width,
            img.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

fn tiling_sources() -> Vec<(&'static str, Box<dyn NoiseSource>)> {
    vec![
        ("perlin", Box::new(PerlinNoise::new(4))),
        ("simplex", Box::new(SimplexNoise::new(4))),
        ("value", Box::new(ValueNoise::new(4))),
        ("worley", Box::new(WorleyNoise::new(4).with_frequency(1.7))),
        (
            "worley distance",
            Box::new(WorleyNoise::new(4).with_return_distance(true)),
        ),
        (
            "perlin fbm",
            Box::new(Fbm::new(PerlinNoise::new(4)).with_frequency(2.3)),
        ),
        (
            "simplex fbm",
            Box::new(Fbm::new(SimplexNoise::new(4)).with_octaves(4)),
        ),
        (
            "worley fbm",
            Box::new(Fbm::new(WorleyNoise::new(4)).with_lacunarity(2.2)),
        ),
    ]
}

/// A tiled sample repeats exactly one period on along each axis, for
/// fractional periods too, and stays in range.
#[test]
fn tiled_samples_repeat_every_period() {
    for (name, n) in tiling_sources() {
        for (px, py) in [(1.0, 1.0), (3.3, 0.7), (6.0, 2.5)] {
            for (x, y) in points().map(|(x, y, _)| (x.rem_euclid(px), y.rem_euclid(py))) {
                let a = n.sample_2d_tiled(x, y, px, py);
                assert!((-1.0..=1.0).contains(&a), "{name}: {a} out of range");
                for (b, axis) in [
                    (n.sample_2d_tiled(x + px, y, px, py), "x"),
                    (n.sample_2d_tiled(x, y - py, px, py), "y"),
                    (n.sample_2d_tiled(x - 2.0 * px, y + py, px, py), "xy"),
                ] {
                    assert!(
                        (a - b).abs() < 1e-6,
                        "{name} period ({px}, {py}) at ({x}, {y}) shifted {axis}: {a} vs {b}"
                    );
                }
            }
        }
    }
}

/// Wrapping a lattice changes only which cells repeat: inside the first
/// tile, a whole-number period gives the plain field back exactly.
#[test]
fn lattice_tiling_keeps_the_field() {
    let perlin = PerlinNoise::new(8);
    let value = ValueNoise::new(8);
    let worley = WorleyNoise::new(8);
    for (x, y) in [(0.3, 0.6), (1.7, 2.2), (2.9, 0.05)] {
        assert_eq!(
            perlin.sample_2d_tiled(x, y, 4.0, 4.0),
            perlin.sample_2d(x, y)
        );
        assert_eq!(value.sample_2d_tiled(x, y, 4.0, 4.0), value.sample_2d(x, y));
    }
    // Worley also searches neighbouring cells, so only points whose
    // neighbourhood stays inside the tile agree.
    assert_eq!(
        worley.sample_2d_tiled(4.5, 4.5, 9.0, 9.0),
        worley.sample_2d(4.5, 4.5)
    );
}

/// A period that isn't positive leaves its axis unwrapped (a noise op
/// with `scale_x = 0` samples a constant x).
#[test]
fn tiling_with_no_period_is_plain() {
    let p = PerlinNoise::new(2);
    let s = SimplexNoise::new(2);
    for (x, y) in [(0.3, 0.6), (5.7, -2.2)] {
        assert_eq!(p.sample_2d_tiled(x, y, 0.0, 0.0), p.sample_2d(x, y));
        assert_eq!(s.sample_2d_tiled(x, y, 0.0, 0.0), s.sample_2d(x, y));
    }
}

/// The tiled Musgrave and Voronoi samplers repeat at the unit tile.
#[test]
fn tiled_musgrave_and_voronoi_repeat() {
    let src = SimplexNoise::new(6);
    for kind in [
        MusgraveType::Fbm,
        MusgraveType::Multifractal,
        MusgraveType::RidgedMultifractal,
        MusgraveType::HybridMultifractal,
        MusgraveType::HeteroTerrain,
    ] {
        let m = |x: f64, y: f64| {
            musgrave_sample_tiled(&src, x, y, 1.5, 1.0, kind, 2.6, 4, 2.1, 1.0, 1.0, 2.0)
        };
        for (x, y) in [(0.1, 0.2), (0.77, 0.5)] {
            assert!((m(x, y) - m(x + 1.5, y + 1.0)).abs() < 1e-6, "{kind:?}");
        }
    }
    for feature in [VoronoiFeature::F1, VoronoiFeature::DistanceToEdge] {
        let v = |x: f64, y: f64| {
            voronoi_sample_tiled(x, y, 9, 5.4, 1.0, feature, VoronoiMetric::Euclidean)
        };
        for (x, y) in [(0.01, 0.3), (0.99, 0.98)] {
            let (a, b) = (v(x, y), v(x + 1.0, y - 1.0));
            assert!((a.value - b.value).abs() < 1e-9 && a.cell_id == b.cell_id);
        }
    }
}
