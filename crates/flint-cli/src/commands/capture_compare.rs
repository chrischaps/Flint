//! Two captures of one take, frame by frame (`flint capture --verify`,
//! #722; was the showcase film's `compare.py`, #435, whose rig is
//! `flint-examples/tools/showcase/` since #984).
//!
//! Frames pair by name. For each pair: how many pixels differ, and the
//! largest difference in any colour channel in 8-bit levels. A take is
//! deterministic when every pair is the same; issue #341 found stepped
//! captures differing by one level in a handful of pixels, so the report
//! also counts frames by their worst difference, which tells a one-level
//! wobble from a real divergence (a wrong glyph is tens of levels over a
//! whole cell).

use super::capture::frame_files;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One frame that differs between the runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Differing {
    pub frame: String,
    /// Pixels with any channel different
    pub pixels: u64,
    /// The largest difference in a channel, 0-255 (255 for a size change)
    pub max_level: u8,
    pub fraction: f64,
    /// `(w, h)` of each run when they aren't the same size
    pub sizes: Option<((u32, u32), (u32, u32))>,
}

#[derive(Debug, Clone)]
pub struct Comparison {
    pub a: PathBuf,
    pub b: PathBuf,
    /// Frames both runs wrote
    pub frames: usize,
    pub differing: Vec<Differing>,
    /// Frames only one run wrote
    pub only_in_one: Vec<String>,
}

impl Comparison {
    /// Every frame byte for byte.
    pub fn identical(&self) -> bool {
        self.differing.is_empty() && self.only_in_one.is_empty()
    }

    pub fn max_level(&self) -> u8 {
        self.differing
            .iter()
            .map(|d| d.max_level)
            .max()
            .unwrap_or(0)
    }

    /// The runs match when every frame is in both and none differs by more
    /// than `tolerance` levels.
    pub fn same_within(&self, tolerance: u8) -> bool {
        self.only_in_one.is_empty() && self.max_level() <= tolerance
    }

    fn by_level(&self) -> BTreeMap<u8, usize> {
        let mut m = BTreeMap::new();
        for d in &self.differing {
            *m.entry(d.max_level).or_insert(0) += 1;
        }
        m
    }

    pub fn summary(&self) -> String {
        if self.identical() {
            return format!("identical: all {} frames byte for byte", self.frames);
        }
        let worst_pixels = self.differing.iter().map(|d| d.pixels).max().unwrap_or(0);
        let mut s = format!(
            "{}/{} frames identical; {} differ (worst {}, at most {} in a frame)",
            self.frames - self.differing.len(),
            self.frames,
            self.differing.len(),
            count(self.max_level() as u64, "level"),
            count(worst_pixels, "pixel")
        );
        if !self.only_in_one.is_empty() {
            s.push_str(&format!(
                "; {} in one run only",
                count(self.only_in_one.len() as u64, "frame")
            ));
        }
        s
    }

    /// The summary, then the frames that differ (the first ten), then the
    /// verdict against `tolerance`.
    pub fn text_lines(&self, tolerance: u8) -> Vec<String> {
        let mut lines = vec![self.summary()];
        const SHOWN: usize = 10;
        for d in self.differing.iter().take(SHOWN) {
            lines.push(match d.sizes {
                Some((a, b)) => format!("  {}: {}x{} against {}x{}", d.frame, a.0, a.1, b.0, b.1),
                None => format!(
                    "  {}: {} ({:.4}%), up to {}",
                    d.frame,
                    count(d.pixels, "pixel"),
                    d.fraction * 100.0,
                    count(d.max_level as u64, "level")
                ),
            });
        }
        if self.differing.len() > SHOWN {
            lines.push(format!(
                "  ... and {} more (verify.json lists them)",
                self.differing.len() - SHOWN
            ));
        }
        for name in self.only_in_one.iter().take(SHOWN) {
            lines.push(format!("  {name}: in one run only"));
        }
        if !self.identical() {
            let by: Vec<String> = self
                .by_level()
                .iter()
                .map(|(l, n)| format!("{n} at {l}"))
                .collect();
            lines.push(format!("  frames by worst level: {}", by.join(", ")));
            lines.push(if self.same_within(tolerance) {
                format!("  within --tolerance {tolerance}: the same")
            } else {
                format!(
                    "  DIFFERENT (over --tolerance {tolerance}); both runs kept: {} and {}",
                    self.a.display(),
                    self.b.display()
                )
            });
        }
        lines
    }

