//! Animated GIFs for judging motion (#216), which sets one rule: nothing may move
//! in the file that didn't move in the render.
//!
//! The usual way of writing a GIF breaks it. `image`'s encoder quantizes each frame
//! to a palette of its own, so a patch of sky comes out a slightly different colour
//! every frame, and shimmer is the very thing someone is looking at a living prop to
//! judge. So every frame here shares one palette, with no dithering, and a pixel the
//! render didn't change is written as transparent over the frame before it. A still
//! pixel can't flicker, and stillness compresses to almost nothing.

use anyhow::{anyhow, bail, Context, Result};
use color_quant::NeuQuant;
use image::RgbImage;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// The palette's colours, leaving the last index to mean "as the last frame".
const COLORS: usize = 255;
const UNCHANGED: u8 = 255;

/// About how many pixels the palette is learned from, spread across the frames, so
/// that a long GIF costs no more to quantize than a short one.
const PALETTE_SAMPLES: usize = 2_000_000;

/// NeuQuant's own trade of speed for quality: 1 looks at every pixel it's given and
/// 10 at one in ten, its default. Under this many pixels it looks at them all, since
/// at one in ten a rare colour, such as a sheet's lettering, may never get a place.
const PALETTE_THOROUGH_BELOW: usize = 400_000;
const PALETTE_EFFORT: i32 = 10;

/// How hard a spilled frame is deflated: 1 is the fastest, and the frame-to-frame
/// difference it packs is mostly zeros, which any level packs to almost nothing.
const SPILL_LEVEL: u8 = 1;

/// Write `frames` as a GIF that loops forever at `fps`.
pub fn write(path: &Path, frames: &[RgbImage], fps: f32) -> Result<()> {
    let Some(first) = frames.first() else {
        bail!("a GIF needs at least one frame");
    };
    let (width, height) = first.dimensions();
    let mut spool = Spool::new(path, width, height, frames.len())?;
    for frame in frames {
        spool.push(frame)?;
    }
    spool.finish(fps)
}

/// A GIF written as its frames arrive, in memory that stays flat however long it
/// runs (#588). The palette has to be known before the first frame is encoded and
/// is learned from all of them, so each frame waits on disk, beside the GIF, as
/// its difference from the frame before, deflated: what stood still costs almost
/// nothing. The palette's sample is taken on the way in, the pixels it would have
/// taken from the frames all held at once, so the file is byte for byte the one
/// [`write`] made when it held them.
pub struct Spool {
    path: PathBuf,
    spill_path: PathBuf,
    spill: Option<BufWriter<File>>,
    width: u32,
    height: u32,
    /// Every `stride`th pixel of each frame goes into the palette's sample.
    stride: usize,
    sample: Vec<u8>,
    last: Vec<u8>,
    frames: usize,
    /// The spill's bytes, for the summary.
    spilled: u64,
}

impl Spool {
    /// Start a GIF of about `expected_frames` frames of `width` x `height`. The
    /// count only spreads the palette's sample; a GIF may end up longer or shorter.
    pub fn new(path: &Path, width: u32, height: u32, expected_frames: usize) -> Result<Self> {
        if u16::try_from(width).is_err() || u16::try_from(height).is_err() {
            bail!("{width} x {height} is too big for a GIF, whose sides stop at 65535");
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".frames~");
        let spill_path = path.with_file_name(name);
        let spill = File::create(&spill_path)
            .with_context(|| format!("failed to create {}", spill_path.display()))?;
        let pixels = width as usize * height as usize * expected_frames.max(1);
        let stride = (pixels / PALETTE_SAMPLES).max(1);
        Ok(Self {
            path: path.to_path_buf(),
            spill_path,
            spill: Some(BufWriter::new(spill)),
            width,
            height,
            stride,
            sample: Vec::with_capacity((pixels / stride + 1).min(PALETTE_SAMPLES * 2) * 4),
            last: vec![0; width as usize * height as usize * 3],
            frames: 0,
            spilled: 0,
        })
    }

