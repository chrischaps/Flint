//! What the frame debugger shows, as data: the capture's passes as rows of
//! a tree, every draw and cull in one list, the images, and what is
//! selected. No egui here, so the stepping, filtering and linking are
//! tested without a window.

use super::super::{name_matches, passes};
use flint_render::frame_capture::{DrawDescription, FrameDescription, ImageDescription};
use std::collections::BTreeMap;

/// One pass, as a row of the tree.
#[derive(Debug, Clone)]
pub struct PassRow {
    /// Its place in `all_passes` (the `NN` of its image files).
    pub index: usize,
    pub name: String,
    /// `Bloom`, or `Bloom (2)` for the second of that name.
    pub key: String,
    /// How deep it sits: a shadow layer under `Shadows` is 1.
    pub depth: usize,
    /// The comic panel it belongs to; `None` for the frame's (or the
    /// page's) own passes.
    pub panel: Option<String>,
    pub ran: bool,
    pub skipped: Option<String>,
    pub timed: bool,
    pub gpu_ms: Option<f32>,
    /// This pass's draws and culls, as ranges of [`Model::draws`].
    pub draws: std::ops::Range<usize>,
    pub culled: std::ops::Range<usize>,
    /// Its triangles drawn (its own draws, not its sub-passes').
    pub triangles: u64,
    /// The draw-list layers its draws fall in, in first-drawn order, with
    /// how many draws each.
    pub layers: Vec<(String, usize)>,
    /// Its images, as indices into [`Model::images`].
    pub images: Vec<usize>,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub render_passes: Vec<String>,
    pub also: Vec<String>,
}

impl PassRow {
    /// How many draws it drew (not counting culls).
    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }
}

/// One draw, or one draw a cull set aside.
#[derive(Debug, Clone)]
pub struct DrawRow {
    /// The row of [`Model::passes`] it belongs to.
    pub pass: usize,
    /// Its place among its pass's draws (or culls), from 0.
    pub order: usize,
    pub draw: DrawDescription,
}

impl DrawRow {
    pub fn culled(&self) -> bool {
        self.draw.culled.is_some()
    }

    /// The entity's name, or the draw's kind in brackets for a draw of no
    /// entity (grass, a sprite batch).
    pub fn entity_label(&self) -> String {
        match &self.draw.entity {
            Some(name) => name.clone(),
            None => format!("({})", self.draw.kind),
        }
    }
}

/// One image of the capture.
#[derive(Debug, Clone)]
pub struct ImageRow {
    /// The row of [`Model::passes`] that wrote it.
    pub pass: usize,
    pub image: ImageDescription,
}

impl ImageRow {
    /// How the image is told apart from its pass's others: its target,
    /// and its step or layer when it has one.
    pub fn label(&self) -> String {
        let mut s = self.image.target.clone();
        if let Some(step) = &self.image.step {
            s.push_str(&format!(" · {step}"));
        }
        if let Some(layer) = self.image.layer {
            s.push_str(&format!(" · layer {layer}"));
        }
        s
    }
}

/// The capture, laid out for the window.
pub struct Model {
    pub passes: Vec<PassRow>,
    pub draws: Vec<DrawRow>,
    pub images: Vec<ImageRow>,
    /// The slowest pass's GPU time, for scaling the tree's bars.
    pub max_gpu_ms: f32,
}

