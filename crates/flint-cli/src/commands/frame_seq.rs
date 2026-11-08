//! Headless frame sequences for `flint render --frames N` (#345).
//!
//! One render is one frame, so a temporal style (ASCII's per-cell history,
//! #277) never sees a frame before it. A sequence draws N frames on one
//! renderer, a fixed `--frame-dt` apart, moving the camera between them
//! (`--orbit` deg/s about the target, or a `--camera-path` of keys), so the
//! styles see consecutive `StyleFrame::frame` numbers and their history
//! runs as it does in the player, without a window or a script.
//!
//! [`FlipCounter`] measures glyph shimmer from the ASCII style's own grid
//! (the #317 text capture, read from the cell state the glyph pass drew):
//! flips are cells whose glyph changed since the frame before, reversals
//! the ones that changed back to the glyph of two frames ago (A, B, A).
//! Motion rarely reverses a cell in two frames; shimmer does little else.

use anyhow::{bail, Context, Result};
use flint_core::Vec3;
use flint_render::stylize::GlyphText;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One key of a `--camera-path` file.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CameraKey {
    /// Seconds from the first frame.
    pub time: f32,
    pub position: [f32; 3],
    pub target: [f32; 3],
}

/// A `--camera-path` file:
///
/// ```toml
/// [[key]]
/// time = 0.0
/// position = [4.2, 1.5, 8.5]
/// target = [0.6, 0.6, 4.2]
///
/// [[key]]
/// time = 3.0
/// position = [5.4, 1.5, 6.9]
/// target = [0.6, 0.6, 4.2]
/// ```
///
/// Position and target are each interpolated linearly between the keys
/// around a frame's time, and held past the first and last.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CameraPath {
    pub key: Vec<CameraKey>,
}

impl CameraPath {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
        Self::parse(&text).with_context(|| format!("--camera-path {path}"))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut path: CameraPath = toml::from_str(text)?;
        if path.key.is_empty() {
            bail!("a camera path needs at least one [[key]]");
        }
        if path.key.iter().any(|k| !k.time.is_finite()) {
            bail!("a key's time must be a number of seconds");
        }
        path.key.sort_by(|a, b| a.time.total_cmp(&b.time));
        Ok(path)
    }

    /// The camera's position and target at `t` seconds.
    pub fn at(&self, t: f32) -> (Vec3, Vec3) {
        let v = |a: [f32; 3]| Vec3::new(a[0], a[1], a[2]);
        let keys = &self.key;
        let first = &keys[0];
        let last = &keys[keys.len() - 1];
        if t <= first.time {
            return (v(first.position), v(first.target));
        }
        if t >= last.time {
            return (v(last.position), v(last.target));
        }
        let i = keys.partition_point(|k| k.time <= t).max(1);
        let (a, b) = (&keys[i - 1], &keys[i]);
        let span = (b.time - a.time).max(1e-6);
        let s = (t - a.time) / span;
        let lerp = |p: [f32; 3], q: [f32; 3]| {
            Vec3::new(
                p[0] + (q[0] - p[0]) * s,
                p[1] + (q[1] - p[1]) * s,
                p[2] + (q[2] - p[2]) * s,
            )
        };
        (lerp(a.position, b.position), lerp(a.target, b.target))
    }
}

/// Frame `index`'s file beside `output`: `out/seq.png` gives
/// `out/seq_0000.png`, `out/seq_0001.png`... (`digits` wide, at least 4).
pub fn numbered(output: &str, index: u32, frames: u32) -> PathBuf {
    let path = Path::new(output);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "frame".into());
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "png".into());
    let digits = frames.saturating_sub(1).max(1).to_string().len().max(4);
    path.with_file_name(format!("{stem}_{index:0digits$}.{ext}"))
}

/// What a sequence's glyphs did, frame to frame.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct FlipStats {
    pub frames: u32,
    /// Cells in the grid (columns x rows).
    pub cells: u32,
    pub flips_per_frame: f64,
    pub flips_pct: f64,
    pub reversals_per_frame: f64,
    pub reversals_pct: f64,
    /// Per frame from the second: cells whose glyph changed.
    pub flips: Vec<u32>,
    /// Per frame from the third: cells changed back to two frames ago.
    pub reversals: Vec<u32>,
}

/// Counts glyph flips and reversals over a sequence of [`GlyphText`]s.
#[derive(Default)]
pub struct FlipCounter {
    prev: Option<Vec<Option<char>>>,
    prev2: Option<Vec<Option<char>>>,
    size: Option<(u32, u32)>,
    frames: u32,
    flips: Vec<u32>,
    reversals: Vec<u32>,
}

/// What [`FlipCounter::push`] saw in one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameFlips {
    pub flips: Option<u32>,
    pub reversals: Option<u32>,
}

