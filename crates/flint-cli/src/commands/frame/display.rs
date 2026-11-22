//! How a capture's image is shown: one mapping from stored texels to
//! display colour, used by `flint frame image` (a PNG, on the CPU) and by
//! `flint frame open`'s target viewer (`open/viewer.wgsl`, on the GPU, which
//! mirrors [`Display::map`] line for line).
//!
//! A texel is first [`Prepared`]: widened to four channels and divided by
//! its format's step (an 8-bit PNG's 255, a 16-bit PNG's 65535), so every
//! value is in the units the GPU held it in, except an integer format's,
//! which stays the integer. Then [`Display::auto`] picks how to show it
//! (what `flint frame image` always did), and [`Display::map`] turns one
//! texel into display-encoded RGB in 0..1.

use super::read::{Kind, Texels};
use flint_render::frame_capture::{FrameDescription, ImageDescription};

/// How a texel becomes a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Exposed, ACES-tonemapped and sRGB-encoded, as the composite does.
    Tonemap,
    /// Exposed, clamped to 0..1 and sRGB-encoded: linear values as they are.
    Linear,
    /// As stored, already display values (an 8-bit PNG, a mask, a blur).
    Stored,
    /// Depth stretched over `lo..hi`, near white; the clear value black.
    Depth,
    /// An integer id, a colour each; 0 black.
    Ids,
    /// A measure past 1 (a distance), on a log scale up to `max`.
    Log,
    /// A signed value, 0 at mid-grey, `max` at white.
    Signed,
    /// Scene depth turned into metres through the frame's camera (#1176),
    /// on a log scale over [`Metres::lo`]`..`[`Metres::hi`], near white;
    /// the clear value black. Without a camera it is [`Mode::Depth`].
    Metres,
}

impl Mode {
    pub const ALL: [Mode; 8] = [
        Mode::Tonemap,
        Mode::Linear,
        Mode::Stored,
        Mode::Depth,
        Mode::Metres,
        Mode::Ids,
        Mode::Log,
        Mode::Signed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Tonemap => "Tonemapped",
            Mode::Linear => "Linear",
            Mode::Stored => "As stored",
            Mode::Depth => "Depth",
            Mode::Ids => "Ids",
            Mode::Log => "Log",
            Mode::Signed => "Signed",
            Mode::Metres => "Metres",
        }
    }

    /// The number the viewer's shader knows it by.
    pub fn code(self) -> u32 {
        self as u32
    }

    /// Whether exposure changes what this mode shows.
    pub fn exposed(self) -> bool {
        matches!(self, Mode::Tonemap | Mode::Linear)
    }
}

/// Which of a texel's channels to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Rgb,
    R,
    G,
    B,
    A,
    /// Rec. 709 luminance of R, G, B.
    Luma,
}

impl Channel {
    pub const ALL: [Channel; 6] = [
        Channel::Rgb,
        Channel::R,
        Channel::G,
        Channel::B,
        Channel::A,
        Channel::Luma,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Channel::Rgb => "RGB",
            Channel::R => "R",
            Channel::G => "G",
            Channel::B => "B",
            Channel::A => "A",
            Channel::Luma => "Luma",
        }
    }

    pub fn code(self) -> u32 {
        self as u32
    }
}

/// An image's texels as the viewer and the PNG writer read them: four
/// channels a texel, in the GPU's units (see the module docs).
pub struct Prepared {
    pub width: u32,
    pub height: u32,
    /// How many channels the stored image has (1 to 4).
    pub channels: usize,
    pub kind: Kind,
    /// An integer format: values are the integers, not divided.
    pub integer: bool,
    /// What one stored step is (255, 65535, or 1): `rgba * scale` is the
    /// value as stored.
    pub scale: f32,
    /// `width * height * 4` values, row by row from the top; channels the
    /// image lacks are 0, and alpha 1.
    pub rgba: Vec<f32>,
}

