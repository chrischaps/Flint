//! Wide strokes and glows round text, as one picture (#626).
//!
//! A thin stroke is the text drawn again at eight compass offsets, which
//! reads as an outline up to about [`STAMPED_STROKE_MAX`] points. Wider, the
//! copies come apart into a blocky cloud, and since each is blended on its
//! own a translucent stroke is denser where they overlap. So a stroke wider
//! than that, and any glow, is drawn here instead: the galley's glyphs are
//! copied out of egui's font atlas into a coverage image at the screen's
//! pixel size, the stroke is a band of the distance to the glyphs (round at
//! every width), the glow a blur of glyphs and stroke together, and the two
//! are composited into one texture that's painted once, under the text.
//! Overlaps can't darken because there are none.
//!
//! Pictures are cached by everything that shapes them (text, font, spacing,
//! stroke, glow and the pixel scale; not the text's own colour), and one
//! unused for a frame is dropped, so a label that changes every frame makes
//! one texture a frame and a still one makes none.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The widest stroke, in points, still drawn as eight offset copies.
/// Wider ones are rasterised.
pub(super) const STAMPED_STROKE_MAX: f32 = 2.0;

/// The most pictures kept at once; past this the cache starts again.
const MAX_ENTRIES: usize = 512;

/// The eight compass offsets of a stamped stroke `w` wide.
pub(super) fn stamp_offsets(w: f32) -> [(f32, f32); 8] {
    [
        (-w, 0.0),
        (w, 0.0),
        (0.0, -w),
        (0.0, w),
        (-w, -w),
        (w, -w),
        (-w, w),
        (w, w),
    ]
}

/// Whether a stroke this wide (and a glow, if any) needs a picture.
pub(super) fn wants_halo(stroke: Option<([f32; 4], f32)>, glow: Option<([f32; 4], f32)>) -> bool {
    let wide = matches!(stroke, Some((c, w)) if w > STAMPED_STROKE_MAX && c[3] > 0.0);
    let glows = matches!(glow, Some((c, r)) if r > 0.0 && c[3] > 0.0);
    wide || glows
}

/// What a halo is made of.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HaloSpec {
    /// Colour and width in points of the band round the glyphs.
    pub stroke: Option<([f32; 4], f32)>,
    /// Colour and reach in points of the blur under it.
    pub glow: Option<([f32; 4], f32)>,
}

/// A halo rasterised: premultiplied sRGBA pixels and where they sit.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct HaloImage {
    pub image: egui::ColorImage,
    /// The image's top-left corner from the galley's origin, in pixels.
    pub origin_px: [i32; 2],
}

/// The cache of halo pictures, one per distinct label.
#[derive(Default)]
pub(super) struct TextHalos {
    entries: HashMap<u64, Entry>,
    frame: u64,
}

struct Entry {
    texture: egui::TextureHandle,
    origin_px: [i32; 2],
    size_px: [usize; 2],
    used: u64,
}

