//! A glyph set's specimen (#264, #284): a lit sphere over a floor drawn one
//! glyph a cell through a ramp, edge glyphs on the silhouette, all on the
//! CPU. The `glyph_atlas` example and `flint gen <set>.glyphs.toml` share it,
//! along with each ramp's measured ink, so both say the same thing about a set.

use super::{GlyphAtlas, GlyphRamp, GlyphSet};

/// Cells across and down a specimen.
pub const SPECIMEN_COLS: u32 = 72;
pub const SPECIMEN_ROWS: u32 = 36;

/// Paper, ink and edge ink of a specimen (display values).
pub const SPECIMEN_PAPER: [u8; 3] = [14, 16, 20];
pub const SPECIMEN_INK: [u8; 3] = [236, 226, 200];
pub const SPECIMEN_EDGE_INK: [u8; 3] = [255, 176, 92];

/// What a specimen cell holds: a ramp glyph for a luminance, or an edge
/// glyph along the silhouette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpecimenCell {
    Tone(char),
    Edge(char),
}

impl SpecimenCell {
    pub fn glyph(self) -> char {
        match self {
            SpecimenCell::Tone(c) | SpecimenCell::Edge(c) => c,
        }
    }
}

const SPHERE: (f32, f32, f32) = (-0.25, 0.08, 0.74);

/// Scene space of a point in the grid (0..1 across, 0..1 down), with the
/// cells' aspect so the sphere stays round.
fn to_scene(u: f32, v: f32, cell_aspect: f32) -> (f32, f32) {
    let aspect = SPECIMEN_COLS as f32 / SPECIMEN_ROWS as f32 * cell_aspect;
    ((u - 0.5) * 2.0 * aspect, (0.5 - v) * 2.0)
}

fn inside(x: f32, y: f32) -> bool {
    let (cx, cy, r) = SPHERE;
    (x - cx).powi(2) + (y - cy).powi(2) < r * r
}

/// Luminance of a lit sphere over a floor gradient at a cell centre, plus
/// the silhouette's direction when the cell's corners straddle the rim.
pub fn specimen_scene(col: u32, row: u32, cell_aspect: f32) -> (f32, Option<f32>) {
    let at = |fx: f32, fy: f32| {
        to_scene(
            (col as f32 + fx) / SPECIMEN_COLS as f32,
            (row as f32 + fy) / SPECIMEN_ROWS as f32,
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

/// The specimen's cells, a row of [`SPECIMEN_COLS`] at a time.
pub fn specimen_cells(set: &GlyphSet, ramp: &GlyphRamp) -> Vec<SpecimenCell> {
    let [cw, ch] = set.atlas().cell_size();
    let aspect = cw as f32 / ch as f32;
    let edges = set.edges();
    let mut cells = Vec::with_capacity((SPECIMEN_COLS * SPECIMEN_ROWS) as usize);
    for row in 0..SPECIMEN_ROWS {
        for col in 0..SPECIMEN_COLS {
            cells.push(match specimen_scene(col, row, aspect) {
                (_, Some(angle)) => SpecimenCell::Edge(edges.for_angle(angle)),
                (lum, None) => SpecimenCell::Tone(ramp.pick(lum)),
            });
        }
    }
    cells
}

/// The specimen as text, a line a row: what an agent reads in place of the
/// picture.
pub fn specimen_text(set: &GlyphSet, ramp: &GlyphRamp) -> String {
    let cells = specimen_cells(set, ramp);
    let mut out = String::with_capacity(cells.len() + SPECIMEN_ROWS as usize);
    for row in cells.chunks(SPECIMEN_COLS as usize) {
        let line: String = row.iter().map(|c| c.glyph()).collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// The specimen drawn from the atlas, `scale` image pixels a glyph pixel.
pub fn draw_specimen(set: &GlyphSet, ramp: &GlyphRamp, scale: u32) -> image::RgbImage {
    let atlas: &GlyphAtlas = set.atlas();
    let [cw, ch] = atlas.cell_size();
    let scale = scale.max(1);
    let (w, h) = (SPECIMEN_COLS * cw * scale, SPECIMEN_ROWS * ch * scale);
    let paper = SPECIMEN_PAPER;
    let mut out = image::RgbImage::from_pixel(w, h, image::Rgb(paper));
    let cells = specimen_cells(set, ramp);
    for (i, cell) in cells.iter().enumerate() {
        let (col, row) = (i as u32 % SPECIMEN_COLS, i as u32 / SPECIMEN_COLS);
        let (glyph, colour) = match cell {
            SpecimenCell::Edge(c) => (*c, SPECIMEN_EDGE_INK),
            SpecimenCell::Tone(c) => (*c, SPECIMEN_INK),
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
    out
}

/// A ramp's glyphs' measured ink, darkest step first.
pub fn ramp_coverage(set: &GlyphSet, ramp: &GlyphRamp) -> Vec<f32> {
    ramp.glyphs()
        .iter()
        .map(|c| set.atlas().coverage(*c).unwrap_or(0.0))
        .collect()
}

/// Whether ink never falls from one step to the next.
pub fn is_rising(coverage: &[f32]) -> bool {
    coverage.windows(2).all(|w| w[1] >= w[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_specimen_text_is_the_specimen_grid() {
        let set = GlyphSet::builtin();
        let ramp = set.default_ramp();
        let text = specimen_text(&set, ramp);
        assert_eq!(text.lines().count(), SPECIMEN_ROWS as usize);
        // The sphere's lit side reaches the ramp's brightest glyph, and the
        // rim draws edge glyphs.
        let top = *ramp.glyphs().last().unwrap();
        assert!(text.contains(top), "no {top:?} in\n{text}");
        let edges = set.edges().all();
        assert!(text.chars().any(|c| edges.contains(&c)));
    }

    #[test]
    fn a_ramp_rises_while_its_ink_never_falls() {
        assert!(is_rising(&[0.0, 0.1, 0.1, 0.5]));
        assert!(!is_rising(&[0.0, 0.2, 0.1]));
        let set = GlyphSet::builtin();
        let blocks = set.ramp("blocks").unwrap();
        assert!(is_rising(&ramp_coverage(&set, blocks)));
    }
}
