//! `flint frame image`: a pass's image as a PNG anyone can look at.
//!
//! A capture keeps each target exactly as the GPU held it (ADR 0126
//! decision 2), which a person or an agent can't always look at: a float
//! target is HDR, a depth buffer is numbers near 1, a mask is ids. The
//! viewable copy is made here, never stored:
//!
//! - an 8-bit PNG is copied byte for byte (it is already viewable);
//! - float colour is exposed at the frame's `exposure`, ACES-tonemapped
//!   and sRGB-encoded, as the composite does;
//! - one float channel (SSAO, a blur) is grey, stretched only when it
//!   passes 1; two (a tensor) go to red and green;
//! - the scene's depth is turned into metres through the frame's camera
//!   (#1176) and shown on a log scale from the nearest drawn (white) to
//!   the farthest (dark), the clear value black; the `view` line names the
//!   range (`depth 0.42..38.2 m`). A capture that names no camera (written
//!   before #1176) and a shadow map's layer are stretched over the depths
//!   drawn instead, near white;
//! - integer ids (a 16-bit or u32 mask) take a colour each, 0 black.

use super::display::{DepthCamera, Display, Prepared};
use super::read::{self, Capture, Texels};
use super::{passes, ImageArgs};
use crate::output::{self, Answer};
use anyhow::{bail, Context, Result};
use flint_render::frame_capture::{FrameDescription, ImageDescription};
use serde_json::{json, Value};
use std::path::Path;

pub fn run(args: ImageArgs) -> Result<()> {
    let capture = Capture::open(&args.capture)?;
    let d = &capture.description;
    if d.images().is_empty() {
        bail!(
            "{} holds no images: it is a description only (`--frame-dump`); \
             `flint render --frame-capture <dir>` keeps each pass's image",
            Capture::shown(&args.capture)
        );
    }
    let mut written = Vec::new();
    if args.all {
        std::fs::create_dir_all(&args.output)
            .with_context(|| format!("creating {}", args.output.display()))?;
        for image in d.images() {
            let stem = Path::new(&image.file)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| image.target.clone());
            let ext = if args.raw {
                extension(&image.file)
            } else {
                "png".into()
            };
            let out = args.output.join(format!("{stem}.{ext}"));
            match write_one(&capture, image, &out, args.raw) {
                Ok(view) => written.push(entry(d, image, &out, &view)),
                // One that can't be viewed doesn't stop the rest
                Err(e) => crate::output::say!("skipped {}: {e:#}", image.file),
            }
        }
    } else {
        let Some(asked) = &args.pass else {
            bail!("name a pass (`flint frame show <capture> --passes` lists them), or give --all");
        };
        let image = choose(d, asked, args.target.as_deref(), args.step.as_deref())?;
        if let Some(dir) = args.output.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let view = write_one(&capture, image, &args.output, args.raw)?;
        written.push(entry(d, image, &args.output, &view));
    }
    let text: String = written
        .iter()
        .map(|w| {
            format!(
                "Wrote {}: {} after {} ({}, {})\n",
                w["path"].as_str().unwrap_or(""),
                w["target"].as_str().unwrap_or(""),
                w["pass"].as_str().unwrap_or(""),
                w["format"].as_str().unwrap_or(""),
                w["view"].as_str().unwrap_or("")
            )
        })
        .collect();
    output::emit(&Answer::new(text, json!({ "written": written })))
}

fn extension(file: &str) -> String {
    Path::new(file)
        .extension()
        .map_or("bin".into(), |e| e.to_string_lossy().into_owned())
}

fn entry(d: &FrameDescription, image: &ImageDescription, out: &Path, view: &str) -> Value {
    let pass = passes(d)
        .into_iter()
        .find(|p| p.pass.images.iter().any(|i| i.file == image.file))
        .map(|p| p.key());
    json!({
        "path": out.display().to_string().replace('\\', "/"),
        "from": image.file,
        "pass": pass,
        "target": image.target,
        "step": image.step,
        "layer": image.layer,
        "format": image.format,
        "size": image.size,
        "view": view,
    })
}

/// The image `asked` names: a pass (its first image unless `target` or
/// `step` says), or an image's file name with or without `targets/`.
fn choose<'a>(
    d: &'a FrameDescription,
    asked: &str,
    target: Option<&str>,
    step: Option<&str>,
) -> Result<&'a ImageDescription> {
    let file_named = d.images().into_iter().find(|i| {
        let name = Path::new(&i.file)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned());
        let stem = Path::new(&i.file)
            .file_stem()
            .map(|f| f.to_string_lossy().into_owned());
        i.file == asked || name.as_deref() == Some(asked) || stem.as_deref() == Some(asked)
    });
    if let Some(image) = file_named {
        return Ok(image);
    }
    let all = passes(d);
    let named: Vec<_> = all
        .iter()
        .filter(|p| p.pass.name.eq_ignore_ascii_case(asked) || p.key().eq_ignore_ascii_case(asked))
        .collect();
    if named.is_empty() {
        let names: Vec<&str> = all
            .iter()
            .filter(|p| !p.pass.images.is_empty())
            .map(|p| p.pass.name.as_str())
            .collect();
        let hint = flint_core::diagnostic::did_you_mean(asked, names.iter().copied())
            .map_or(String::new(), |h| format!(": {h}"));
        bail!(
            "no pass or image is named {asked:?}{hint} (passes with images: {})",
            {
                let mut n = names.clone();
                n.dedup();
                n.join(", ")
            }
        );
    }
    let images: Vec<&ImageDescription> = named.iter().flat_map(|p| p.pass.images.iter()).collect();
    if images.is_empty() {
        bail!(
            "{asked} wrote no image this frame{}",
            named[0]
                .pass
                .skipped
                .as_deref()
                .map_or(String::new(), |why| format!(" (skipped: {why})"))
        );
    }
    let fits = |i: &&ImageDescription| {
        target.is_none_or(|t| i.target == t)
            && step.is_none_or(|s| {
                i.step
                    .as_deref()
                    .is_some_and(|is| is.eq_ignore_ascii_case(s))
            })
    };
    match images.iter().copied().find(fits) {
        Some(image) => Ok(image),
        None => {
            let list: Vec<String> = images
                .iter()
                .map(|i| match &i.step {
                    Some(s) => format!("{} (step {s})", i.target),
                    None => i.target.clone(),
                })
                .collect();
            bail!(
                "{asked} wrote no image that fits: it wrote {}",
                list.join(", ")
            )
        }
    }
}