impl TextHalos {
    /// Start a frame: drop what the last one didn't use.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        let keep_from = self.frame.saturating_sub(1);
        self.entries.retain(|_, e| e.used >= keep_from);
    }

    /// How many pictures are held (tests and stats).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The shape that paints `spec` under `galley` drawn at `pos`, or
    /// `None` when there's nothing to draw. `key` names the layout
    /// (text, font, spacing) without its colour.
    pub fn shape(
        &mut self,
        ctx: &egui::Context,
        galley: &Arc<egui::Galley>,
        key: impl Hash,
        pos: egui::Pos2,
        spec: HaloSpec,
    ) -> Option<egui::Shape> {
        let ppp = galley.pixels_per_point;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut h);
        hash_spec(&spec, &mut h);
        ppp.to_bits().hash(&mut h);
        let id = h.finish();

        if !self.entries.contains_key(&id) {
            let halo = ctx.fonts(|f| {
                let atlas = f.texture_atlas();
                let atlas = atlas.lock();
                rasterise(galley, atlas.image(), spec)
            })?;
            if self.entries.len() >= MAX_ENTRIES {
                self.entries.clear();
            }
            let size_px = halo.image.size;
            let texture = ctx.load_texture(
                format!("flint_text_halo_{id:016x}"),
                halo.image,
                egui::TextureOptions::LINEAR,
            );
            self.entries.insert(
                id,
                Entry {
                    texture,
                    origin_px: halo.origin_px,
                    size_px,
                    used: self.frame,
                },
            );
        }
        let e = self.entries.get_mut(&id)?;
        e.used = self.frame;
        // The tessellator puts a galley on a whole pixel; so does this.
        let snap = |v: f32| (v * ppp).round() / ppp;
        let min = egui::pos2(
            snap(pos.x) + e.origin_px[0] as f32 / ppp,
            snap(pos.y) + e.origin_px[1] as f32 / ppp,
        );
        let rect = egui::Rect::from_min_size(
            min,
            egui::vec2(e.size_px[0] as f32 / ppp, e.size_px[1] as f32 / ppp),
        );
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        Some(egui::Shape::image(
            e.texture.id(),
            rect,
            uv,
            egui::Color32::WHITE,
        ))
    }
}

fn hash_spec(spec: &HaloSpec, h: &mut impl Hasher) {
    for part in [spec.stroke, spec.glow] {
        match part {
            Some((c, w)) => {
                1u8.hash(h);
                for v in c {
                    v.to_bits().hash(h);
                }
                w.to_bits().hash(h);
            }
            None => 0u8.hash(h),
        }
    }
}

