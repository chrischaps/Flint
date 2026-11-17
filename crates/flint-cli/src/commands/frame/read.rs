//! Reading a capture: its manifest, checked for its version, and its images
//! as numbers (ADR 0126 decisions 2 and 3).
//!
//! Only the folder is read: no GPU, no scene, no schemas, so a capture from
//! another machine or an older engine reads the same.

use anyhow::{bail, Context, Result};
use flint_render::frame_capture::{FrameDescription, ImageDescription, FORMAT, VERSION};
use std::path::{Path, PathBuf};

/// A capture as read: where it is and what its manifest says.
pub struct Capture {
    /// The folder its image paths are relative to.
    pub dir: PathBuf,
    pub description: FrameDescription,
}

impl Capture {
    /// Read the capture at `path`: a capture folder (its `manifest.json`),
    /// the manifest itself, or a `--frame-dump` file (a description with
    /// no images).
    pub fn open(path: &Path) -> Result<Self> {
        let manifest = if path.is_dir() {
            let manifest = path.join("manifest.json");
            if !manifest.is_file() {
                if path.join("targets").is_dir() {
                    bail!(
                        "{} has images but no manifest.json: the capture was interrupted \
                         before it finished (the manifest is written last)",
                        path.display()
                    );
                }
                bail!(
                    "{} is not a frame capture: it has no manifest.json \
                     (make one with `flint render <scene> --frame-capture <dir>`)",
                    path.display()
                );
            }
            manifest
        } else if path.is_file() {
            path.to_path_buf()
        } else {
            bail!("{}: no such capture", path.display());
        };
        let shown = Self::shown(&manifest);
        let text =
            std::fs::read_to_string(&manifest).with_context(|| format!("reading {shown}"))?;
        let description = parse(&text).with_context(|| shown.clone())?;
        let dir = manifest
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self { dir, description })
    }

    /// How the capture is named in output: the path as given, `/`-separated.
    pub fn shown(path: &Path) -> String {
        path.display().to_string().replace('\\', "/")
    }

    /// An image's file.
    pub fn file(&self, image: &ImageDescription) -> PathBuf {
        self.dir.join(&image.file)
    }
}

/// A manifest's text as a description: refused when it is not a Flint
/// frame or is newer than this reader (ADR 0126 decision 3). Older
/// versions read: what they lack is defaulted.
pub fn parse(text: &str) -> Result<FrameDescription> {
    let value: serde_json::Value = serde_json::from_str(text).context("not JSON")?;
    let format = value.get("format").and_then(|f| f.as_str()).unwrap_or("");
    if format != FORMAT {
        bail!(
            "not a Flint frame: its `format` is {:?}, not {FORMAT:?}",
            value
                .get("format")
                .map_or("missing".into(), |f| f.to_string())
        );
    }
    let version = value.get("version").and_then(|v| v.as_u64());
    match version {
        None => bail!("its `version` is missing or not a whole number"),
        Some(v) if v > VERSION as u64 => bail!(
            "this is a version {v} capture, and this flint reads version {VERSION} and older: \
             read it with a newer engine"
        ),
        Some(_) => {}
    }
    serde_json::from_value(value).context("its manifest doesn't read as a frame description")
}

/// How an image's texels are kept, as the reader sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 8-bit PNG: values 0-255.
    Unorm8,
    /// 16-bit PNG: values 0-65535, normalized or integers by the format.
    Bits16,
    /// OpenEXR half or f32 colour.
    Float,
    /// OpenEXR f32 depth (`Z`).
    Depth,
    /// OpenEXR u32.
    Uint,
}

/// An image's texels as numbers, interleaved, `channels` a texel.
pub struct Texels {
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub kind: Kind,
    /// Each texel's channels, as stored (an 8-bit PNG's 0-255, a float
    /// EXR's floats, a u32's integers).
    pub values: Vec<f32>,
}

impl Texels {
    pub fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// One texel's channels.
    pub fn texel(&self, i: usize) -> &[f32] {
        &self.values[i * self.channels..(i + 1) * self.channels]
    }

    /// What one step of a value is, for normalizing a difference: 255 or
    /// 65535 for the PNGs, 1 for the rest.
    pub fn scale(&self) -> f32 {
        match self.kind {
            Kind::Unorm8 => 255.0,
            Kind::Bits16 => 65535.0,
            _ => 1.0,
        }
    }
}