impl Prepared {
    pub fn new(t: &Texels, image: &ImageDescription) -> Self {
        let integer =
            image.format.contains("Uint") || image.format.contains("Sint") || t.kind == Kind::Uint;
        let scale = if integer { 1.0 } else { t.scale() };
        let n = t.pixels();
        let c = t.channels.clamp(1, 4);
        let mut rgba = Vec::with_capacity(n * 4);
        for i in 0..n {
            let p = t.texel(i);
            for k in 0..4 {
                let v = if k < c {
                    p[k] / scale
                } else if k == 3 {
                    1.0
                } else {
                    0.0
                };
                rgba.push(v);
            }
        }
        Self {
            width: t.width,
            height: t.height,
            channels: c,
            kind: t.kind,
            integer,
            scale,
            rgba,
        }
    }

    pub fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn texel(&self, i: usize) -> [f32; 4] {
        let p = &self.rgba[i * 4..i * 4 + 4];
        [p[0], p[1], p[2], p[3]]
    }

    /// Texel `(x, y)`'s channels as stored (an 8-bit PNG's 0-255, a float's
    /// float), `channels` of them.
    pub fn stored(&self, x: u32, y: u32) -> Option<Vec<f32>> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        Some(
            self.rgba[i..i + self.channels]
                .iter()
                .map(|v| v * self.scale)
                .collect(),
        )
    }
}

/// The camera's depth planes (#1176): what turns a stored depth into
/// metres from the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthCamera {
    pub near: f32,
    pub far: f32,
    pub orthographic: bool,
}

impl DepthCamera {
    /// The camera `image` was drawn from, when the capture names one and
    /// the image is the scene's depth: `None` for a shadow map's layer
    /// (orthographic and linear already, in a light's metres), any image
    /// of a capture written before #1176, and a camera whose planes don't
    /// make a range.
    pub fn of(d: &FrameDescription, image: &ImageDescription) -> Option<Self> {
        let c = d.camera_of(image)?;
        let fits = c.far > c.near && (c.orthographic || c.near > 0.0);
        fits.then_some(Self {
            near: c.near,
            far: c.far,
            orthographic: c.orthographic,
        })
    }

    /// The distance along the view a stored depth `d` (0 at near, 1 at
    /// far: wgpu's convention, ADR 0055) is, in metres.
    pub fn metres(&self, d: f32) -> f32 {
        if self.orthographic {
            self.near + d * (self.far - self.near)
        } else {
            self.near * self.far / (self.far - d * (self.far - self.near))
        }
    }
}

/// Depth in metres (#1176): the camera that linearizes it, and the range
/// its log scale runs over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metres {
    pub camera: DepthCamera,
    /// The nearest and farthest drawn, metres.
    pub lo: f32,
    pub hi: f32,
}

/// The nearest a log scale of metres starts from: a camera's near plane
/// is never closer than this in a scene (`Camera::near` is 0.1 by default).
const NEAREST_M: f32 = 0.01;

impl Metres {
    /// A stored depth on the scale: 0 at `lo`, 1 at `hi`, by the logarithm
    /// of the distance, so each doubling of distance is one step of grey.
    pub fn scaled(&self, d: f32) -> f32 {
        let lo = self.lo.max(NEAREST_M);
        let hi = self.hi.max(lo);
        let z = self.camera.metres(d).max(lo);
        ((z / lo).ln() / (hi / lo).ln().max(1e-6)).clamp(0.0, 1.0)
    }
}

/// A length in metres to as many places as it needs: `0.42`, `38.2`.
pub fn metres_text(m: f32) -> String {
    if m.abs() < 10.0 {
        format!("{m:.2}")
    } else {
        format!("{m:.1}")
    }
}

/// How one image is shown: a [`Mode`] and what it needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub mode: Mode,
    /// Multiplies the value before a tonemap or a linear view.
    pub exposure: f32,
    /// The depths a [`Mode::Depth`] stretches over.
    pub lo: f32,
    pub hi: f32,
    /// The top of a [`Mode::Log`] or [`Mode::Signed`] range.
    pub max: f32,
    /// What a [`Mode::Metres`] needs, when the frame named its camera.
    pub metres: Option<Metres>,
}

