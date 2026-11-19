//! The form's cards: small two-sided quads on its surface, like scales or
//! flakes, which are what boils.
//!
//! A card rides one segment of the spine rigidly, and Living Props' jitter layer
//! turns it about its own normal on the jitter clock, so the surface keeps
//! shifting while the silhouette holds. `card_tilt` stands cards up off the
//! surface toward the tip: flat cards boil inside the outline, standing ones boil
//! the outline's edge too.

use std::f32::consts::{FRAC_PI_2, TAU};

use flint_core::Vec3;

use super::body::{radius_along, surface, up_the_surface, widest, Lineage, Spine};
use super::{CardLayout, FormParams, RowFaces};
use crate::living::{LeafBinding, RowDrift, RowLine, RowWord, JITTER_CHANNELS};
use crate::types::{BoundingBox, BranchSegment, MaterialData, MeshData, SubMesh, Vertex};
use crate::SeededRng;

/// Vertices one card takes: a front face and a back face.
pub(super) const CARD_VERTICES: usize = 8;

/// Indices one card takes, front and back together.
pub(super) const CARD_INDICES: usize = 12;

/// How far a card's back face sits behind its front, so the two never fight.
const BACK_OFFSET: f32 = 4e-4;

/// The cards in the rest pose, in shade order: card `k` is indices
/// `k * CARD_INDICES ..`, and vertices `k * CARD_VERTICES ..`. With rows,
/// also where each line and word was written, which is what they drift along.
pub(super) fn card_mesh(
    p: &FormParams,
    lin: &Lineage,
    spine: &Spine,
    rng: &mut SeededRng,
) -> (MeshData, Option<RowLayout>) {
    let shades = p.card_shades as usize;
    let mut per_shade: Vec<Vec<[Vertex; CARD_VERTICES]>> = vec![Vec::new(); shades];
    let layout = match p.card_layout {
        CardLayout::Scatter => {
            scatter(p, lin, spine, rng, &mut per_shade);
            None
        }
        CardLayout::Rows => Some(rows(p, lin, spine, rng, &mut per_shade)),
    };
    // Card `k` was written into shade `k % shades`, and `assemble` lays the
    // shades end to end.
    let layout = layout.map(|mut layout| {
        let before: Vec<usize> = (0..shades)
            .scan(0, |sum, s| {
                let at = *sum;
                *sum += per_shade[s].len();
                Some(at)
            })
            .collect();
        for (k, word) in layout.words.iter_mut().enumerate() {
            word.card = before[k % shades] + k / shades;
        }
        layout
    });
    (assemble(p, &per_shade), layout)
}

/// Where rows were written: each line, and each word on it in writing order.
#[derive(Debug, Clone)]
pub(super) struct RowLayout {
    pub lines: Vec<LineAt>,
    pub words: Vec<WordAt>,
}

/// One line of writing: its height, the face it's on and how it reads.
#[derive(Debug, Clone, Copy)]
pub(super) struct LineAt {
    pub u: f32,
    /// `+1` or `-1`: which half of the section, as in [`front_side`].
    pub side: f32,
    /// Half its width, inside the margins.
    pub half: f32,
    /// `+1` if the broad-axis offset grows the way the line reads.
    pub reading: f32,
}

/// One word: its line, where it starts and how long it is, from the line's
/// left end as it reads; and which card it is in the assembled mesh.
#[derive(Debug, Clone, Copy)]
pub(super) struct WordAt {
    pub line: u32,
    pub start: f32,
    pub length: f32,
    pub card: usize,
}

/// Samples along a line's track: enough that a word on a rounded section
/// keeps to it.
const TRACK_SAMPLES: usize = 48;