/// Rasterise `spec` round `galley`, reading glyph coverage from `atlas`
/// (egui's font atlas, which holds every glyph the galley was laid out
/// with). `None` for a galley with no glyphs or a spec that draws nothing.
pub(super) fn rasterise(
    galley: &egui::Galley,
    atlas: &egui::FontImage,
    spec: HaloSpec,
) -> Option<HaloImage> {
    let ppp = galley.pixels_per_point;
    let stroke = spec.stroke.filter(|(c, w)| *w > 0.0 && c[3] > 0.0);
    let glow = spec.glow.filter(|(c, r)| *r > 0.0 && c[3] > 0.0);
    if stroke.is_none() && glow.is_none() {
        return None;
    }
    let stroke_px = stroke.map_or(0.0, |(_, w)| w * ppp);
    let glow_px = glow.map_or(0.0, |(_, r)| r * ppp);

    // The glyphs' bounds in galley pixels.
    let quads = glyph_quads(galley, ppp);
    if quads.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
    for q in &quads {
        for i in 0..2 {
            lo[i] = lo[i].min(q.dst[i]);
            hi[i] = hi[i].max(q.dst[i] + q.size[i] as i32);
        }
    }
    let pad = (stroke_px + glow_px).ceil() as i32 + 2;
    // The image's origin, in galley pixels, and its size.
    let origin = [lo[0] - pad, lo[1] - pad];
    let w = (hi[0] - lo[0] + 2 * pad) as usize;
    let h = (hi[1] - lo[1] + 2 * pad) as usize;

    // Coverage: each glyph's atlas texels copied 1:1 (a galley's glyphs are
    // laid out at the atlas's own pixel size), overlaps by their max.
    let mut cov = vec![0.0f32; w * h];
    for q in &quads {
        for y in 0..q.size[1] {
            let ty = q.uv[1] + y;
            let dy = (q.dst[1] - origin[1]) as usize + y;
            if ty >= atlas.size[1] || dy >= h {
                continue;
            }
            for x in 0..q.size[0] {
                let tx = q.uv[0] + x;
                let dx = (q.dst[0] - origin[0]) as usize + x;
                if tx >= atlas.size[0] || dx >= w {
                    continue;
                }
                let c = atlas.pixels[ty * atlas.size[0] + tx];
                let d = &mut cov[dy * w + dx];
                *d = d.max(c);
            }
        }
    }

    // The stroke: everything within `stroke_px` of the glyphs' edge, which
    // lies about half a pixel past the centre of the nearest inked pixel.
    // The glyphs' own coverage is kept under it, so their fringe is filled.
    // Only the glyphs' box grown by the stroke can be in the band, so the
    // distances are taken there, not over the glow's margin.
    let band = stroke.map(|_| {
        let reach = (stroke_px.ceil() as usize + 2).min(pad as usize);
        let m = pad as usize - reach;
        let (iw, ih) = (w - 2 * m, h - 2 * m);
        let mut inner = vec![0.0f32; iw * ih];
        for y in 0..ih {
            inner[y * iw..(y + 1) * iw]
                .copy_from_slice(&cov[(y + m) * w + m..(y + m) * w + m + iw]);
        }
        let dist = distance_to_ink(&inner, iw, ih);
        let mut band = vec![0.0f32; w * h];
        for y in 0..ih {
            let row = &mut band[(y + m) * w + m..(y + m) * w + m + iw];
            for (x, b) in row.iter_mut().enumerate() {
                let i = y * iw + x;
                *b = (stroke_px + 1.0 - dist[i]).clamp(0.0, 1.0).max(inner[i]);
            }
        }
        band
    });

    // The glow: a near-gaussian blur (three box passes) of the lit shape,
    // whose standard deviation puts its reach at 2.5 sigma. Raised along a
    // curve that leaves 0 and 1 where they are, so thin letters still glow.
    // A blur has no detail finer than its sigma, so a wide one runs on a
    // smaller copy (a pixel per `s` x `s` block, s <= sigma / 3) and is read
    // back bilinearly: four times fewer pixels at s = 2, sixteen at 4.
    let halo = glow.map(|_| {
        let lit: &[f32] = match &band {
            Some(b) => b,
            None => &cov,
        };
        let sigma = glow_px / 2.5;
        let s = ((sigma / 3.0).floor() as usize).clamp(1, 4);
        let (sw, sh) = (w.div_ceil(s), h.div_ceil(s));
        let mut small = shrink(lit, w, h, s);
        gaussian_blur(&mut small, sw, sh, sigma / s as f32);
        let mut out = if s == 1 {
            small
        } else {
            grow(&small, sw, sh, s, w, h)
        };
        for v in &mut out {
            let b = v.clamp(0.0, 1.0);
            *v = b * (2.0 - b);
        }
        out
    });

    // Premultiplied sRGBA, the stroke over the glow.
    let premul = |c: &[f32; 4]| -> [f32; 4] {
        let p = egui::Color32::from_rgba_unmultiplied(
            (c[0].clamp(0.0, 1.0) * 255.0) as u8,
            (c[1].clamp(0.0, 1.0) * 255.0) as u8,
            (c[2].clamp(0.0, 1.0) * 255.0) as u8,
            (c[3].clamp(0.0, 1.0) * 255.0) as u8,
        );
        p.to_array().map(f32::from)
    };
    let sc = stroke.map_or([0.0; 4], |(c, _)| premul(&c));
    let gc = glow.map_or([0.0; 4], |(c, _)| premul(&c));
    let zero = vec![
        0.0f32;
        if band.is_none() || halo.is_none() {
            w * h
        } else {
            0
        }
    ];
    let b = band.as_deref().unwrap_or(&zero);
    let g = halo.as_deref().unwrap_or(&zero);
    let pixels = b
        .iter()
        .zip(g)
        .map(|(&b, &g)| {
            let keep = 1.0 - sc[3] * b / 255.0;
            let gk = g * keep;
            // Both terms are at least 0 and sum to at most 255.
            let o = |k: usize| (sc[k] * b + gc[k] * gk + 0.5).min(255.0) as u8;
            egui::Color32::from_rgba_premultiplied(o(0), o(1), o(2), o(3))
        })
        .collect();

    Some(HaloImage {
        image: egui::ColorImage {
            size: [w, h],
            pixels,
        },
        origin_px: origin,
    })
}

/// One glyph's texels: where they go in galley pixels, where they come from
/// in the atlas, and how many.
struct Quad {
    dst: [i32; 2],
    uv: [usize; 2],
    size: [usize; 2],
}

