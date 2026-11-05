//! Animated GIFs for judging motion (#216), which sets one rule: nothing may move
//! in the file that didn't move in the render.
//!
//! The usual way of writing a GIF breaks it. `image`'s encoder quantizes each frame
//! to a palette of its own, so a patch of sky comes out a slightly different colour
//! every frame, and shimmer is the very thing someone is looking at a living prop to
//! judge. So every frame here shares one palette, with no dithering, and a pixel the
//! render didn't change is written as transparent over the frame before it. A still
//! pixel can't flicker, and stillness compresses to almost nothing.

use anyhow::{bail, Context, Result};
use color_quant::NeuQuant;
use image::RgbImage;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

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

/// Write `frames` as a GIF that loops forever at `fps`.
pub fn write(path: &Path, frames: &[RgbImage], fps: f32) -> Result<()> {
    let Some(first) = frames.first() else {
        bail!("a GIF needs at least one frame");
    };
    let (width, height) = first.dimensions();
    let (Ok(gif_width), Ok(gif_height)) = (u16::try_from(width), u16::try_from(height)) else {
        bail!("{width} x {height} is too big for a GIF, whose sides stop at 65535");
    };
    if frames.iter().any(|f| f.dimensions() != (width, height)) {
        bail!("every frame of a GIF has to be the same size");
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let palette = Palette::learn(frames);
    let mut encoder = gif::Encoder::new(BufWriter::new(file), gif_width, gif_height, &palette.rgb)
        .context("failed to start the GIF")?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .context("failed to loop the GIF")?;

    let mut mapper = palette.mapper();
    let mut shown: Vec<u8> = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        let indices: Vec<u8> = frame.pixels().map(|p| mapper.index(p.0)).collect();
        let delay = delay_of(index, fps);
        let piece = Piece::between(&shown, &indices, width as usize);
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
    Ok(())
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
    /// Learn from a spread of pixels through all the frames, so that a colour that
    /// only appears late still gets a place.
    fn learn(frames: &[RgbImage]) -> Self {
        let pixels: usize = frames.iter().map(|f| f.pixels().len()).sum();
        let stride = (pixels / PALETTE_SAMPLES).max(1);
        let mut sample = Vec::with_capacity(pixels / stride * 4);
        for frame in frames {
            for pixel in frame.pixels().step_by(stride) {
                sample.extend_from_slice(&[pixel.0[0], pixel.0[1], pixel.0[2], 255]);
            }
        }
        let effort = if sample.len() / 4 < PALETTE_THOROUGH_BELOW {
            1
        } else {
            PALETTE_EFFORT
        };
        let quant = NeuQuant::new(effort, COLORS, &sample);
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
    fn frames_of_different_sizes_are_refused() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("flint-gif-mixed-{}.gif", std::process::id()));
        let mixed = [RgbImage::new(4, 4), RgbImage::new(8, 4)];
        assert!(write(&path, &mixed, 10.0).is_err());
        assert!(write(&path, &[], 10.0).is_err());
    }
}