    /// Add the next frame.
    pub fn push(&mut self, frame: &RgbImage) -> Result<()> {
        if frame.dimensions() != (self.width, self.height) {
            bail!("every frame of a GIF has to be the same size");
        }
        for pixel in frame.pixels().step_by(self.stride) {
            self.sample
                .extend_from_slice(&[pixel.0[0], pixel.0[1], pixel.0[2], 255]);
        }
        let raw = frame.as_raw();
        let delta: Vec<u8> = raw.iter().zip(&self.last).map(|(a, b)| a ^ b).collect();
        self.last.copy_from_slice(raw);
        let packed = miniz_oxide::deflate::compress_to_vec(&delta, SPILL_LEVEL);
        let spill = self.spill.as_mut().context("the GIF is already written")?;
        spill
            .write_all(&(packed.len() as u64).to_le_bytes())
            .and_then(|()| spill.write_all(&packed))
            .with_context(|| format!("failed to write {}", self.spill_path.display()))?;
        self.spilled += 8 + packed.len() as u64;
        self.frames += 1;
        Ok(())
    }

    /// Frames pushed so far.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The bytes the frames took on disk while they waited.
    pub fn spilled(&self) -> u64 {
        self.spilled
    }

    /// Learn the palette, encode every frame into the GIF at `fps`, and remove the
    /// spill.
    pub fn finish(mut self, fps: f32) -> Result<()> {
        if self.frames == 0 {
            bail!("a GIF needs at least one frame");
        }
        let spill = self.spill.take().context("the GIF is already written")?;
        spill
            .into_inner()
            .map_err(|e| e.into_error())
            .with_context(|| format!("failed to write {}", self.spill_path.display()))?;
        let mut spill = BufReader::new(
            File::open(&self.spill_path)
                .with_context(|| format!("failed to read {}", self.spill_path.display()))?,
        );

        let file = File::create(&self.path)
            .with_context(|| format!("failed to create {}", self.path.display()))?;
        let palette = Palette::learn(&std::mem::take(&mut self.sample));
        let mut encoder = gif::Encoder::new(
            BufWriter::new(file),
            self.width as u16,
            self.height as u16,
            &palette.rgb,
        )
        .context("failed to start the GIF")?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .context("failed to loop the GIF")?;

        let width = self.width as usize;
        let mut mapper = palette.mapper();
        let mut rgb = vec![0u8; width * self.height as usize * 3];
        let mut shown: Vec<u8> = Vec::new();
        for index in 0..self.frames {
            let unread = |e| anyhow!("failed to read frame {index} back from the spill: {e}");
            let mut len = [0u8; 8];
            spill.read_exact(&mut len).map_err(unread)?;
            let mut packed = vec![0u8; u64::from_le_bytes(len) as usize];
            spill.read_exact(&mut packed).map_err(unread)?;
            let delta = miniz_oxide::inflate::decompress_to_vec(&packed)
                .map_err(|e| anyhow!("frame {index} of the spill is damaged: {e:?}"))?;
            if delta.len() != rgb.len() {
                bail!("frame {index} of the spill is the wrong size");
            }
            for (pixel, d) in rgb.iter_mut().zip(&delta) {
                *pixel ^= d;
            }
            let (pixels, _) = rgb.as_chunks::<3>();
            let indices: Vec<u8> = pixels.iter().map(|&p| mapper.index(p)).collect();
            let delay = delay_of(index, fps);
            let piece = Piece::between(&shown, &indices, width);
            encoder
                .write_frame(&gif::Frame {
                    delay,
                    dispose: gif::DisposalMethod::Keep,
                    transparent: Some(UNCHANGED),
                    left: piece.left,
                    top: piece.top,
                    width: piece.width,
                    height: piece.height,
                    buffer: Cow::Owned(piece.buffer),
                    ..Default::default()
                })
                .with_context(|| format!("failed to write frame {index} of the GIF"))?;
            shown = indices;
        }
        encoder
            .into_inner()
            .context("failed to finish the GIF")?
            .flush()
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        Ok(())
    }
}

impl Drop for Spool {
    /// The spill goes however the GIF ends, written or not.
    fn drop(&mut self) {
        self.spill = None;
        let _ = std::fs::remove_file(&self.spill_path);
    }
}

/// Frame `index`'s delay in hundredths of a second, rounding each frame's end and
/// not its length: 12 fps alternates 8s and 9s and stays on time, where every
/// frame at 8 would run four seconds of game time in under 3.9.
fn delay_of(index: usize, fps: f32) -> u16 {
    let end_of = |frame: usize| (frame as f64 * 100.0 / f64::from(fps)).round() as u64;
    (end_of(index + 1) - end_of(index)).min(u64::from(u16::MAX)) as u16
}