/// The galley's glyph quads. A row's glyph vertices come four to a glyph
/// (positions in points from the galley's origin, already on whole pixels;
/// uvs in atlas texels). An italic glyph's shear is dropped.
fn glyph_quads(galley: &egui::Galley, ppp: f32) -> Vec<Quad> {
    let mut out = Vec::new();
    for row in &galley.rows {
        let verts = &row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()];
        for q in verts.as_chunks::<4>().0 {
            let mut pmin = [f32::MAX; 2];
            let (mut umin, mut umax) = ([f32::MAX; 2], [f32::MIN; 2]);
            for v in q {
                let p = [v.pos.x, v.pos.y];
                let u = [v.uv.x, v.uv.y];
                for i in 0..2 {
                    pmin[i] = pmin[i].min(p[i]);
                    umin[i] = umin[i].min(u[i]);
                    umax[i] = umax[i].max(u[i]);
                }
            }
            let size = [
                (umax[0] - umin[0]).round() as usize,
                (umax[1] - umin[1]).round() as usize,
            ];
            if size[0] == 0 || size[1] == 0 {
                continue;
            }
            out.push(Quad {
                dst: [
                    (pmin[0] * ppp).round() as i32,
                    (pmin[1] * ppp).round() as i32,
                ],
                uv: [umin[0].round() as usize, umin[1].round() as usize],
                size,
            });
        }
    }
    out
}

/// Each pixel's Euclidean distance, in pixels, to the nearest pixel at least
/// half inked (0 on one): Felzenszwalb & Huttenlocher's exact transform.
fn distance_to_ink(cov: &[f32], w: usize, h: usize) -> Vec<f32> {
    const FAR: f32 = 1e20;
    // Rows: the input is ink or not, so each pixel's squared distance to
    // the nearest ink in its row is two sweeps, no parabolas.
    let mut d = vec![FAR; w * h];
    for (row, line) in cov.chunks_exact(w).zip(d.chunks_exact_mut(w)) {
        let mut last: Option<usize> = None;
        for (x, c) in row.iter().enumerate() {
            if *c >= 0.5 {
                last = Some(x);
            }
            if let Some(l) = last {
                line[x] = ((x - l) * (x - l)) as f32;
            }
        }
        let mut next: Option<usize> = None;
        for x in (0..w).rev() {
            if row[x] >= 0.5 {
                next = Some(x);
            }
            if let Some(n) = next {
                line[x] = line[x].min(((n - x) * (n - x)) as f32);
            }
        }
    }
    // Columns (transposed, so memory is read in order): the exact
    // transform, a lower envelope of parabolas; a column with no ink in
    // reach of any row stays far.
    let mut t = transpose(&d, w, h);
    let mut out = vec![0.0f32; h];
    let mut v = vec![0usize; h];
    let mut z = vec![0.0f32; h + 1];
    for line in t.chunks_exact_mut(h) {
        if line.iter().all(|&f| f >= FAR) {
            continue;
        }
        edt_1d(line, &mut out, &mut v, &mut z);
        line.copy_from_slice(&out);
    }
    let mut d = transpose(&t, h, w);
    d.iter_mut().for_each(|v| *v = v.sqrt());
    d
}

/// `img` (`w` x `h`, row-major) with rows and columns swapped.
fn transpose(img: &[f32], w: usize, h: usize) -> Vec<f32> {
    const B: usize = 32;
    let mut out = vec![0.0f32; w * h];
    for y0 in (0..h).step_by(B) {
        for x0 in (0..w).step_by(B) {
            for y in y0..(y0 + B).min(h) {
                for x in x0..(x0 + B).min(w) {
                    out[x * h + y] = img[y * w + x];
                }
            }
        }
    }
    out
}