/// The drift of a layout's lines (#576), on bones from `first_bone` with room
/// for `room` of them: a word a bone, or neighbours on a line together when
/// there are more words than that. Each line's pace is `row_pace`, spread by
/// `row_pace_spread` either way from a draw of the lineage's.
#[allow(clippy::too_many_arguments)]
pub(super) fn row_drift(
    p: &FormParams,
    lin: &Lineage,
    spine: &Spine,
    layout: &RowLayout,
    segments: usize,
    first_bone: u16,
    room: usize,
    rng: &mut SeededRng,
) -> Option<RowDrift> {
    if p.row_pace == 0.0 || layout.words.is_empty() || room == 0 {
        return None;
    }
    // Words to a bone: the fewest that fit, neighbours on one line.
    let per_line: Vec<usize> = (0..layout.lines.len())
        .map(|l| layout.words.iter().filter(|w| w.line as usize == l).count())
        .collect();
    let together = (1..)
        .find(|g| per_line.iter().map(|n| n.div_ceil(*g)).sum::<usize>() <= room)
        .expect("one bone a line fits once every line is one group");

    let mut lines = Vec::with_capacity(layout.lines.len());
    for at in &layout.lines {
        let segment = ((at.u * segments as f32) as usize).min(segments - 1);
        let mut points = Vec::with_capacity(TRACK_SAMPLES);
        let mut normals = Vec::with_capacity(TRACK_SAMPLES);
        let r = radius_along(p, lin, at.u, at.side * FRAC_PI_2).max(1e-6);
        for i in 0..TRACK_SAMPLES {
            let x = -at.half + 2.0 * at.half * i as f32 / (TRACK_SAMPLES - 1) as f32;
            let a = at.side * (x * at.reading / r).clamp(-1.0, 1.0).acos();
            let (pos, n) = surface(p, lin, spine, at.u, a);
            points.push(pos + n * p.card_lift);
            normals.push(n);
        }
        let alongs = (0..TRACK_SAMPLES)
            .map(|i| {
                let (a, b) = (i.saturating_sub(1), (i + 1).min(TRACK_SAMPLES - 1));
                (points[b] - points[a]).normalized()
            })
            .collect();
        let pace = p.row_pace * (1.0 + p.row_pace_spread * rng.next_range(-1.0, 1.0));
        lines.push(RowLine {
            bone: segment as u16 + 1,
            pace,
            half: at.half,
            period: 0.0,
            points,
            alongs,
            normals,
        });
    }

    let mut words: Vec<RowWord> = Vec::new();
    let mut card_words = vec![0u32; layout.words.len()];
    let mut i = 0;
    while i < layout.words.len() {
        let first = layout.words[i];
        let mut j = i + 1;
        while j < layout.words.len() && j - i < together && layout.words[j].line == first.line {
            j += 1;
        }
        let last = layout.words[j - 1];
        let w = words.len();
        let hash = crate::living::mix64(w as u64 ^ 0x5157_6f72_6473);
        let share = 0.5 + 0.5 * ((hash >> 8) & 0xff) as f32 / 255.0;
        words.push(RowWord {
            line: first.line,
            start: first.start,
            length: last.start + last.length - first.start,
            channel: (hash % JITTER_CHANNELS as u64) as u16,
            share: if hash & 0x10000 == 0 { share } else { -share },
        });
        for word in &layout.words[i..j] {
            card_words[word.card] = w as u32;
        }
        i = j;
    }
    // A word has wholly gone at one end before it starts in at the other.
    for (l, line) in lines.iter_mut().enumerate() {
        let longest = words
            .iter()
            .filter(|w| w.line as usize == l)
            .map(|w| w.length)
            .fold(0.0, f32::max);
        line.period = 2.0 * line.half + longest + p.word_gap;
    }
    Some(RowDrift {
        lines,
        words,
        card_words,
        card_vertices: CARD_VERTICES,
        first_bone,
    })
}