impl FlipCounter {
    /// Count one frame against the two before it. A grid of another size
    /// (a resize mid-sequence) starts the count over.
    pub fn push(&mut self, text: &GlyphText) -> FrameFlips {
        let glyphs: Vec<Option<char>> = text.cells.iter().map(|c| c.glyph).collect();
        if self.size != Some((text.cols, text.rows)) {
            self.size = Some((text.cols, text.rows));
            self.prev = None;
            self.prev2 = None;
        }
        self.frames += 1;
        let mut out = FrameFlips {
            flips: None,
            reversals: None,
        };
        if let Some(prev) = &self.prev {
            let changed: Vec<bool> = glyphs.iter().zip(prev).map(|(a, b)| a != b).collect();
            let flips = changed.iter().filter(|&&c| c).count() as u32;
            self.flips.push(flips);
            out.flips = Some(flips);
            if let Some(prev2) = &self.prev2 {
                let back = glyphs
                    .iter()
                    .zip(prev2)
                    .zip(&changed)
                    .filter(|((a, b), &c)| c && a == b)
                    .count() as u32;
                self.reversals.push(back);
                out.reversals = Some(back);
            }
        }
        self.prev2 = self.prev.take();
        self.prev = Some(glyphs);
        out
    }

    pub fn stats(&self) -> FlipStats {
        let cells = self.size.map_or(0, |(c, r)| c * r);
        let mean = |v: &[u32]| {
            if v.is_empty() {
                0.0
            } else {
                v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64
            }
        };
        let pct = |m: f64| {
            if cells == 0 {
                0.0
            } else {
                100.0 * m / cells as f64
            }
        };
        let flips = mean(&self.flips);
        let reversals = mean(&self.reversals);
        FlipStats {
            frames: self.frames,
            cells,
            flips_per_frame: flips,
            flips_pct: pct(flips),
            reversals_per_frame: reversals,
            reversals_pct: pct(reversals),
            flips: self.flips.clone(),
            reversals: self.reversals.clone(),
        }
    }
}

impl FlipStats {
    /// The one-line summary `tools/ascii_flips.py` prints, so the two read
    /// alike.
    pub fn summary(&self) -> String {
        format!(
            "{} frames, {} cells: {:.1} flips/frame ({:.2}%), {:.1} reversals/frame ({:.2}%)",
            self.frames,
            self.cells,
            self.flips_per_frame,
            self.flips_pct,
            self.reversals_per_frame,
            self.reversals_pct
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_render::stylize::GlyphCell;

    fn grid(s: &str) -> GlyphText {
        GlyphText {
            cols: s.len() as u32,
            rows: 1,
            cells: s
                .chars()
                .map(|c| GlyphCell {
                    glyph: (c != ' ').then_some(c),
                    ink: [255; 3],
                    ground: [0; 3],
                })
                .collect(),
        }
    }

    #[test]
    fn flips_and_reversals() {
        let mut n = FlipCounter::default();
        assert_eq!(n.push(&grid("aaaa")).flips, None);
        // two cells change
        let f = n.push(&grid("abba"));
        assert_eq!((f.flips, f.reversals), (Some(2), None));
        // one goes back (a reversal), one moves on (a flip only)
        let f = n.push(&grid("aaca"));
        assert_eq!((f.flips, f.reversals), (Some(2), Some(1)));
        // nothing changes: no flips, so no reversals even though cell 0
        // matches two frames ago
        let f = n.push(&grid("aaca"));
        assert_eq!((f.flips, f.reversals), (Some(0), Some(0)));
        let s = n.stats();
        assert_eq!(s.frames, 4);
        assert_eq!(s.cells, 4);
        assert_eq!(s.flips, vec![2, 2, 0]);
        assert_eq!(s.reversals, vec![1, 0]);
        assert!((s.flips_per_frame - 4.0 / 3.0).abs() < 1e-9);
        assert!((s.reversals_pct - 12.5).abs() < 1e-9);
    }

    #[test]
    fn passed_cells_count_as_a_glyph_of_their_own() {
        let mut n = FlipCounter::default();
        n.push(&grid("a "));
        assert_eq!(n.push(&grid("aa")).flips, Some(1));
    }

    #[test]
    fn a_new_grid_size_starts_over() {
        let mut n = FlipCounter::default();
        n.push(&grid("aa"));
        assert_eq!(n.push(&grid("aaa")).flips, None);
        assert_eq!(n.push(&grid("aab")).flips, Some(1));
    }

    #[test]
    fn camera_path_interpolates_and_holds() {
        let path = CameraPath::parse(
            "[[key]]\ntime = 2.0\nposition = [2, 0, 0]\ntarget = [0, 0, 2]\n\
             [[key]]\ntime = 0.0\nposition = [0, 0, 0]\ntarget = [0, 0, 0]\n",
        )
        .unwrap();
        assert_eq!(path.key[0].time, 0.0, "keys sort by time");
        let (p, t) = path.at(0.5);
        assert!((p.x - 0.5).abs() < 1e-6 && (t.z - 0.5).abs() < 1e-6);
        assert_eq!(path.at(-1.0).0.x, 0.0);
        assert_eq!(path.at(9.0).0.x, 2.0);
        assert!(CameraPath::parse("key = []").is_err());
        assert!(CameraPath::parse("[[key]]\ntime = 0\nposition = [0,0,0]\n").is_err());
    }

    #[test]
    fn numbered_paths() {
        let p = |o, i, n| numbered(o, i, n).to_string_lossy().replace('\\', "/");
        assert_eq!(p("out/seq.png", 3, 181), "out/seq_0003.png");
        assert_eq!(p("render.png", 0, 1), "render_0000.png");
        assert_eq!(p("a/b", 12345, 20000), "a/b_12345.png");
    }
}