/// Read one image of a capture as numbers. `raw` images have no layout to
/// read: refused, naming the format.
pub fn texels(path: &Path, image: &ImageDescription) -> Result<Texels> {
    match image.stored.as_str() {
        "png" | "png16" => png(path),
        "exr-half" | "exr-f32" | "exr-u32" => exr(path),
        other => bail!(
            "{} is stored {other:?} ({}): it has no texel layout this reader knows",
            path.display(),
            image.format
        ),
    }
}

fn png(path: &Path) -> Result<Texels> {
    use image::DynamicImage as D;
    let img = image::open(path).with_context(|| format!("reading {}", path.display()))?;
    let (width, height) = (img.width(), img.height());
    let (channels, kind, values): (usize, Kind, Vec<f32>) = match img {
        D::ImageLuma8(i) => (
            1,
            Kind::Unorm8,
            i.into_raw().into_iter().map(f32::from).collect(),
        ),
        D::ImageLumaA8(i) => (
            2,
            Kind::Unorm8,
            i.into_raw().into_iter().map(f32::from).collect(),
        ),
        D::ImageRgb8(i) => (
            3,
            Kind::Unorm8,
            i.into_raw().into_iter().map(f32::from).collect(),
        ),
        D::ImageRgba8(i) => (
            4,
            Kind::Unorm8,
            i.into_raw().into_iter().map(f32::from).collect(),
        ),
        D::ImageLuma16(i) => (
            1,
            Kind::Bits16,
            i.into_raw().into_iter().map(f32::from).collect(),
        ),
        D::ImageLumaA16(i) => (
            2,
            Kind::Bits16,
            i.into_raw().into_iter().map(f32::from).collect(),
        ),
        D::ImageRgb16(i) => (
            3,
            Kind::Bits16,
            i.into_raw().into_iter().map(f32::from).collect(),
        ),
        D::ImageRgba16(i) => (
            4,
            Kind::Bits16,
            i.into_raw().into_iter().map(f32::from).collect(),
        ),
        other => (
            4,
            Kind::Unorm8,
            other
                .to_rgba8()
                .into_raw()
                .into_iter()
                .map(f32::from)
                .collect(),
        ),
    };
    Ok(Texels {
        width,
        height,
        channels,
        kind,
        values,
    })
}

fn exr(path: &Path) -> Result<Texels> {
    use exr::prelude::*;
    let image = read_first_flat_layer_from_file(path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let size = image.layer_data.size;
    let list = &image.layer_data.channel_data.list;
    // R, G, B, A in that order whatever order the file sorts them, then
    // anything else (Z) after
    let rank = |name: &str| match name {
        "R" => 0,
        "G" => 1,
        "B" => 2,
        "A" => 3,
        _ => 4,
    };
    let mut order: Vec<usize> = (0..list.len()).collect();
    order.sort_by_key(|&i| rank(&list[i].name.to_string()));
    let depth = list.len() == 1 && list[0].name.to_string() == "Z";
    let mut kind = if depth { Kind::Depth } else { Kind::Float };
    let columns: Vec<Vec<f32>> = order
        .iter()
        .map(|&i| match &list[i].sample_data {
            FlatSamples::F16(v) => v.iter().map(|h| h.to_f32()).collect(),
            FlatSamples::F32(v) => v.clone(),
            FlatSamples::U32(v) => {
                kind = Kind::Uint;
                v.iter().map(|&u| u as f32).collect()
            }
        })
        .collect();
    let pixels = size.0 * size.1;
    let channels = columns.len();
    let mut values = Vec::with_capacity(pixels * channels);
    for p in 0..pixels {
        for column in &columns {
            values.push(column.get(p).copied().unwrap_or(0.0));
        }
    }
    Ok(Texels {
        width: size.0 as u32,
        height: size.1 as u32,
        channels,
        kind,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_version_is_refused_naming_both() {
        let e = parse(r#"{"format": "flintframe", "version": 7}"#).unwrap_err();
        let text = format!("{e:#}");
        assert!(text.contains("version 7"), "{text}");
        assert!(
            text.contains(&format!("version {VERSION} and older")),
            "{text}"
        );
    }

    #[test]
    fn an_older_or_sparser_manifest_reads_with_defaults() {
        let d = parse(r#"{"format": "flintframe", "version": 1, "later": {"x": 1}}"#).unwrap();
        assert!(d.passes.is_empty());
        assert!(d.scene.is_none());
    }

    #[test]
    fn something_else_is_not_a_frame() {
        let e = parse(r#"{"format": "gltf", "version": 1}"#).unwrap_err();
        assert!(e.to_string().contains("not a Flint frame"));
        assert!(parse(r#"{"format": "flintframe"}"#).is_err());
    }
}