impl Display {
    /// How `flint frame image` shows `p` (a float colour target at the
    /// frame's `exposure`), and a few words saying so. `camera` is the
    /// camera a scene depth was drawn from ([`DepthCamera::of`]): with one,
    /// depth is shown in metres; without (a version-1 capture, a shadow
    /// map) it is stretched over the stored values.
    pub fn auto(p: &Prepared, exposure: f32, camera: Option<DepthCamera>) -> (Self, String) {
        let mut d = Display {
            mode: Mode::Stored,
            exposure,
            lo: 0.0,
            hi: 1.0,
            max: 1.0,
            metres: None,
        };
        let n = p.pixels();
        let words = if p.integer {
            d.mode = Mode::Ids;
            "ids coloured".to_string()
        } else if p.kind == Kind::Depth {
            d.mode = Mode::Depth;
            let (lo, hi) = (0..n)
                .map(|i| p.rgba[i * 4])
                .filter(|v| drawn_depth(*v))
                .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
            if lo > hi {
                "depth: nothing drawn".into()
            } else if let Some(camera) = camera {
                d.lo = lo;
                d.hi = hi;
                // Depth grows with distance in both projections, so the
                // drawn range's ends are the nearest and farthest metres
                let m = Metres {
                    camera,
                    lo: camera.metres(lo),
                    hi: camera.metres(hi),
                };
                d.mode = Mode::Metres;
                d.metres = Some(m);
                format!(
                    "depth {}..{} m, log scale, near white",
                    metres_text(m.lo),
                    metres_text(m.hi)
                )
            } else {
                d.lo = lo;
                d.hi = hi;
                format!("depth {lo:.5}..{hi:.5} stretched, near white")
            }
        } else if p.channels >= 3 && p.kind == Kind::Float {
            d.mode = Mode::Tonemap;
            format!("tonemapped at exposure {exposure}")
        } else {
            // One or two channels, or 16-bit colour: as stored when it lies
            // in 0..1. Past 1 it is a measure (a distance, a count),
            // stretched on a log scale so that a far sky doesn't black out
            // the rest.
            let used = |i: usize| (0..p.channels).map(move |k| p.rgba[i * 4 + k]);
            let finite = || (0..n).flat_map(used).filter(|v| v.is_finite());
            let max = finite().fold(0.0f32, |m, v| m.max(v.abs()));
            let min = finite().fold(0.0f32, f32::min);
            let signed = min < 0.0;
            d.max = max;
            if signed {
                d.mode = Mode::Signed;
            } else if max > 1.0 {
                d.mode = Mode::Log;
            }
            match (p.channels, max > 1.0, signed) {
                (_, _, true) => "signed, 0 at mid-grey".into(),
                (_, true, _) => format!("log-stretched over 0..{max:.3}"),
                (1, ..) => "grey".into(),
                (2, ..) => "R and G as red and green".into(),
                _ => "as stored, 8-bit".into(),
            }
        };
        (d, words)
    }

