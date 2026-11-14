#![allow(clippy::needless_range_loop)] // pixel loops index several images at once

use super::*;
use std::path::PathBuf;

/// A context whose fonts exist, at `ppp` pixels per point.
fn context(ppp: f32) -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(ppp);
    let _ = ctx.run_ui(Default::default(), |_| {});
    ctx
}

/// Lay `text` out at `size` points and rasterise `spec` round it.
fn halo(
    ctx: &egui::Context,
    text: &str,
    size: f32,
    spec: HaloSpec,
) -> (Arc<egui::Galley>, HaloImage) {
    halo_within(ctx, text, size, spec, 8192)
}

/// [`halo`] on a device whose textures are at most `max_side` a side.
fn halo_within(
    ctx: &egui::Context,
    text: &str,
    size: f32,
    spec: HaloSpec,
    max_side: usize,
) -> (Arc<egui::Galley>, HaloImage) {
    let mut out = None;
    let _ = ctx.run_ui(Default::default(), |ui| {
        let ctx = &ui.ctx().clone();
        let galley = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
            )
        });
        let image = ctx.fonts_mut(|f| rasterise(&galley, &f.image(), spec, max_side));
        out = Some((galley, image.expect("a halo")));
    });
    out.unwrap()
}

/// The glyphs' own coverage in the halo's frame, for checks and goldens.
fn coverage(ctx: &egui::Context, galley: &egui::Galley, halo: &HaloImage) -> Vec<f32> {
    let [w, h] = halo.image.size;
    let mut cov = vec![0.0; w * h];
    ctx.fonts_mut(|f| {
        let img = f.image();
        for q in glyph_quads(galley, galley.pixels_per_point) {
            for y in 0..q.size[1] {
                for x in 0..q.size[0] {
                    let dx = (q.dst[0] - halo.origin_px[0]) as usize + x;
                    let dy = (q.dst[1] - halo.origin_px[1]) as usize + y;
                    let c =
                        img.pixels[(q.uv[1] + y) * img.size[0] + q.uv[0] + x].a() as f32 / 255.0;
                    cov[dy * w + dx] = f32::max(cov[dy * w + dx], c);
                }
            }
        }
    });
    cov
}