/// One card: a quad of half-axes `hx` and `hy` about `centre`, facing `normal`,
/// and its back. `hx x hy` must point away from `normal`, since only front
/// faces draw.
fn card(centre: Vec3, normal: Vec3, hx: Vec3, hy: Vec3) -> [Vertex; CARD_VERTICES] {
    let corners = [
        centre - hx - hy,
        centre + hx - hy,
        centre + hx + hy,
        centre - hx + hy,
    ];
    let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let back = normal * -BACK_OFFSET;
    let ax = hx.normalized();
    std::array::from_fn(|c| {
        let (corner, facing, offset) = if c < 4 {
            (c, normal, Vec3::ZERO)
        } else {
            (c - 4, normal * -1.0, back)
        };
        Vertex {
            position: (corners[corner] + offset).to_array(),
            normal: facing.to_array(),
            tangent: [ax.x, ax.y, ax.z, 1.0],
            uv: uvs[corner],
        }
    })
}

/// Cards scattered over the whole surface, by area and `card_rise`.
fn scatter(
    p: &FormParams,
    lin: &Lineage,
    spine: &Spine,
    rng: &mut SeededRng,
    per_shade: &mut [Vec<[Vertex; CARD_VERTICES]>],
) {
    let shades = per_shade.len();
    let widest = widest(p, lin);
    let (from, to) = (p.card_from, p.card_to);
    for k in 0..p.cards {
        // Area-weighted, and crowded toward the tip by `card_rise`: more cards
        // where the form is wide, and where it's reaching.
        let (u, a) = loop {
            let u = rng.next_range(from, to);
            let a = rng.next_range(0.0, TAU);
            let wide = radius_along(p, lin, u, a) / widest;
            let rise = (1.0 + p.card_rise * u) / (1.0 + p.card_rise.max(0.0));
            if rng.next_f32() < wide * rise {
                break (u, a);
            }
        };
        let (pos, n) = surface(p, lin, spine, u, a);
        let up = up_the_surface(p, lin, spine, u, a);
        let up = (up - n * n.dot(&up)).normalized();
        let side = n.cross(&up).normalized();
        let turn = rng.next_range(-1.0, 1.0) * p.card_turn_deg.to_radians();
        let (ax, ay) = (
            side * turn.cos() + up * turn.sin(),
            up * turn.cos() - side * turn.sin(),
        );
        // Stand the card up about its width, its tip leaving the surface.
        let tilt = p.card_tilt * std::f32::consts::FRAC_PI_2 * rng.next_range(0.7, 1.0);
        let (ay, normal) = (
            ay * tilt.cos() + n * tilt.sin(),
            n * tilt.cos() - ay * tilt.sin(),
        );
        let size = p.card_size * rng.next_range(0.7, 1.3);
        let (hx, hy) = (ax * (size * 0.5), ay * (size * 0.5 * p.card_aspect));
        // A flat card is centred on its point; a standing one grows from it.
        let centre = pos + n * p.card_lift + hy * p.card_tilt;
        per_shade[k as usize % shades].push(card(centre, normal, hx, hy));
    }
}