/// One shared palette and how to find a colour in it.
struct Palette {
    quant: NeuQuant,
    rgb: Vec<u8>,
}

impl Palette {
    /// Learn from a spread of pixels through all the frames (`sample`, RGBA, as
    /// [`Spool::push`] gathers it), so that a colour that only appears late still
    /// gets a place.
    fn learn(sample: &[u8]) -> Self {
        let effort = if sample.len() / 4 < PALETTE_THOROUGH_BELOW {
            1
        } else {
            PALETTE_EFFORT
        };
        let quant = NeuQuant::new(effort, COLORS, sample);
        let mut rgb = quant.color_map_rgb();
        rgb.resize(256 * 3, 0);
        Self { quant, rgb }
    }

    fn mapper(&self) -> Mapper<'_> {
        Mapper {
            quant: &self.quant,
            known: HashMap::new(),
        }
    }
}

/// Colour to index, remembering, since a frame repeats few colours many times and
/// the search is a slow part of writing a GIF.
struct Mapper<'a> {
    quant: &'a NeuQuant,
    known: HashMap<[u8; 3], u8>,
}

impl Mapper<'_> {
    fn index(&mut self, rgb: [u8; 3]) -> u8 {
        let quant = self.quant;
        *self
            .known
            .entry(rgb)
            .or_insert_with(|| quant.index_of(&[rgb[0], rgb[1], rgb[2], 255]) as u8)
    }
}

/// What has to be written to get from one frame to the next: the box around what
/// changed, with what hasn't changed inside it left transparent.
#[derive(Debug, PartialEq)]
struct Piece {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    buffer: Vec<u8>,
}