#[test]
fn distance_transform_matches_brute_force() {
    let (w, h) = (23, 17);
    // A few blobs and a lone pixel.
    let cov: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let blob = ((x - 6.0).powi(2) + (y - 5.0).powi(2)) < 9.0;
            let bar = (14..17).contains(&(x as i32)) && (3..14).contains(&(y as i32));
            let dot = x == 20.0 && y == 15.0;
            if blob || bar || dot {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let d = distance_to_ink(&cov, w, h);
    for y in 0..h {
        for x in 0..w {
            let mut best = f32::MAX;
            for j in 0..h {
                for i in 0..w {
                    if cov[j * w + i] >= 0.5 {
                        let dd =
                            ((x as f32 - i as f32).powi(2) + (y as f32 - j as f32).powi(2)).sqrt();
                        best = best.min(dd);
                    }
                }
            }
            assert!(
                (d[y * w + x] - best).abs() < 1e-3,
                "({x},{y}): {} vs {best}",
                d[y * w + x]
            );
        }
    }
}

#[test]
fn three_boxes_have_the_gaussians_variance() {
    for sigma in [1.0f32, 2.5, 4.0, 9.0, 20.0] {
        let r = box_radii(sigma);
        // A box of radius r has variance ((2r+1)^2 - 1) / 12; three add.
        let var: f32 = r
            .iter()
            .map(|&r| (((2 * r + 1).pow(2) - 1) as f32) / 12.0)
            .sum();
        assert!(
            (var.sqrt() - sigma).abs() < 0.6,
            "sigma {sigma}: boxes {r:?} give {}",
            var.sqrt()
        );
    }
}

#[test]
fn a_blur_keeps_its_mass() {
    let (w, h) = (64, 48);
    let mut img = vec![0.0; w * h];
    img[24 * w + 32] = 1.0;
    gaussian_blur(&mut img, w, h, 3.0);
    let sum: f32 = img.iter().sum();
    assert!((sum - 1.0).abs() < 1e-4, "{sum}");
    assert!(img[24 * w + 32] < 0.05 && img[24 * w + 32] > img[24 * w + 38]);
}

/// The issue's done-when: a 10 pt stroke at 0.3 alpha round 60 pt text is
/// one even band. Every pixel clear of the glyphs and well inside the band
/// is exactly the stroke's colour, at one alpha: no copy shows through and
/// no overlap is darker; and the band ends at the same distance everywhere.
#[test]
fn a_wide_translucent_stroke_is_an_even_band() {
    let ctx = context(1.0);
    let stroke = ([0.2, 0.9, 1.0, 0.3], 10.0);
    let (galley, img) = halo(
        &ctx,
        "60",
        60.0,
        HaloSpec {
            stroke: Some(stroke),
            glow: None,
        },
    );
    let cov = coverage(&ctx, &galley, &img);
    let [w, h] = img.image.size;
    let dist = distance_to_ink(&cov, w, h);
    let want = to_premul(&stroke.0);
    let (mut inside, mut outside) = (0, 0);
    for i in 0..w * h {
        let px = img.image.pixels[i].to_array();
        if cov[i] == 0.0 && dist[i] > 1.0 && dist[i] < 9.5 {
            assert_eq!(px, want, "pixel {i} at {} px from ink", dist[i]);
            inside += 1;
        }
        if dist[i] >= 11.0 {
            assert_eq!(
                px[3], 0,
                "pixel {i} at {} px from ink is past the band",
                dist[i]
            );
            outside += 1;
        }
    }
    assert!(
        inside > 1500 && outside > 1000,
        "{inside} band pixels, {outside} outside"
    );
}

#[test]
fn a_glow_fades_out_by_its_radius() {
    let ctx = context(1.0);
    let glow = ([1.0, 0.3, 0.6, 1.0], 16.0);
    let (galley, img) = halo(
        &ctx,
        "I",
        60.0,
        HaloSpec {
            stroke: None,
            glow: Some(glow),
        },
    );
    let cov = coverage(&ctx, &galley, &img);
    let [w, h] = img.image.size;
    let dist = distance_to_ink(&cov, w, h);
    let alpha_at = |lo: f32, hi: f32| {
        let v: Vec<f32> = (0..w * h)
            .filter(|&i| dist[i] >= lo && dist[i] < hi)
            .map(|i| f32::from(img.image.pixels[i].a()))
            .collect();
        v.iter().sum::<f32>() / v.len().max(1) as f32
    };
    let near = alpha_at(1.0, 3.0);
    let mid = alpha_at(7.0, 9.0);
    let far = alpha_at(17.0, 19.0);
    assert!(near > mid && mid > far, "{near} {mid} {far}");
    assert!(near > 60.0, "the glow is bright at the glyph: {near}");
    assert!(far < 8.0, "and nearly gone past its radius: {far}");
}

#[test]
fn hidpi_halos_are_drawn_at_the_screens_pixels() {
    let spec = HaloSpec {
        stroke: Some(([0.0, 0.0, 0.0, 1.0], 4.0)),
        glow: None,
    };
    let (_, one) = halo(&context(1.0), "Ab", 30.0, spec);
    let (_, two) = halo(&context(2.0), "Ab", 30.0, spec);
    let [w1, h1] = one.image.size;
    let [w2, h2] = two.image.size;
    // Twice the pixels, less the fixed two-pixel margin each side.
    assert!(
        ((w2 - 4) as f32 / (w1 - 4) as f32 - 2.0).abs() < 0.12,
        "{w1} -> {w2}"
    );
    assert!(
        ((h2 - 4) as f32 / (h1 - 4) as f32 - 2.0).abs() < 0.12,
        "{h1} -> {h2}"
    );
}

#[test]
fn thin_strokes_stay_stamped_and_wide_ones_and_glows_dont() {
    let c = [0.0, 0.0, 0.0, 1.0];
    assert!(!wants_halo(None, None));
    assert!(!wants_halo(Some((c, 1.0)), None));
    assert!(!wants_halo(Some((c, STAMPED_STROKE_MAX)), None));
    assert!(wants_halo(Some((c, 2.5)), None));
    assert!(
        !wants_halo(Some(([0.0; 4], 8.0)), None),
        "an invisible stroke draws nothing"
    );
    assert!(wants_halo(None, Some((c, 6.0))));
    assert!(!wants_halo(None, Some((c, 0.0))));
}

#[test]
fn a_still_label_makes_one_texture_and_a_dropped_one_frees_it() {
    let ctx = context(1.0);
    let mut halos = TextHalos::default();
    let spec = HaloSpec {
        stroke: Some(([1.0; 4], 6.0)),
        glow: Some(([1.0; 4], 8.0)),
    };
    let frame = |halos: &mut TextHalos, text: Option<&str>| {
        let _ = ctx.run_ui(Default::default(), |ui| {
            let ctx = &ui.ctx().clone();
            halos.begin_frame();
            if let Some(text) = text {
                let font = egui::FontId::proportional(40.0);
                let galley = ctx
                    .fonts_mut(|f| f.layout_no_wrap(text.into(), font.clone(), egui::Color32::RED));
                let shape = halos.shape(ctx, &galley, (text, &font), egui::pos2(10.3, 20.0), spec);
                let Some(egui::Shape::Mesh(m)) = shape else {
                    panic!("an image mesh")
                };
                // On a whole pixel, and reaching left of the galley.
                assert_eq!(m.vertices[0].pos.x.fract(), 0.0);
                assert!(m.vertices[0].pos.x < 10.0);
            }
        });
    };
    frame(&mut halos, Some("42"));
    frame(&mut halos, Some("42"));
    assert_eq!(halos.len(), 1);
    frame(&mut halos, Some("43"));
    assert_eq!(halos.len(), 2, "last frame's is kept a frame");
    frame(&mut halos, Some("43"));
    assert_eq!(halos.len(), 1);
    frame(&mut halos, None);
    frame(&mut halos, None);
    assert_eq!(halos.len(), 0);
}

/// #657: a glow round lettering too wide for the device's textures is drawn
/// smaller, over the same place, instead of asking for a texture the GPU
/// refuses (which ended the run).
#[test]
fn a_halo_past_the_texture_limit_is_drawn_smaller_in_the_same_place() {
    let ctx = context(1.0);
    let spec = HaloSpec {
        stroke: Some(([0.2, 0.85, 1.0, 0.6], 4.0)),
        glow: Some(([0.2, 0.85, 1.0, 0.45], 10.0)),
    };
    let (_, full) = halo_within(&ctx, "AFTERGLOW", 300.0, spec, 8192);
    assert_eq!(full.scale, 1);
    let [fw, fh] = full.image.size;
    assert!(fw > 1500, "the label is wider than the limit: {fw}");

    let limit = 1024;
    let (_, small) = halo_within(&ctx, "AFTERGLOW", 300.0, spec, limit);
    let [sw, sh] = small.image.size;
    let k = small.scale as i32;
    assert!(sw <= limit && sh <= limit, "{sw}x{sh} fits {limit}");
    assert!(k as usize >= fw.div_ceil(limit), "scale {k}");
    // Its footprint covers the full picture's, give or take a block.
    let (f0, f1) = (
        full.origin_px,
        [full.origin_px[0] + fw as i32, full.origin_px[1] + fh as i32],
    );
    let (s0, s1) = (
        small.origin_px,
        [
            small.origin_px[0] + sw as i32 * k,
            small.origin_px[1] + sh as i32 * k,
        ],
    );
    for i in 0..2 {
        assert!(
            s0[i] <= f0[i] + k && s1[i] >= f1[i] - k,
            "{s0:?}..{s1:?} over {f0:?}..{f1:?}"
        );
    }
    // And it still glows.
    let peak = small.image.pixels.iter().map(|p| p.a()).max().unwrap();
    assert!(peak > 100, "peak alpha {peak}");

    // Through the cache on such a device: one texture within the limit,
    // painted over the full size.
    let mut halos = TextHalos::default();
    let input = egui::RawInput {
        max_texture_side: Some(limit),
        ..Default::default()
    };
    let _ = ctx.run_ui(input, |ui| {
        let ctx = &ui.ctx().clone();
        halos.begin_frame();
        let font = egui::FontId::proportional(300.0);
        let galley = ctx.fonts_mut(|f| {
            f.layout_no_wrap("AFTERGLOW".into(), font.clone(), egui::Color32::WHITE)
        });
        let Some(egui::Shape::Mesh(m)) = halos.shape(
            ctx,
            &galley,
            ("AFTERGLOW", &font),
            egui::pos2(0.0, 0.0),
            spec,
        ) else {
            panic!("an image mesh")
        };
        let width = m.vertices.iter().map(|v| v.pos.x).fold(f32::MIN, f32::max)
            - m.vertices.iter().map(|v| v.pos.x).fold(f32::MAX, f32::min);
        assert!(
            (width - fw as f32).abs() <= 2.0 * k as f32,
            "{width} vs {fw}"
        );
    });
}

fn to_premul(c: &[f32; 4]) -> [u8; 4] {
    egui::Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (c[3] * 255.0) as u8,
    )
    .to_array()
}