/// Cards in rows across the broad faces, a card a word, as a page is written:
/// lines `row_spacing` apart from `card_to` down to `card_from`, each starting at the
/// left margin as its face is seen from outside and ending where the next word
/// won't fit, so the right edge is ragged. A paragraph's last line stops short,
/// and the next begins indented; the last line of all is a paragraph's last,
/// so the foot is never a flat, full line. The cards lie flat, so nothing bristles at the
/// outline, and a line is only as wide as the section where it runs: the
/// writing's outline is the form's.
fn rows(
    p: &FormParams,
    lin: &Lineage,
    spine: &Spine,
    rng: &mut SeededRng,
    per_shade: &mut [Vec<[Vertex; CARD_VERTICES]>],
) -> RowLayout {
    let mut layout = RowLayout {
        lines: Vec::new(),
        words: Vec::new(),
    };
    let shades = per_shade.len();
    let front = front_side(p, lin, spine);
    let faces = match p.row_faces {
        RowFaces::Front => vec![front],
        RowFaces::Back => vec![-front],
        RowFaces::Both => vec![front, -front],
    };
    // Lines in the paragraph so far, and in the whole of it, per face.
    let mut paragraph = vec![(0u32, paragraph_length(p, rng)); faces.len()];
    let word = p.card_size * p.card_aspect;
    let gap = p.word_gap;
    let step = p.row_spacing / p.height;
    let mut k = 0usize;
    // Written from the tip down, as a page is read, so a paragraph's first
    // line is its top one.
    let mut u = p.card_to - 0.5 * step;
    'lines: while u >= p.card_from {
        // The writing ends where a paragraph does: its last line is short, so
        // the foot of the form is never a flat, full line.
        let final_line = u - step < p.card_from;
        for (f, &side) in faces.iter().enumerate() {
            let half = radius_along(p, lin, u, side * FRAC_PI_2) * (1.0 - p.row_margin);
            // Too narrow for a few words: the rounded ends stay clear.
            if 2.0 * half < 3.0 * word {
                continue;
            }
            let (line, length) = paragraph[f];
            let last = length > 0 && (final_line || line + 1 >= length);
            let indent = if line == 0 && length > 0 {
                1.5 * word
            } else {
                0.0
            };
            let end = if last {
                -half + 2.0 * half * rng.next_range(0.25, 0.7)
            } else {
                half
            };
            // Which way along the section is left to right, seen from outside.
            let reading = reading_sign(p, lin, spine, u, side);
            // This line's index in the layout, once a word is on it (the
            // paragraph's own count is `line`).
            let written = layout.lines.len() as u32;
            let mut x = -half + indent;
            loop {
                let length = word * (0.3 + 1.4 * rng.next_f32().powf(1.5));
                if x + length > end {
                    break;
                }
                if k >= p.cards as usize {
                    break 'lines;
                }
                if layout.lines.len() == written as usize {
                    layout.lines.push(LineAt {
                        u,
                        side,
                        half,
                        reading,
                    });
                }
                layout.words.push(WordAt {
                    line: written,
                    start: x,
                    length,
                    card: k,
                });
                let middle = (x + 0.5 * length) * reading;
                let r = radius_along(p, lin, u, side * FRAC_PI_2).max(1e-6);
                let a = side * (middle / r).clamp(-1.0, 1.0).acos();
                let (pos, n) = surface(p, lin, spine, u, a);
                let up = up_the_surface(p, lin, spine, u, a);
                let up = (up - n * n.dot(&up)).normalized();
                let left = n.cross(&up).normalized();
                let turn = rng.next_range(-1.0, 1.0) * p.card_turn_deg.to_radians();
                let (ax, ay) = (
                    left * turn.cos() + up * turn.sin(),
                    up * turn.cos() - left * turn.sin(),
                );
                let thickness = p.card_size * rng.next_range(0.85, 1.1);
                let centre = pos + n * p.card_lift;
                per_shade[k % shades].push(card(
                    centre,
                    n,
                    ax * (0.5 * length),
                    ay * (0.5 * thickness),
                ));
                k += 1;
                x += length + gap * rng.next_range(0.7, 1.3);
            }
            paragraph[f] = if last {
                (0, paragraph_length(p, rng))
            } else {
                (line + 1, length)
            };
        }
        u -= step;
    }
    layout
}

/// How many lines the next paragraph runs to, 0 for one unbroken block.
fn paragraph_length(p: &FormParams, rng: &mut SeededRng) -> u32 {
    if p.paragraph_lines <= 0.0 {
        return 0;
    }
    (p.paragraph_lines * rng.next_range(0.5, 1.5))
        .round()
        .max(2.0) as u32
}

/// The side of the section (`+1`: angles in 0..π, `-1`: π..2π) whose broad
/// face is turned away from `facing_deg`: the front.
fn front_side(p: &FormParams, lin: &Lineage, spine: &Spine) -> f32 {
    let facing = p.facing_deg.to_radians() + lin.facing_turn;
    let away = Vec3::new(facing.cos(), 0.0, facing.sin());
    let (_, n) = surface(p, lin, spine, 0.5, FRAC_PI_2);
    if n.dot(&away) > 0.0 {
        -1.0
    } else {
        1.0
    }
}