    pub fn json(&self, tolerance: u8) -> Value {
        json!({
            "a": self.a.display().to_string(),
            "b": self.b.display().to_string(),
            "frames": self.frames,
            "identical": self.frames - self.differing.len(),
            "same": self.same_within(tolerance),
            "tolerance": tolerance,
            "max_level": self.max_level(),
            "frames_by_max_level": self.by_level().iter()
                .map(|(l, n)| (l.to_string(), json!(n)))
                .collect::<serde_json::Map<_, _>>(),
            "differing": self.differing.iter().map(|d| {
                let mut v = json!({
                    "frame": d.frame,
                    "pixels": d.pixels,
                    "max_level": d.max_level,
                    "fraction": (d.fraction * 1e8).round() / 1e8,
                });
                if let Some((a, b)) = d.sizes {
                    v["sizes"] = json!([[a.0, a.1], [b.0, b.1]]);
                }
                v
            }).collect::<Vec<_>>(),
            "only_in_one": self.only_in_one,
            "summary": self.summary(),
        })
    }
}

/// `1 pixel`, `2 pixels`.
fn count(n: u64, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// Compares two decoded frames: `None` when they're the same.
pub fn compare_images(name: &str, x: &image::RgbImage, y: &image::RgbImage) -> Option<Differing> {
    if x.dimensions() != y.dimensions() {
        return Some(Differing {
            frame: name.to_string(),
            pixels: 0,
            max_level: 255,
            fraction: 1.0,
            sizes: Some((x.dimensions(), y.dimensions())),
        });
    }
    let mut pixels = 0u64;
    let mut worst = 0u8;
    for (p, q) in x
        .as_raw()
        .as_chunks::<3>()
        .0
        .iter()
        .zip(y.as_raw().as_chunks::<3>().0)
    {
        if p != q {
            pixels += 1;
            for c in 0..3 {
                worst = worst.max(p[c].abs_diff(q[c]));
            }
        }
    }
    (pixels > 0).then(|| Differing {
        frame: name.to_string(),
        pixels,
        max_level: worst,
        fraction: pixels as f64 / (x.width() as f64 * x.height() as f64),
        sizes: None,
    })
}

fn compare_pair(a: &Path, b: &Path, name: &str) -> Result<Option<Differing>> {
    let x = std::fs::read(a.join(name))
        .with_context(|| format!("reading {}", a.join(name).display()))?;
    let y = std::fs::read(b.join(name))
        .with_context(|| format!("reading {}", b.join(name).display()))?;
    // Byte-identical files are the common case and need no decode
    if x == y {
        return Ok(None);
    }
    let decode = |bytes: &[u8], dir: &Path| {
        image::load_from_memory(bytes)
            .map(|i| i.to_rgb8())
            .with_context(|| format!("decoding {}", dir.join(name).display()))
    };
    Ok(compare_images(name, &decode(&x, a)?, &decode(&y, b)?))
}

/// Compares every frame of two take folders, on every core.
pub fn compare_folders(a: &Path, b: &Path) -> Result<Comparison> {
    let names_a: BTreeSet<String> = frame_files(a)?.into_iter().collect();
    let names_b: BTreeSet<String> = frame_files(b)?.into_iter().collect();
    let common: Vec<&String> = names_a.intersection(&names_b).collect();
    let only_in_one: Vec<String> = names_a.symmetric_difference(&names_b).cloned().collect();

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(common.len().max(1));
    let chunk = common.len().div_ceil(threads).max(1);
    let mut differing: Vec<Differing> = std::thread::scope(|scope| {
        let handles: Vec<_> = common
            .chunks(chunk)
            .map(|names| {
                scope.spawn(move || -> Result<Vec<Differing>> {
                    let mut found = Vec::new();
                    for name in names {
                        if let Some(d) = compare_pair(a, b, name)? {
                            found.push(d);
                        }
                    }
                    Ok(found)
                })
            })
            .collect();
        let mut all = Vec::new();
        for h in handles {
            all.extend(h.join().expect("compare thread")?);
        }
        Ok::<_, anyhow::Error>(all)
    })?;
    differing.sort_by(|x, y| x.frame.cmp(&y.frame));
    Ok(Comparison {
        a: a.to_path_buf(),
        b: b.to_path_buf(),
        frames: common.len(),
        differing,
        only_in_one,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flint-capture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        dir
    }

    fn frame(v: u8) -> RgbImage {
        RgbImage::from_pixel(8, 4, Rgb([v, v, v]))
    }

    #[test]
    fn identical_runs_say_so() {
        let dir = scratch("same");
        for n in 1..=3 {
            for side in ["a", "b"] {
                frame(40)
                    .save(dir.join(side).join(format!("frame_{n:04}.png")))
                    .unwrap();
            }
        }
        let c = compare_folders(&dir.join("a"), &dir.join("b")).unwrap();
        assert!(c.identical());
        assert_eq!(c.frames, 3);
        assert_eq!(c.summary(), "identical: all 3 frames byte for byte");
        assert_eq!(c.json(0)["same"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_report_names_each_frame_that_differs() {
        let dir = scratch("differ");
        for n in 1..=4 {
            frame(40)
                .save(dir.join("a").join(format!("frame_{n:04}.png")))
                .unwrap();
        }
        frame(40).save(dir.join("b/frame_0001.png")).unwrap();
        // One level in two pixels
        let mut wobble = frame(40);
        wobble.put_pixel(0, 0, Rgb([41, 40, 40]));
        wobble.put_pixel(3, 2, Rgb([40, 39, 40]));
        wobble.save(dir.join("b/frame_0002.png")).unwrap();
        // A real divergence over half the frame
        let mut wrong = frame(40);
        for x in 0..4 {
            for y in 0..4 {
                wrong.put_pixel(x, y, Rgb([200, 40, 40]));
            }
        }
        wrong.save(dir.join("b/frame_0003.png")).unwrap();
        // frame 4 only in a
        let c = compare_folders(&dir.join("a"), &dir.join("b")).unwrap();
        assert_eq!(c.frames, 3);
        assert_eq!(c.only_in_one, ["frame_0004.png"]);
        let names: Vec<_> = c.differing.iter().map(|d| d.frame.as_str()).collect();
        assert_eq!(names, ["frame_0002.png", "frame_0003.png"]);
        assert_eq!((c.differing[0].pixels, c.differing[0].max_level), (2, 1));
        assert_eq!((c.differing[1].pixels, c.differing[1].max_level), (16, 160));
        assert_eq!(c.differing[1].fraction, 0.5);
        assert!(!c.same_within(255), "a frame in one run only never matches");
        let text = c.text_lines(0).join("\n");
        // What --verify prints for these runs (`-- --nocapture` shows it)
        println!("{text}");
        assert!(
            text.contains("1/3 frames identical; 2 differ (worst 160 levels"),
            "{text}"
        );
        assert!(text.contains("frame_0002.png: 2 pixels"), "{text}");
        assert!(text.contains("frame_0004.png: in one run only"), "{text}");
        let j = c.json(0);
        assert_eq!(j["frames_by_max_level"]["1"], 1);
        assert_eq!(j["differing"][1]["max_level"], 160);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tolerance_forgives_a_wobble_not_a_divergence() {
        let mut y = frame(40);
        y.put_pixel(1, 1, Rgb([41, 40, 40]));
        let d = compare_images("f", &frame(40), &y).unwrap();
        let c = Comparison {
            a: "a".into(),
            b: "b".into(),
            frames: 1,
            differing: vec![d],
            only_in_one: vec![],
        };
        assert!(!c.same_within(0));
        assert!(c.same_within(1));
        let sized = compare_images("f", &frame(1), &RgbImage::new(4, 4)).unwrap();
        assert_eq!(sized.max_level, 255);
        assert_eq!(sized.sizes, Some(((8, 4), (4, 4))));
    }
}