/// The golden: neon "60" — a 10 pt stroke at 0.3 alpha and a 14 pt glow
/// round 60 pt text — composited the way the HUD paints it (dark ground,
/// halo, then the text over it), at 2 pixels a point. The halo is the CPU's
/// own work, so this is the picture the GPU is handed, pixel for pixel.
/// `FLINT_BLESS=1` rewrites it.
#[test]
fn neon_numeral_golden() {
    let ctx = context(2.0);
    let face = [1.0, 0.97, 0.9, 1.0];
    let spec = HaloSpec {
        stroke: Some(([0.25, 0.85, 1.0, 0.3], 10.0)),
        glow: Some(([0.2, 0.6, 1.0, 0.9], 14.0)),
    };
    let (galley, img) = halo(&ctx, "60", 60.0, spec);
    let cov = coverage(&ctx, &galley, &img);
    let [w, h] = img.image.size;
    let ground = [12.0f32, 10.0, 24.0];
    let face_p = to_premul(&face).map(f32::from);
    let mut rgb = image::RgbImage::new(w as u32, h as u32);
    for i in 0..w * h {
        let hp = img.image.pixels[i].to_array().map(f32::from);
        let mut c = [0, 1, 2].map(|k| hp[k] + ground[k] * (1.0 - hp[3] / 255.0));
        let a = cov[i];
        c = [0, 1, 2].map(|k| face_p[k] * a + c[k] * (1.0 - a));
        rgb.put_pixel(
            (i % w) as u32,
            (i / w) as u32,
            image::Rgb(c.map(|v| v.round().clamp(0.0, 255.0) as u8)),
        );
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/text-halo-neon-60.png");
    if std::env::var_os("FLINT_BLESS").is_some() || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        rgb.save(&path).unwrap();
        if std::env::var_os("FLINT_BLESS").is_none() {
            panic!(
                "no golden at {}: wrote one, check it and commit it",
                path.display()
            );
        }
        return;
    }
    let golden = image::open(&path).unwrap().to_rgb8();
    assert_eq!(
        golden.dimensions(),
        rgb.dimensions(),
        "the halo's size moved"
    );
    // Font rasterisation is CPU float work: allow a level here and there.
    let mut off = 0usize;
    let mut worst = 0u8;
    for (a, b) in golden.pixels().zip(rgb.pixels()) {
        let d = (0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap();
        worst = worst.max(d);
        if d > 2 {
            off += 1;
        }
    }
    let share = off as f32 / (w * h) as f32;
    assert!(
        share < 0.001 && worst < 32,
        "{off} pixels off by more than 2 (worst {worst})"
    );
}

#[test]
fn shrinking_and_growing_keep_a_flat_field_flat() {
    let (w, h) = (37, 22);
    let img = vec![0.4f32; w * h];
    for s in 1..=4 {
        let small = shrink(&img, w, h, s);
        let back = grow(&small, w.div_ceil(s), h.div_ceil(s), s, w, h);
        // Edge blocks past the image are partly empty, so only the
        // interior is held flat.
        for y in s..h - 2 * s {
            for x in s..w - 2 * s {
                assert!(
                    (back[y * w + x] - 0.4).abs() < 1e-5,
                    "s {s} at ({x},{y}): {}",
                    back[y * w + x]
                );
            }
        }
    }
}