/// Write `image`'s viewable copy (or, `raw`, its file) to `out`; what kind
/// of copy it is.
fn write_one(capture: &Capture, image: &ImageDescription, out: &Path, raw: bool) -> Result<String> {
    let src = capture.file(image);
    if !src.is_file() {
        bail!("{} is missing from the capture", image.file);
    }
    if raw || image.stored == "png" {
        std::fs::copy(&src, out).with_context(|| format!("writing {}", out.display()))?;
        return Ok(if raw {
            "as stored".into()
        } else {
            "copied".into()
        });
    }
    let texels = read::texels(&src, image)?;
    let d = &capture.description;
    let (rgba, view) = viewable(&texels, image, exposure(d), DepthCamera::of(d, image));
    rgba.save(out)
        .with_context(|| format!("writing {}", out.display()))?;
    Ok(view)
}

/// The exposure the frame's composite drew at.
pub(crate) fn exposure(d: &FrameDescription) -> f32 {
    d.frame
        .post_process
        .get("exposure")
        .and_then(Value::as_f64)
        .map_or(1.0, |e| e as f32)
}

/// `texels` as an 8-bit RGBA picture, and what was done to make it one
/// ([`Display::auto`], the same choice `flint frame open` starts from);
/// `camera` turns a scene depth into metres.
pub(crate) fn viewable(
    t: &Texels,
    image: &ImageDescription,
    exposure: f32,
    camera: Option<DepthCamera>,
) -> (image::RgbaImage, String) {
    let p = Prepared::new(t, image);
    let (display, view) = Display::auto(&p, exposure, camera);
    let mut out = vec![0u8; p.pixels() * 4];
    for i in 0..p.pixels() {
        let rgb = display.map(p.texel(i), p.channels);
        for (k, v) in rgb.iter().enumerate() {
            out[i * 4 + k] = (v * 255.0 + 0.5) as u8;
        }
        out[i * 4 + 3] = 255;
    }
    let rgba = image::RgbaImage::from_raw(t.width, t.height, out).expect("sized above");
    (rgba, view)
}

#[cfg(test)]
mod tests {
    use super::super::read::Kind;
    use super::*;

    fn image(format: &str) -> ImageDescription {
        let mut image = ImageDescription::default();
        image.target = "t".into();
        image.format = format.into();
        image
    }

    #[test]
    fn hdr_is_tonemapped_and_depth_stretched() {
        let hdr = Texels {
            width: 2,
            height: 1,
            channels: 4,
            kind: Kind::Float,
            values: vec![0.0, 0.0, 0.0, 1.0, 100.0, 1.0, 0.18, 1.0],
        };
        let (rgba, view) = viewable(&hdr, &image("Rgba16Float"), 1.0, None);
        assert!(view.contains("tonemapped"));
        assert_eq!(rgba.get_pixel(0, 0).0, [0, 0, 0, 255]);
        let p = rgba.get_pixel(1, 0).0;
        assert_eq!(p[0], 255, "a bright value saturates");
        assert!(p[2] > 60 && p[2] < 200, "mid grey stays mid: {p:?}");
        let depth = Texels {
            width: 3,
            height: 1,
            channels: 1,
            kind: Kind::Depth,
            values: vec![0.9, 0.99, 1.0],
        };
        let (rgba, _) = viewable(&depth, &image("Depth32Float"), 1.0, None);
        assert_eq!(rgba.get_pixel(0, 0).0[0], 255, "nearest is white");
        assert!(rgba.get_pixel(1, 0).0[0] < 60, "farthest is dark");
        assert_eq!(rgba.get_pixel(2, 0).0[0], 0, "the clear value is black");
    }

    #[test]
    fn ids_take_a_colour_each() {
        let ids = Texels {
            width: 3,
            height: 1,
            channels: 2,
            kind: Kind::Bits16,
            values: vec![0.0, 0.0, 1.0, 0.0, 2.0, 0.0],
        };
        let (rgba, view) = viewable(&ids, &image("Rg16Uint"), 1.0, None);
        assert_eq!(view, "ids coloured");
        assert_eq!(rgba.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_ne!(rgba.get_pixel(1, 0), rgba.get_pixel(2, 0));
    }

    #[test]
    fn a_distance_is_log_stretched_so_the_near_stays_seen() {
        let far = Texels {
            width: 3,
            height: 1,
            channels: 1,
            kind: Kind::Float,
            values: vec![0.0, 9.0, 900.0],
        };
        let (rgba, view) = viewable(&far, &image("R32Float"), 1.0, None);
        assert!(view.starts_with("log-stretched"), "{view}");
        assert_eq!(rgba.get_pixel(2, 0).0[0], 255);
        // 9 m of 900 is a third of the way up, not 1%
        let near = rgba.get_pixel(1, 0).0[0];
        assert!(near > 70 && near < 110, "{near}");
    }
}