/// `img` shrunk by `s`: each output pixel the mean of an `s` x `s` block
/// (blocks past the edge count what's inside).
fn shrink(img: &[f32], w: usize, h: usize, s: usize) -> Vec<f32> {
    if s == 1 {
        return img.to_vec();
    }
    let (sw, sh) = (w.div_ceil(s), h.div_ceil(s));
    let mut out = vec![0.0f32; sw * sh];
    for y in 0..h {
        let row = &mut out[(y / s) * sw..(y / s + 1) * sw];
        for (x, v) in img[y * w..(y + 1) * w].iter().enumerate() {
            row[x / s] += v;
        }
    }
    let norm = 1.0 / (s * s) as f32;
    out.iter_mut().for_each(|v| *v *= norm);
    out
}

/// `small` (`sw` x `sh`) grown back to `w` x `h` by `s`, bilinearly, each
/// small pixel's value at its block's centre.
fn grow(small: &[f32], sw: usize, sh: usize, s: usize, w: usize, h: usize) -> Vec<f32> {
    let at = |x: isize, y: isize| -> f32 {
        let x = x.clamp(0, sw as isize - 1) as usize;
        let y = y.clamp(0, sh as isize - 1) as usize;
        small[y * sw + x]
    };
    let mut out = vec![0.0f32; w * h];
    let inv = 1.0 / s as f32;
    for y in 0..h {
        let fy = (y as f32 + 0.5) * inv - 0.5;
        let y0 = fy.floor();
        let ty = fy - y0;
        for x in 0..w {
            let fx = (x as f32 + 0.5) * inv - 0.5;
            let x0 = fx.floor();
            let tx = fx - x0;
            let (xi, yi) = (x0 as isize, y0 as isize);
            let top = at(xi, yi) * (1.0 - tx) + at(xi + 1, yi) * tx;
            let bottom = at(xi, yi + 1) * (1.0 - tx) + at(xi + 1, yi + 1) * tx;
            out[y * w + x] = top * (1.0 - ty) + bottom * ty;
        }
    }
    out
}

/// The squared distance transform of one line of sampled function `f`.
fn edt_1d(f: &[f32], out: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    let meet = |q: usize, p: usize| {
        ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32))
    };
    for q in 1..n {
        // z[0] is -inf, so this stops at k = 0 at the latest.
        let mut s = meet(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = meet(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        let dq = q as f32 - p as f32;
        *o = dq * dq + f[p];
    }
}

/// Blur `img` in place by about a gaussian of `sigma` pixels: three box
/// passes each way (Wells, 1986), each a running sum. Box passes commute, so
/// all three run along rows, then along the transposed columns.
fn gaussian_blur(img: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma <= 0.0 || w == 0 || h == 0 {
        return;
    }
    let radii = box_radii(sigma);
    let mut line = vec![0.0f32; w.max(h)];
    let mut boxes = |img: &mut [f32], w: usize| {
        for row in img.chunks_exact_mut(w) {
            for r in radii {
                line[..w].copy_from_slice(row);
                box_line(&line[..w], row, r);
            }
        }
    };
    boxes(img, w);
    let mut t = transpose(img, w, h);
    boxes(&mut t, h);
    img.copy_from_slice(&transpose(&t, h, w));
}

/// Radii of three boxes whose sum approximates a gaussian of `sigma`.
fn box_radii(sigma: f32) -> [usize; 3] {
    // Ideal box width for three passes, split between the two odd widths
    // either side of it so the variance comes out right.
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let m = ((12.0 * sigma * sigma - (3 * wl * wl) as f32 - (12 * wl) as f32 - 9.0)
        / (-4 * wl - 4) as f32)
        .round() as i32;
    [0, 1, 2].map(|i| {
        let width = if i < m { wl } else { wu };
        (width / 2) as usize
    })
}

/// One box pass of radius `r` from `src` into `dst`, zero outside.
fn box_line(src: &[f32], dst: &mut [f32], r: usize) {
    let n = src.len();
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut acc = 0.0f32;
    for v in src.iter().take(r.min(n)) {
        acc += v;
    }
    for i in 0..n {
        if i + r < n {
            acc += src[i + r];
        }
        if i > r {
            acc -= src[i - r - 1];
        }
        dst[i] = acc * norm;
    }
}

#[cfg(test)]
#[path = "text_halo_tests.rs"]
mod tests;