/// `+1` if a growing broad-axis offset runs left to right on this face at `u`,
/// seen from outside (with the surface's up as up), else `-1`.
fn reading_sign(p: &FormParams, lin: &Lineage, spine: &Spine, u: f32, side: f32) -> f32 {
    let at = |x: f32| side * x.clamp(-1.0, 1.0).acos();
    let (here, n) = surface(p, lin, spine, u, at(0.0));
    let (there, _) = surface(p, lin, spine, u, at(0.1));
    let up = up_the_surface(p, lin, spine, u, at(0.0));
    let right = up.cross(&n);
    if (there - here).dot(&right) >= 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// The cards, a shade at a time, after the body's material.
fn assemble(p: &FormParams, per_shade: &[Vec<[Vertex; CARD_VERTICES]>]) -> MeshData {
    let shades = per_shade.len();
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut submeshes = Vec::new();
    for cards in per_shade {
        let start = indices.len() as u32;
        for card in cards {
            let b = vertices.len() as u32;
            vertices.extend_from_slice(card);
            // Counter-clockwise seen from the side each face's normal points
            // to (`ax x ay` is the card's back), since only front faces draw.
            indices.extend_from_slice(&[b, b + 2, b + 1, b, b + 3, b + 2]);
            indices.extend_from_slice(&[b + 4, b + 5, b + 6, b + 4, b + 6, b + 7]);
        }
        submeshes.push(SubMesh {
            index_start: start,
            index_count: indices.len() as u32 - start,
            material_index: submeshes.len() + 1,
        });
    }
    let mut materials = vec![body_material(p)];
    for s in 0..shades {
        let t = if shades > 1 {
            s as f32 / (shades - 1) as f32 - 0.5
        } else {
            0.0
        };
        let c = p.card_color;
        materials.push(MaterialData {
            name: format!("form_card_{s}"),
            base_color: [
                c[0] * (1.0 + 0.16 * t),
                c[1] * (1.0 + 0.12 * t),
                c[2] * (1.0 + 0.04 * t),
                1.0,
            ],
            metallic: 0.0,
            roughness: p.card_roughness,
            normal_map: None,
        });
    }
    let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
    MeshData {
        bounding_box: BoundingBox::from_positions(&positions),
        vertices,
        indices,
        materials,
        submeshes,
    }
}

/// The body's material: every card material comes after it.
fn body_material(p: &FormParams) -> MaterialData {
    MaterialData {
        name: "form_body".into(),
        // Below 1 the renderer draws it translucent, after the cards, and an
        // echo's opacity multiplies this one.
        base_color: [
            p.body_color[0],
            p.body_color[1],
            p.body_color[2],
            p.body_color[3] * p.body_opacity,
        ],
        metallic: 0.0,
        roughness: p.body_roughness,
        normal_map: None,
    }
}

/// Each card bound to the spine segment nearest its centre.
pub(super) fn bind_cards(cards: &MeshData, segments: &[BranchSegment]) -> Vec<LeafBinding> {
    cards
        .vertices
        .chunks(CARD_VERTICES)
        .map(|card| {
            let centre = card[..4]
                .iter()
                .fold(Vec3::ZERO, |s, v| s + Vec3::from_array(v.position))
                * 0.25;
            let (segment, along, at) = segments
                .iter()
                .map(|s| {
                    let ab = s.end - s.start;
                    let t = ((centre - s.start).dot(&ab) / ab.dot(&ab).max(1e-9)).clamp(0.0, 1.0);
                    (s.id, t, s.start + ab * t)
                })
                .min_by(|a, b| (centre - a.2).length().total_cmp(&(centre - b.2).length()))
                .expect("a spine has segments");
            LeafBinding {
                segment,
                along,
                offset: centre - at,
            }
        })
        .collect()
}