impl Model {
    pub fn new(d: &FrameDescription) -> Self {
        let mut panel_of: Vec<Option<String>> = Vec::new();
        for panel in &d.panels {
            fn count(passes: &[flint_render::frame_capture::PassDescription]) -> usize {
                passes.iter().map(|p| 1 + count(&p.passes)).sum()
            }
            for _ in 0..count(&panel.passes) {
                panel_of.push(Some(panel.name.clone()));
            }
        }
        let mut rows = Vec::new();
        let mut draws = Vec::new();
        let mut images = Vec::new();
        for at in passes(d) {
            let p = at.pass;
            let row = rows.len();
            let start = draws.len();
            let mut layers: Vec<(String, usize)> = Vec::new();
            for (order, draw) in p.draws.iter().enumerate() {
                match layers.iter_mut().find(|(l, _)| *l == draw.layer) {
                    Some((_, n)) => *n += 1,
                    None => layers.push((draw.layer.clone(), 1)),
                }
                draws.push(DrawRow {
                    pass: row,
                    order,
                    draw: draw.clone(),
                });
            }
            let drawn = start..draws.len();
            for (order, draw) in p.culled.iter().enumerate() {
                let mut draw = draw.clone();
                draw.culled.get_or_insert_with(|| "culled".into());
                draws.push(DrawRow {
                    pass: row,
                    order,
                    draw,
                });
            }
            let culled = drawn.end..draws.len();
            let first_image = images.len();
            for image in &p.images {
                images.push(ImageRow {
                    pass: row,
                    image: image.clone(),
                });
            }
            rows.push(PassRow {
                index: at.index,
                name: p.name.clone(),
                key: at.key(),
                depth: at.depth,
                panel: panel_of.get(at.index).cloned().flatten(),
                ran: p.ran,
                skipped: p.skipped.clone(),
                timed: p.timed,
                gpu_ms: p.gpu_ms,
                triangles: p.draws.iter().map(|d| d.triangles as u64).sum(),
                draws: drawn,
                culled,
                layers,
                images: (first_image..images.len()).collect(),
                reads: p.reads.clone(),
                writes: p.writes.clone(),
                render_passes: p.render_passes.clone(),
                also: p.also.clone(),
            });
        }
        let max_gpu_ms = rows.iter().filter_map(|r| r.gpu_ms).fold(0.0f32, f32::max);
        Self {
            passes: rows,
            draws,
            images,
            max_gpu_ms,
        }
    }

    /// The pass to open on: the last that wrote an image (the finished
    /// frame), else the last that ran, else the first.
    pub fn default_pass(&self) -> usize {
        self.images
            .last()
            .map(|i| i.pass)
            .or_else(|| self.passes.iter().rposition(|p| p.ran))
            .unwrap_or(0)
    }

    /// The row whose name or key is `name` (`Bloom`, `Bloom (2)`), any
    /// case.
    pub fn find_pass(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        self.passes
            .iter()
            .position(|p| p.key == name || p.name == name)
            .or_else(|| {
                self.passes
                    .iter()
                    .position(|p| p.key.to_lowercase() == lower || p.name.to_lowercase() == lower)
            })
    }

    /// The image `pass` shows for the viewer: `choice` of its own (an
    /// index into its images, kept in range), else the main image of the
    /// nearest earlier pass that wrote one, with `false` to say it is not
    /// this pass's own.
    pub fn shown_image(&self, pass: usize, choice: usize) -> Option<(usize, bool)> {
        let row = self.passes.get(pass)?;
        if !row.images.is_empty() {
            let i = choice.min(row.images.len() - 1);
            return Some((row.images[i], true));
        }
        self.earlier(pass).map(|i| (i, false))
    }

    /// The first image of the nearest pass before `pass` that wrote any:
    /// its main target (the HDR colour rather than the depth beside it).
    fn earlier(&self, pass: usize) -> Option<usize> {
        let last = self.images.iter().rposition(|im| im.pass < pass)?;
        let of = self.images[last].pass;
        self.passes[of].images.first().copied()
    }

    /// The image before `image` that an A/B wipe compares it with: the
    /// main image of the nearest earlier pass that wrote one.
    pub fn previous_image(&self, image: usize) -> Option<usize> {
        let pass = self.images.get(image)?.pass;
        self.earlier(pass)
    }

    /// The finished frame: the last image of the capture.
    pub fn final_image(&self) -> Option<usize> {
        self.images.len().checked_sub(1)
    }

    /// Every row from `from`, `step` rows on (negative: back), clamped.
    pub fn step(&self, from: usize, step: isize) -> usize {
        let last = self.passes.len().saturating_sub(1) as isize;
        (from as isize + step).clamp(0, last) as usize
    }