    /// One texel (`rgba` in the GPU's units, `channels` of them stored) as
    /// display-encoded RGB in 0..1. `open/viewer.wgsl` is this, in WGSL.
    pub fn map(&self, rgba: [f32; 4], channels: usize) -> [f32; 3] {
        let rgb = match self.mode {
            Mode::Ids => id_colour(rgba[0].max(0.0) as u32),
            Mode::Metres if self.metres.is_some() => {
                let m = self.metres.expect("checked above");
                let v = rgba[0];
                let g = if drawn_depth(v) && m.lo <= m.hi {
                    1.0 - 0.85 * m.scaled(v)
                } else {
                    0.0
                };
                [g; 3]
            }
            Mode::Depth | Mode::Metres => {
                let v = rgba[0];
                let span = (self.hi - self.lo).max(1e-12);
                let g = if drawn_depth(v) && self.lo <= self.hi {
                    1.0 - 0.85 * ((v - self.lo) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                [g; 3]
            }
            Mode::Tonemap | Mode::Linear if channels >= 3 => {
                [rgba[0], rgba[1], rgba[2]].map(|x| self.exposed(x))
            }
            Mode::Tonemap | Mode::Linear => self.spread(rgba, channels, |x| self.exposed(x)),
            Mode::Stored => self.spread(rgba, channels, |x| x),
            Mode::Log => {
                let top = (1.0 + self.max.max(1e-6)).ln();
                self.spread(rgba, channels, |x| (1.0 + x.max(0.0)).ln() / top)
            }
            Mode::Signed => {
                let top = self.max.max(1.0);
                self.spread(rgba, channels, |x| 0.5 + 0.5 * x / top)
            }
        };
        rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) })
    }

    fn exposed(&self, x: f32) -> f32 {
        let x = x * self.exposure;
        match self.mode {
            Mode::Tonemap => linear_to_srgb(aces(x)),
            _ => linear_to_srgb(x),
        }
    }

    /// One channel to grey, two to red and green, more as they are.
    fn spread(&self, p: [f32; 4], channels: usize, f: impl Fn(f32) -> f32) -> [f32; 3] {
        match channels {
            1 => [f(p[0]); 3],
            2 => [f(p[0]), f(p[1]), 0.0],
            _ => [f(p[0]), f(p[1]), f(p[2])],
        }
    }
}

/// A texel with only `channel` of it kept, and how many channels it then
/// has: one channel is shown grey.
pub fn pick(p: [f32; 4], channels: usize, channel: Channel) -> ([f32; 4], usize) {
    let one = |v: f32| ([v, 0.0, 0.0, 1.0], 1);
    match channel {
        Channel::Rgb => (p, channels),
        Channel::R => one(p[0]),
        Channel::G => one(p[1]),
        Channel::B => one(p[2]),
        Channel::A => one(p[3]),
        Channel::Luma => one(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]),
    }
}

fn drawn_depth(v: f32) -> bool {
    v < 1.0 && v > 0.0
}

/// The composite's ACES fit (`aces_filmic`, flint-render's `tonemap.wgsl`).
pub(crate) fn aces(x: f32) -> f32 {
    let (a, b, c, d, e) = (2.51, 0.03, 2.43, 0.59, 0.14);
    ((x * (a * x + b)) / (x * (c * x + d) + e)).clamp(0.0, 1.0)
}