impl Piece {
    /// The first frame is all of itself. A frame with nothing new is one see-through
    /// pixel: it still has to be there, to hold the time.
    fn between(shown: &[u8], next: &[u8], width: usize) -> Self {
        let height = next.len() / width;
        if shown.is_empty() {
            return Self {
                left: 0,
                top: 0,
                width: width as u16,
                height: height as u16,
                buffer: next.to_vec(),
            };
        }
        let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
        for y in 0..height {
            let row = y * width;
            for x in 0..width {
                if shown[row + x] != next[row + x] {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == width {
            return Self {
                left: 0,
                top: 0,
                width: 1,
                height: 1,
                buffer: vec![UNCHANGED],
            };
        }
        let mut buffer = Vec::with_capacity((x1 - x0 + 1) * (y1 - y0 + 1));
        for y in y0..=y1 {
            let row = y * width;
            for x in x0..=x1 {
                let index = next[row + x];
                buffer.push(if shown[row + x] == index {
                    UNCHANGED
                } else {
                    index
                });
            }
        }
        Self {
            left: x0 as u16,
            top: y0 as u16,
            width: (x1 - x0 + 1) as u16,
            height: (y1 - y0 + 1) as u16,
            buffer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifDecoder;
    use image::{AnimationDecoder, Rgb};
    use std::io::Cursor;

    /// A gradient for a backdrop, with a square that moves `offset` pixels across.
    fn frame(offset: u32) -> RgbImage {
        RgbImage::from_fn(48, 32, |x, y| {
            if (offset..offset + 6).contains(&x) && (10..16).contains(&y) {
                Rgb([230, 170, 70])
            } else {
                Rgb([(x * 4) as u8, (y * 6) as u8, 90])
            }
        })
    }

    fn written(name: &str, frames: &[RgbImage], fps: f32) -> Vec<u8> {
        let dir = std::env::temp_dir().join(format!("flint-gif-{}", std::process::id()));
        let path = dir.join(name);
        write(&path, frames, fps).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        bytes
    }

    fn decoded(bytes: &[u8]) -> Vec<image::Frame> {
        GifDecoder::new(Cursor::new(bytes))
            .unwrap()
            .into_frames()
            .collect_frames()
            .unwrap()
    }

    #[test]
    fn delays_keep_to_the_clock_however_they_round() {
        let cs: Vec<u16> = (0..12).map(|i| delay_of(i, 12.0)).collect();
        assert_eq!(cs.iter().map(|&d| u32::from(d)).sum::<u32>(), 100);
        assert!(cs.iter().all(|&d| d == 8 || d == 9));
        assert_eq!(delay_of(0, 50.0), 2);
    }

    #[test]
    fn a_pixel_that_never_changes_never_changes_in_the_file() {
        let frames: Vec<RgbImage> = (0..8).map(|i| frame(4 + i * 3)).collect();
        let shown = decoded(&written("still.gif", &frames, 12.0));
        assert_eq!(shown.len(), frames.len());

        let mut flickered = 0;
        for y in 0..32 {
            for x in 0..48 {
                let source: Vec<_> = frames.iter().map(|f| *f.get_pixel(x, y)).collect();
                if source.iter().all(|p| *p == source[0]) {
                    let out: Vec<_> = shown.iter().map(|f| *f.buffer().get_pixel(x, y)).collect();
                    flickered += usize::from(out.iter().any(|p| *p != out[0]));
                }
            }
        }
        assert_eq!(flickered, 0, "still pixels changed in the GIF");
    }

    #[test]
    fn what_moved_arrives_in_the_right_place() {
        let frames = [frame(4), frame(30)];
        let shown = decoded(&written("moved.gif", &frames, 12.0));
        let (x, y) = (32, 12);
        let close = |a: u8, b: u8| a.abs_diff(b) < 40;
        let at = |i: usize| *shown[i].buffer().get_pixel(x, y);
        assert!(close(at(1)[0], 230) && close(at(1)[1], 170) && close(at(1)[2], 70));
        assert!(!close(at(0)[1], 170));
        // ...and the square's old place goes back to the backdrop.
        let old = *shown[1].buffer().get_pixel(6, 12);
        assert!(
            !close(old[1], 170),
            "the moved square left a trail: {old:?}"
        );
    }

    #[test]
    fn it_comes_out_the_same_every_time() {
        let frames: Vec<RgbImage> = (0..4).map(|i| frame(i * 5)).collect();
        assert_eq!(
            written("a.gif", &frames, 10.0),
            written("b.gif", &frames, 10.0)
        );
    }

    #[test]
    fn a_held_frame_still_holds_its_time() {
        let frames = [frame(4), frame(4), frame(9)];
        let shown = decoded(&written("held.gif", &frames, 10.0));
        assert_eq!(shown.len(), 3);
        assert_eq!(shown[0].buffer(), shown[1].buffer());
        assert_ne!(shown[1].buffer(), shown[2].buffer());
    }

    #[test]
    fn a_piece_is_only_the_box_around_what_changed() {
        let shown = [1u8; 12];
        let mut next = shown;
        next[5] = 2; // x 1, y 1 in a 4-wide image
        next[6] = 3; // x 2, y 1
        let piece = Piece::between(&shown, &next, 4);
        assert_eq!(
            (piece.left, piece.top, piece.width, piece.height),
            (1, 1, 2, 1)
        );
        assert_eq!(piece.buffer, [2, 3]);
        let same = Piece::between(&shown, &shown, 4);
        assert_eq!(same.buffer, [UNCHANGED]);
    }

    #[test]
    fn the_spill_is_gone_however_the_gif_ends() {
        let dir = std::env::temp_dir().join(format!("flint-gif-spill-{}", std::process::id()));
        let path = dir.join("spilled.gif");
        let spill = dir.join("spilled.gif.frames~");

        let mut spool = Spool::new(&path, 48, 32, 3).unwrap();
        for i in 0..3 {
            spool.push(&frame(i * 7)).unwrap();
        }
        assert!(spill.exists(), "frames wait on disk while the GIF grows");
        assert_eq!(spool.frames(), 3);
        // A still backdrop's difference packs to a sliver of the raw frame.
        assert!(spool.spilled() < 3 * 48 * 32 * 3 / 4);
        spool.finish(10.0).unwrap();
        assert!(!spill.exists());
        assert_eq!(decoded(&std::fs::read(&path).unwrap()).len(), 3);

        let mut abandoned = Spool::new(&path, 48, 32, 3).unwrap();
        abandoned.push(&frame(0)).unwrap();
        drop(abandoned);
        assert!(
            !spill.exists(),
            "a render that fails leaves no spill behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frames_of_different_sizes_are_refused() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("flint-gif-mixed-{}.gif", std::process::id()));
        let mixed = [RgbImage::new(4, 4), RgbImage::new(8, 4)];
        assert!(write(&path, &mixed, 10.0).is_err());
        assert!(write(&path, &[], 10.0).is_err());
    }
}