    /// The next row from `from` that wrote an image, `forward` or back;
    /// `from` when there is none.
    pub fn step_to_image(&self, from: usize, forward: bool) -> usize {
        let has = |i: &usize| !self.passes[*i].images.is_empty();
        if forward {
            (from + 1..self.passes.len()).find(has).unwrap_or(from)
        } else {
            (0..from).rev().find(has).unwrap_or(from)
        }
    }

    /// Each row with how many draws (and culls) it has of `entity`.
    pub fn entity_passes(&self, entity: &str) -> Vec<(usize, usize, usize)> {
        let mut out: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        for d in &self.draws {
            if d.draw.entity.as_deref() == Some(entity) {
                let e = out.entry(d.pass).or_default();
                if d.culled() {
                    e.1 += 1;
                } else {
                    e.0 += 1;
                }
            }
        }
        out.into_iter().map(|(p, (n, c))| (p, n, c)).collect()
    }

    /// The draws the table lists, in its order.
    pub fn table(&self, q: &TableQuery) -> Vec<usize> {
        let filter = q.filter.trim().to_lowercase();
        let matches = |d: &DrawRow| {
            if filter.is_empty() {
                return true;
            }
            let name = d.entity_label().to_lowercase();
            if filter.contains('*') {
                name_matches(&filter, &name)
            } else {
                name.contains(&filter)
                    || d.draw.kind.to_lowercase().contains(&filter)
                    || d.draw.pipeline.to_lowercase().contains(&filter)
                    || d.draw
                        .mesh
                        .as_deref()
                        .is_some_and(|m| m.to_lowercase().contains(&filter))
            }
        };
        let mut rows: Vec<usize> = (0..self.draws.len())
            .filter(|&i| {
                let d = &self.draws[i];
                (q.all_passes || Some(d.pass) == q.pass)
                    && (q.show_culled || !d.culled())
                    && q.layer.as_ref().is_none_or(|l| *l == d.draw.layer)
                    && matches(d)
            })
            .collect();
        let key = |i: &usize| &self.draws[*i];
        match q.sort {
            Sort::Order => rows.sort_by_key(|i| (key(i).pass, key(i).culled(), key(i).order)),
            Sort::Entity => rows.sort_by_key(|i| key(i).entity_label().to_lowercase()),
            Sort::Kind => rows.sort_by(|a, b| key(a).draw.kind.cmp(&key(b).draw.kind)),
            Sort::Pipeline => rows.sort_by(|a, b| key(a).draw.pipeline.cmp(&key(b).draw.pipeline)),
            Sort::Triangles => rows.sort_by_key(|i| {
                std::cmp::Reverse(
                    key(i).draw.triangles as u64 * key(i).draw.instances.max(1) as u64,
                )
            }),
            Sort::Blend => rows.sort_by(|a, b| key(a).draw.blend.cmp(&key(b).draw.blend)),
            Sort::Culled => rows.sort_by(|a, b| key(b).draw.culled.cmp(&key(a).draw.culled)),
            // Costliest first, untimed last (#1203)
            Sort::Gpu => rows.sort_by(|a, b| {
                let ms = |i: &usize| key(i).draw.gpu_ms.unwrap_or(-1.0);
                ms(b).total_cmp(&ms(a))
            }),
        }
        if q.descending {
            rows.reverse();
        }
        rows
    }
}

/// What the draw table lists, and how.
#[derive(Debug, Clone, Default)]
pub struct TableQuery {
    /// Only this row's draws, unless `all_passes`.
    pub pass: Option<usize>,
    pub all_passes: bool,
    /// The draws a cull set aside too.
    pub show_culled: bool,
    /// Only this draw-list layer.
    pub layer: Option<String>,
    /// An entity's name (any part of it, or `*` patterns), a kind, a
    /// pipeline or a mesh.
    pub filter: String,
    pub sort: Sort,
    pub descending: bool,
}

/// A column the draw table sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// As the frame drew them.
    #[default]
    Order,
    Entity,
    Kind,
    Pipeline,
    /// Most triangles (times instances) first.
    Triangles,
    Blend,
    Culled,
    /// Most GPU time first: a replay's `--draw-times` (#1203).
    Gpu,
}