/// The sRGB transfer, as an sRGB target encodes.
pub(crate) fn linear_to_srgb(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        x * 12.92
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// A colour for an id: 0 black, the rest spaced round the hue circle by
/// the golden angle, so neighbouring ids never look alike.
pub(crate) fn id_colour(id: u32) -> [f32; 3] {
    if id == 0 {
        return [0.0; 3];
    }
    let hue = (id as f32 * 0.618_034).fract();
    hsv(hue, 0.62, 0.95)
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(format: &str) -> ImageDescription {
        let mut image = ImageDescription::default();
        image.format = format.into();
        image
    }

    #[test]
    fn an_8bit_png_reads_back_as_stored() {
        let t = Texels {
            width: 1,
            height: 1,
            channels: 3,
            kind: Kind::Unorm8,
            values: vec![255.0, 51.0, 0.0],
        };
        let p = Prepared::new(&t, &image("Rgba8UnormSrgb"));
        assert_eq!(p.texel(0), [1.0, 0.2, 0.0, 1.0]);
        assert_eq!(p.stored(0, 0), Some(vec![255.0, 51.0, 0.0]));
        assert_eq!(p.stored(1, 0), None);
        let (d, _) = Display::auto(&p, 1.0, None);
        assert_eq!(d.mode, Mode::Stored);
        assert_eq!(d.map(p.texel(0), p.channels), [1.0, 0.2, 0.0]);
    }

    #[test]
    fn exposure_brightens_a_tonemap() {
        let d = Display {
            mode: Mode::Tonemap,
            exposure: 1.0,
            lo: 0.0,
            hi: 1.0,
            max: 1.0,
            metres: None,
        };
        let px = [0.1, 0.1, 0.1, 1.0];
        let dim = d.map(px, 4)[0];
        let bright = Display { exposure: 4.0, ..d }.map(px, 4)[0];
        assert!(bright > dim + 0.1, "{dim} {bright}");
    }

    #[test]
    fn a_channel_alone_is_grey() {
        let (p, c) = pick([0.2, 0.4, 0.6, 0.8], 4, Channel::A);
        assert_eq!((p[0], c), (0.8, 1));
        let d = Display {
            mode: Mode::Stored,
            exposure: 1.0,
            lo: 0.0,
            hi: 1.0,
            max: 1.0,
            metres: None,
        };
        assert_eq!(d.map(p, c), [0.8; 3]);
        let (l, _) = pick([1.0, 1.0, 1.0, 1.0], 4, Channel::Luma);
        assert!((l[0] - 1.0).abs() < 1e-6);
    }

    /// The depth a perspective camera with these planes stores for a
    /// point `z` metres along its view (`Camera::perspective_matrix`).
    fn stored_at(c: &DepthCamera, z: f32) -> f32 {
        c.far * (z - c.near) / (z * (c.far - c.near))
    }

    #[test]
    fn a_perspective_depth_reads_back_in_metres() {
        let c = DepthCamera {
            near: 0.1,
            far: 1000.0,
            orthographic: false,
        };
        for z in [0.1, 0.4, 2.0, 38.2, 999.0] {
            let back = c.metres(stored_at(&c, z));
            assert!((back - z).abs() < z * 1e-4, "{z} m came back as {back}");
        }
        let ortho = DepthCamera {
            near: 0.5,
            far: 100.5,
            orthographic: true,
        };
        assert!((ortho.metres(0.25) - 25.5).abs() < 1e-4);
    }

    #[test]
    fn depth_in_metres_tells_far_things_apart_where_the_stretch_cannot() {
        // A prop at 20 m and the ground behind it at 30 m, the nearest
        // thing at 0.5 m: a perspective buffer keeps both far ones near 1
        let c = DepthCamera {
            near: 0.1,
            far: 200.0,
            orthographic: false,
        };
        let values: Vec<f32> = [0.5, 20.0, 30.0]
            .iter()
            .map(|z| stored_at(&c, *z))
            .chain([1.0])
            .collect();
        let t = Texels {
            width: 4,
            height: 1,
            channels: 1,
            kind: Kind::Depth,
            values,
        };
        let p = Prepared::new(&t, &image("Depth32Float"));
        let grey = |d: &Display, i: usize| (d.map(p.texel(i), 1)[0] * 255.0 + 0.5) as i32;

        let (stretch, words) = Display::auto(&p, 1.0, None);
        assert!(words.contains("stretched"), "{words}");
        let (metres, words) = Display::auto(&p, 1.0, Some(c));
        assert_eq!(metres.mode, Mode::Metres);
        assert!(words.starts_with("depth 0.50..30.0 m"), "{words}");

        assert_eq!(grey(&metres, 0), 255, "the nearest is white");
        assert_eq!(grey(&metres, 3), 0, "the clear value is black");
        let apart = |d: &Display| grey(d, 1) - grey(d, 2);
        assert!(
            apart(&stretch) <= 3,
            "the stretch keeps the prop and the ground {} levels apart",
            apart(&stretch)
        );
        assert!(
            apart(&metres) >= 15,
            "in metres they are {} levels apart",
            apart(&metres)
        );
    }

    #[test]
    fn metres_without_a_camera_shows_as_depth() {
        let d = Display {
            mode: Mode::Metres,
            exposure: 1.0,
            lo: 0.5,
            hi: 1.0,
            max: 1.0,
            metres: None,
        };
        assert_eq!(d.map([0.5, 0.0, 0.0, 1.0], 1), [1.0; 3]);
    }

    #[test]
    fn nan_shows_black() {
        let d = Display {
            mode: Mode::Stored,
            exposure: 1.0,
            lo: 0.0,
            hi: 1.0,
            max: 1.0,
            metres: None,
        };
        assert_eq!(d.map([f32::NAN, 0.5, 0.0, 1.0], 3), [0.0, 0.5, 0.0]);
    }
}
