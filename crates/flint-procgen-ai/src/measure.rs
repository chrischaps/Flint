//! How far one image is from another, cheaply, so a refine can tell whether
//! a round moved its asset toward the reference.
//!
//! Three coarse measures, each scaled to 0..1, and their mean as a score out
//! of 100 (0 = the same image):
//!
//! - **mean colour**: the distance between the two images' average sRGB
//!   colours, over the largest distance there is (black to white).
//! - **histogram**: half the L1 distance between 16-bin luminance histograms,
//!   so how much of one image's tonal spread would have to move to match the
//!   other's.
//! - **layout**: the mean absolute difference of the two images' luminance
//!   at 16x16, which is where the light and dark masses sit (a silhouette,
//!   a crown's outline, a brick course).
//!
//! None of these is perceptual. They are a ruler, not a judge: good enough
//! to say "closer" or "further" between two rounds of the same refine.

use image::imageops::FilterType;
use image::{GrayImage, RgbImage};

/// Bins in the luminance histogram.
const BINS: usize = 16;
/// Side of the thumbnail the layout term compares.
const LAYOUT: u32 = 16;
/// The largest mean-colour distance: black to white in sRGB 0..255.
const MAX_RGB_DISTANCE: f64 = 441.672_955_930_063_7; // 255 * sqrt(3)

/// How far an image is from a reference. Lower is closer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance {
    /// The mean of the three terms below, times 100: 0 is identical, 100
    /// is as far apart as these measures go.
    pub score: f64,
    /// Mean-colour distance in sRGB units (0..441.7).
    pub mean_colour: f64,
    /// Luminance-histogram distance, 0..1.
    pub histogram: f64,
    /// 16x16 luminance-layout distance, 0..1.
    pub layout: f64,
    /// The reference's average colour.
    pub reference_mean: [u8; 3],
    /// The measured image's average colour.
    pub current_mean: [u8; 3],
}

/// The distance from `current` to `reference`, both encoded images (PNG,
/// JPEG, ...). `None` when either fails to decode or is empty.
pub fn distance(reference: &[u8], current: &[u8]) -> Option<Distance> {
    let r = decode(reference)?;
    let c = decode(current)?;
    Some(distance_rgb(&r, &c))
}

/// [`distance`] for decoded images.
pub fn distance_rgb(reference: &RgbImage, current: &RgbImage) -> Distance {
    let (rm, cm) = (mean(reference), mean(current));
    let mean_colour = rm
        .iter()
        .zip(cm)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt();

    let (rh, ch) = (histogram(reference), histogram(current));
    let histogram = rh.iter().zip(ch).map(|(a, b)| (a - b).abs()).sum::<f64>() / 2.0;

    let (rl, cl) = (thumbnail(reference), thumbnail(current));
    let layout = rl
        .pixels()
        .zip(cl.pixels())
        .map(|(a, b)| (f64::from(a.0[0]) - f64::from(b.0[0])).abs())
        .sum::<f64>()
        / f64::from(LAYOUT * LAYOUT)
        / 255.0;

    let score = (mean_colour / MAX_RGB_DISTANCE + histogram + layout) / 3.0 * 100.0;
    Distance {
        score,
        mean_colour,
        histogram,
        layout,
        reference_mean: rm.map(|v| v.round().clamp(0.0, 255.0) as u8),
        current_mean: cm.map(|v| v.round().clamp(0.0, 255.0) as u8),
    }
}

/// The average sRGB colour of an encoded image, ignoring alpha.
pub fn mean_rgb(bytes: &[u8]) -> Option<[u8; 3]> {
    decode(bytes).map(|img| mean(&img).map(|v| v.round().clamp(0.0, 255.0) as u8))
}

fn decode(bytes: &[u8]) -> Option<RgbImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgb8();
    (img.width() > 0 && img.height() > 0).then_some(img)
}

fn mean(img: &RgbImage) -> [f64; 3] {
    let n = f64::from(img.width()) * f64::from(img.height());
    let mut sum = [0.0f64; 3];
    for p in img.pixels() {
        for (s, c) in sum.iter_mut().zip(p.0) {
            *s += f64::from(c);
        }
    }
    sum.map(|s| s / n)
}

/// Rec. 709 weights on the sRGB values: a tonal measure, not a colorimetric
/// one, which is all the histogram and layout terms need.
fn luma(p: [u8; 3]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn histogram(img: &RgbImage) -> [f64; BINS] {
    let mut bins = [0.0f64; BINS];
    for p in img.pixels() {
        let b = ((luma(p.0) / 256.0) * BINS as f64) as usize;
        bins[b.min(BINS - 1)] += 1.0;
    }
    let n = f64::from(img.width()) * f64::from(img.height());
    bins.map(|b| b / n)
}

fn thumbnail(img: &RgbImage) -> GrayImage {
    let gray = GrayImage::from_fn(img.width(), img.height(), |x, y| {
        image::Luma([luma(img.get_pixel(x, y).0).round().clamp(0.0, 255.0) as u8])
    });
    image::imageops::resize(&gray, LAYOUT, LAYOUT, FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8) -> RgbImage {
        RgbImage::from_pixel(32, 32, image::Rgb([r, g, b]))
    }

    #[test]
    fn an_image_is_zero_from_itself() {
        let img = solid(120, 80, 40);
        let d = distance_rgb(&img, &img);
        assert_eq!(d.score, 0.0);
        assert_eq!(d.current_mean, [120, 80, 40]);
    }

    #[test]
    fn black_and_white_are_as_far_apart_as_it_goes() {
        let d = distance_rgb(&solid(0, 0, 0), &solid(255, 255, 255));
        assert!((d.mean_colour - MAX_RGB_DISTANCE).abs() < 1e-9);
        assert!((d.histogram - 1.0).abs() < 1e-9);
        assert!((d.layout - 1.0).abs() < 1e-9);
        assert!((d.score - 100.0).abs() < 1e-9);
    }

    #[test]
    fn layout_sees_where_the_mass_is_when_colour_does_not() {
        // Same pixels, mirrored: identical mean and histogram, different layout.
        let left = RgbImage::from_fn(32, 32, |x, _| {
            if x < 16 {
                image::Rgb([0, 0, 0])
            } else {
                image::Rgb([255, 255, 255])
            }
        });
        let mut right = left.clone();
        image::imageops::flip_horizontal_in_place(&mut right);
        let d = distance_rgb(&left, &right);
        assert!(d.mean_colour < 1e-9 && d.histogram < 1e-9);
        assert!(d.layout > 0.9, "{d:?}");
    }

    #[test]
    fn closer_colours_score_lower() {
        let r = solid(40, 110, 50);
        let near = distance_rgb(&r, &solid(50, 100, 50)).score;
        let far = distance_rgb(&r, &solid(200, 60, 40)).score;
        assert!(near < far, "{near} vs {far}");
    }

    #[test]
    fn undecodable_bytes_have_no_distance() {
        assert!(distance(b"nope", b"nope").is_none());
        assert!(mean_rgb(b"nope").is_none());
    }
}