impl Model {
    /// Whether any draw carries a GPU time (a replay's `--draw-times`).
    pub fn draws_timed(&self) -> bool {
        self.draws.iter().any(|d| d.draw.gpu_ms.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_render::frame_capture::PassDescription;

    fn draw(entity: &str, layer: &str, tris: u32) -> DrawDescription {
        DrawDescription {
            entity: Some(entity.into()),
            kind: "Static".into(),
            layer: layer.into(),
            pipeline: "pbr".into(),
            triangles: tris,
            instances: 1,
            ..Default::default()
        }
    }

    fn image(target: &str) -> ImageDescription {
        let mut i = ImageDescription::default();
        i.target = target.into();
        i.file = format!("targets/{target}.png");
        i
    }

    /// Shadows with two layers, a skipped pass, the scene, bloom twice, and
    /// a composite.
    fn frame() -> FrameDescription {
        let pass = |name: &str| PassDescription {
            name: name.into(),
            ran: true,
            ..Default::default()
        };
        let mut shadows = pass("Shadows");
        let mut c0 = pass("Shadow cascade 0");
        c0.draws = vec![
            draw("crate", "Opaque static", 12),
            draw("barrel", "Opaque static", 40),
        ];
        c0.images = vec![image("shadow_map")];
        let mut c1 = pass("Shadow cascade 1");
        c1.draws = vec![draw("crate", "Opaque static", 12)];
        shadows.passes = vec![c0, c1];
        let fog = PassDescription {
            name: "Fog veil".into(),
            skipped: Some("no fog".into()),
            ..Default::default()
        };
        let mut scene = pass("Scene");
        scene.gpu_ms = Some(1.5);
        scene.draws = vec![
            draw("crate", "Opaque static", 12),
            draw("barrel", "Opaque static", 40),
            draw("glass", "Blended static", 2),
        ];
        let mut far = draw("tree_far", "Opaque static", 900);
        far.culled = Some("draw distance".into());
        scene.culled = vec![far];
        scene.images = vec![image("hdr_scene"), image("depth")];
        let mut bloom = pass("Bloom");
        bloom.gpu_ms = Some(0.3);
        let mut composite = pass("Composite");
        composite.images = vec![image("surface")];
        let mut d = FrameDescription::default();
        d.passes = vec![shadows, fog, scene, bloom.clone(), bloom, composite];
        d
    }

    #[test]
    fn the_tree_keeps_the_frames_order_and_nesting() {
        let m = Model::new(&frame());
        let rows: Vec<(&str, usize)> = m.passes.iter().map(|p| (p.key.as_str(), p.depth)).collect();
        assert_eq!(
            rows,
            [
                ("Shadows", 0),
                ("Shadow cascade 0", 1),
                ("Shadow cascade 1", 1),
                ("Fog veil", 0),
                ("Scene", 0),
                ("Bloom", 0),
                ("Bloom (2)", 0),
                ("Composite", 0),
            ]
        );
        let scene = &m.passes[4];
        assert_eq!(scene.draw_count(), 3);
        assert_eq!(scene.culled.len(), 1);
        assert_eq!(scene.triangles, 54);
        assert_eq!(
            scene.layers,
            [
                ("Opaque static".to_string(), 2),
                ("Blended static".to_string(), 1)
            ]
        );
        assert_eq!(m.max_gpu_ms, 1.5);
        assert_eq!(m.default_pass(), 7, "opens on the finished frame");
    }

    #[test]
    fn stepping_walks_every_pass_and_shows_the_last_image_so_far() {
        let m = Model::new(&frame());
        // From the shadows to the composite, one key at a time
        let mut at = 0;
        let mut seen = Vec::new();
        loop {
            let shown = m
                .shown_image(at, 0)
                .map(|(i, own)| (m.images[i].image.target.clone(), own));
            seen.push(shown);
            let next = m.step(at, 1);
            if next == at {
                break;
            }
            at = next;
        }
        let names: Vec<Option<(&str, bool)>> = seen
            .iter()
            .map(|s| s.as_ref().map(|(t, o)| (t.as_str(), *o)))
            .collect();
        assert_eq!(
            names,
            [
                None,
                Some(("shadow_map", true)),
                Some(("shadow_map", false)),
                Some(("shadow_map", false)),
                Some(("hdr_scene", true)),
                Some(("hdr_scene", false)),
                Some(("hdr_scene", false)),
                Some(("surface", true)),
            ]
        );
        assert_eq!(m.step(0, -1), 0);
        assert_eq!(m.step(7, 5), 7);
        assert_eq!(m.step_to_image(1, true), 4);
        assert_eq!(m.step_to_image(4, false), 1);
        assert_eq!(m.step_to_image(7, true), 7);
        // A pass's second image, and a choice past its last
        assert_eq!(m.shown_image(4, 1), Some((2, true)));
        assert_eq!(m.shown_image(4, 9), Some((2, true)));
        assert_eq!(
            m.previous_image(3),
            Some(1),
            "the composite against the HDR scene"
        );
        assert_eq!(m.final_image(), Some(3));
        assert_eq!(m.find_pass("bloom (2)"), Some(6));
        assert_eq!(m.find_pass("Scene"), Some(4));
        assert_eq!(m.find_pass("nothing"), None);
    }

    #[test]
    fn an_entity_is_found_by_name_in_every_pass() {
        let m = Model::new(&frame());
        assert_eq!(m.entity_passes("crate"), [(1, 1, 0), (2, 1, 0), (4, 1, 0)]);
        assert_eq!(m.entity_passes("tree_far"), [(4, 0, 1)]);
        let q = TableQuery {
            all_passes: true,
            filter: "CRA".into(),
            ..Default::default()
        };
        let found: Vec<usize> = m.table(&q).iter().map(|&i| m.draws[i].pass).collect();
        assert_eq!(found, [1, 2, 4]);
        let q = TableQuery {
            all_passes: true,
            filter: "*rel".into(),
            ..Default::default()
        };
        assert_eq!(m.table(&q).len(), 2, "a pattern matches the whole name");
    }

    #[test]
    fn the_table_filters_by_pass_layer_and_cull_and_sorts() {
        let m = Model::new(&frame());
        let mut q = TableQuery {
            pass: Some(4),
            ..Default::default()
        };
        assert_eq!(m.table(&q).len(), 3);
        q.show_culled = true;
        assert_eq!(m.table(&q).len(), 4);
        q.sort = Sort::Triangles;
        let first = &m.draws[m.table(&q)[0]];
        assert_eq!(first.draw.entity.as_deref(), Some("tree_far"));
        assert_eq!(first.draw.culled.as_deref(), Some("draw distance"));
        q.descending = true;
        assert_eq!(
            m.draws[m.table(&q)[0]].draw.entity.as_deref(),
            Some("glass")
        );
        q.layer = Some("Blended static".into());
        assert_eq!(m.table(&q).len(), 1);
    }

    #[test]
    fn the_table_sorts_by_gpu_time_costliest_first_untimed_last() {
        let mut d = frame();
        assert!(!Model::new(&d).draws_timed());
        let scene = d.passes.iter_mut().find(|p| p.name == "Scene").unwrap();
        scene.draws[0].gpu_ms = Some(0.01);
        scene.draws[1].gpu_ms = Some(0.2);
        let m = Model::new(&d);
        assert!(m.draws_timed());
        let at = m.passes.iter().position(|p| p.key == "Scene").unwrap();
        let q = TableQuery {
            pass: Some(at),
            sort: Sort::Gpu,
            ..Default::default()
        };
        let ms: Vec<Option<f32>> = m
            .table(&q)
            .iter()
            .map(|&i| m.draws[i].draw.gpu_ms)
            .collect();
        assert_eq!(&ms[..2], &[Some(0.2), Some(0.01)]);
        assert!(ms[2..].iter().all(Option::is_none), "{ms:?}");
    }

    #[test]
    fn a_description_with_nothing_in_it_makes_an_empty_model() {
        let m = Model::new(&FrameDescription::default());
        assert!(m.passes.is_empty());
        assert_eq!(m.default_pass(), 0);
        assert_eq!(m.shown_image(0, 0), None);
        assert_eq!(m.max_gpu_ms, 0.0);
        assert_eq!(m.step(0, 1), 0);
    }
}
