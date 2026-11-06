//! Look at a glyph set: write its atlas, a coverage table and a lit sphere
//! drawn through each ramp, all on the CPU (#264).
//!
//! ```bash
//! cargo run -p flint-render --example glyph_atlas -- out/            # the built-in set
//! cargo run -p flint-render --example glyph_atlas -- out/ fonts/terminal.glyphs.toml
//! ```
//!
//! Writes `<set>_atlas.png` (4x, nearest), `<set>_coverage.json` (each
//! glyph's measured ink) and `<set>_<ramp>.png` (a sphere and a floor
//! gradient, one glyph per cell, with edge glyphs on the silhouette).

use flint_render::{GlyphAtlas, GlyphRamp, GlyphSet};
use std::path::{Path, PathBuf};

const COLS: u32 = 72;
const ROWS: u32 = 36;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| "glyph_out".into()));
    std::fs::create_dir_all(&out).expect("create the output folder");
    let set = match args.next() {
        Some(path) => GlyphSet::load(Path::new(&path)).unwrap_or_else(|e| panic!("{e}")),
        None => (*GlyphSet::builtin()).clone(),
    };
    let atlas = set.atlas();

    let img = atlas.to_image();
    let big = image::imageops::resize(
        &img,
        img.width() * 4,
        img.height() * 4,
        image::imageops::FilterType::Nearest,
    );
    big.save(out.join(format!("{}_atlas.png", set.name())))
        .unwrap();

    let coverage: Vec<String> = atlas
        .glyphs()
        .iter()
        .map(|c| {
            format!(
                "{{\"glyph\":{},\"coverage\":{:.4}}}",
                serde_json_escape(*c),
                atlas.coverage(*c).unwrap()
            )
        })
        .collect();
    let ramps: Vec<String> = set
        .ramps()
        .map(|r| {
            let glyphs: String = r.glyphs().iter().collect();
            let cov: Vec<String> = r
                .glyphs()
                .iter()
                .map(|c| format!("{:.4}", atlas.coverage(*c).unwrap()))
                .collect();
            format!(
                "{{\"name\":\"{}\",\"glyphs\":{},\"coverage\":[{}]}}",
                r.name(),
                json_string(&glyphs),
                cov.join(",")
            )
        })
        .collect();
    std::fs::write(
        out.join(format!("{}_coverage.json", set.name())),
        format!(
            "{{\"set\":\"{}\",\"cell\":{:?},\"size\":{:?},\"glyphs\":[{}],\"ramps\":[{}]}}",
            set.name(),
            atlas.cell_size(),
            atlas.size(),
            coverage.join(","),
            ramps.join(",")
        ),
    )
    .unwrap();

    for ramp in set.ramps() {
        let specimen = draw_specimen(atlas, ramp, &set);
        specimen
            .save(out.join(format!("{}_{}.png", set.name(), ramp.name())))
            .unwrap();
    }
    println!(
        "{}: {} glyphs in {}x{} cells, atlas {}x{}, ramps {}",
        set.name(),
        atlas.glyphs().len(),
        atlas.cell_size()[0],
        atlas.cell_size()[1],
        atlas.size()[0],
        atlas.size()[1],
        set.ramps().map(|r| r.name()).collect::<Vec<_>>().join(", ")
    );
}

const SPHERE: (f32, f32, f32) = (-0.25, 0.08, 0.74);

/// Scene space of a point in the grid (0..1 across, 0..1 down), with the
/// cells' aspect so the sphere stays round.
fn to_scene(u: f32, v: f32, cell_aspect: f32) -> (f32, f32) {
    let aspect = COLS as f32 / ROWS as f32 * cell_aspect;
    ((u - 0.5) * 2.0 * aspect, (0.5 - v) * 2.0)
}

fn inside(x: f32, y: f32) -> bool {
    let (cx, cy, r) = SPHERE;
    (x - cx).powi(2) + (y - cy).powi(2) < r * r
}

/// Luminance of a lit sphere over a floor gradient at a cell centre, plus
/// the silhouette's direction when the cell's corners straddle the rim.
fn scene(col: u32, row: u32, cell_aspect: f32) -> (f32, Option<f32>) {
    let at = |fx: f32, fy: f32| {
        to_scene(
            (col as f32 + fx) / COLS as f32,
            (row as f32 + fy) / ROWS as f32,
            cell_aspect,
        )
    };
    let corners = [at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0), at(1.0, 1.0)];
    let ins = corners.iter().filter(|(x, y)| inside(*x, *y)).count();
    let (x, y) = at(0.5, 0.5);
    let (cx, cy, r) = SPHERE;
    let (dx, dy) = (x - cx, y - cy);
    let d = (dx * dx + dy * dy).sqrt();
    if ins > 0 && ins < 4 {
        // The rim runs perpendicular to the radius.
        return (0.0, Some(dy.atan2(dx) + std::f32::consts::FRAC_PI_2));
    }
    if d < r {
        let nz = (1.0 - (d / r).powi(2)).max(0.0).sqrt();
        let n = [dx / r, dy / r, nz];
        let l = [-0.5f32, 0.6, 0.62];
        let ll = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        let diffuse = ((n[0] * l[0] + n[1] * l[1] + n[2] * l[2]) / ll).max(0.0);
        let spec = diffuse.powf(24.0) * 0.5;
        ((0.04 + 0.82 * diffuse + spec).min(1.0), None)
    } else {
        // A floor that brightens toward the viewer, with the sphere's shadow.
        let floor = ((-y - 0.1) * 0.6).clamp(0.0, 1.0);
        let (sx, sy) = (x - (cx + 0.45), (y + 0.78) * 2.6);
        let shadow = ((sx * sx + sy * sy).sqrt() / 0.8).clamp(0.0, 1.0);
        (floor * (0.35 + 0.65 * shadow) * 0.55, None)
    }
}

fn draw_specimen(atlas: &GlyphAtlas, ramp: &GlyphRamp, set: &GlyphSet) -> image::RgbImage {
    let [cw, ch] = atlas.cell_size();
    let scale = 2;
    let (w, h) = (COLS * cw * scale, ROWS * ch * scale);
    let paper = [14u8, 16, 20];
    let ink = [236u8, 226, 200];
    let edge_ink = [255u8, 176, 92];
    let mut out = image::RgbImage::from_pixel(w, h, image::Rgb(paper));
    let edges = set.edges();
    for row in 0..ROWS {
        for col in 0..COLS {
            let (lum, edge) = scene(col, row, cw as f32 / ch as f32);
            let (glyph, colour) = match edge {
                Some(angle) => (edges.for_angle(angle), edge_ink),
                None => (ramp.pick(lum), ink),
            };
            for y in 0..ch {
                for x in 0..cw {
                    let c = atlas.sample(glyph, x, y).unwrap_or(0) as f32 / 255.0;
                    if c == 0.0 {
                        continue;
                    }
                    let px: [u8; 3] = std::array::from_fn(|i| {
                        (paper[i] as f32 + (colour[i] as f32 - paper[i] as f32) * c) as u8
                    });
                    for sy in 0..scale {
                        for sx in 0..scale {
                            out.put_pixel(
                                (col * cw + x) * scale + sx,
                                (row * ch + y) * scale + sy,
                                image::Rgb(px),
                            );
                        }
                    }
                }
            }
        }
    }
    out
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn serde_json_escape(c: char) -> String {
    json_string(&c.to_string())
}
