//! Writing a captured frame to its folder (#1085, ADR 0126 decision 2).
//!
//! ```text
//! <name>.flintframe/
//!   manifest.json        the FrameDescription, written last: present means complete
//!   targets/NN-<target>.<ext>
//! ```
//!
//! Each target is stored once, exactly: 8-bit as PNG, 16-bit integers as a
//! 16-bit PNG, half floats as OpenEXR half, depth and 32-bit floats as
//! OpenEXR f32, 32-bit integers as OpenEXR u32, the EXRs ZIP-compressed. A
//! viewable copy (HDR tonemapped, depth linearized) is the reader's to make
//! (`flint frame image`, #1087), not stored beside it. Native only: the web
//! build has no `exr` and doesn't capture.
//!
//! Encoding runs on as many threads as the machine has, off whatever thread
//! calls it; a window's host calls it on a writer thread of its own (#1086).

// A write path: it reads only the folder it is about to write into, which is
// on the disk by definition and never in a bundle or the vfs (#97)
#![allow(clippy::disallowed_methods)]

use crate::frame_capture::{CaptureDescription, FrameDescription};
use crate::frame_taps::{CapturedFrame, CapturedImage, Storage};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use web_time::Instant;

/// What [`write_capture`] wrote.
#[derive(Debug, Clone)]
pub struct WrittenCapture {
    /// The manifest's path.
    pub manifest: PathBuf,
    pub images: usize,
    /// The image files' bytes on disk.
    pub file_bytes: u64,
    /// The texels read back.
    pub raw_bytes: u64,
}

/// The four files of a capture's `replay/` (ADR 0126 decision 4), as
/// [`write_replay`] writes them.
pub struct ReplayFiles<'a> {
    /// The world as it stood, in a scene's encoding.
    pub world_toml: &'a str,
    pub inputs: &'a crate::frame_replay::ReplayInputs,
    /// `skinning.bin`: the palettes `inputs.renderer.skinning` indexes.
    pub skinning: &'a [u8],
    /// `particles.bin`: the packed stream `inputs.particles` indexes.
    pub particles: &'a [u8],
    /// The styles' history textures (#1136), each a file under `replay/`
    /// (`history/ascii.tone.cells.bin`) that `inputs.renderer.style_history`
    /// names, with its bytes.
    pub history: &'a [(String, Vec<u8>)],
}

impl ReplayFiles<'_> {
    /// Every file [`write_replay`] writes, under the capture's folder: the
    /// four of [`REPLAY_FILES`], then each history file.
    pub fn listed(&self) -> Vec<String> {
        REPLAY_FILES
            .iter()
            .map(|f| f.to_string())
            .chain(self.history.iter().map(|(f, _)| format!("replay/{f}")))
            .collect()
    }
}

/// The files [`write_replay`] always writes, under the capture's folder.
pub const REPLAY_FILES: [&str; 4] = [
    "replay/world.toml",
    "replay/inputs.json",
    "replay/skinning.bin",
    "replay/particles.bin",
];

/// Write `files` into `dir/replay/` (made if missing). The player (#1086)
/// and `flint render --frame-capture` (#1089) both write through it, before
/// [`write_capture`], which keeps the folder and writes the manifest last.
/// A `history/` an earlier capture into the folder left is replaced.
pub fn write_replay(dir: &Path, files: &ReplayFiles) -> io::Result<()> {
    let replay = dir.join("replay");
    std::fs::create_dir_all(&replay)?;
    std::fs::write(replay.join("world.toml"), files.world_toml)?;
    let json = serde_json::to_string_pretty(files.inputs).map_err(io::Error::other)?;
    std::fs::write(replay.join("inputs.json"), json)?;
    std::fs::write(replay.join("skinning.bin"), files.skinning)?;
    std::fs::write(replay.join("particles.bin"), files.particles)?;
    let history = replay.join("history");
    if history.is_dir() {
        std::fs::remove_dir_all(&history)?;
    }
    for (file, bytes) in files.history {
        // Only a plain name under `history/`: the states name their files
        let name = Path::new(file);
        let plain = name.starts_with("history")
            && name
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !plain {
            return Err(io::Error::other(format!(
                "a style history file named {file:?} isn't under history/"
            )));
        }
        let path = replay.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

/// Write `frame`'s images into `dir` (made if missing) under the files
/// `description` names for them, then `description` as `manifest.json`,
/// last, with what the capture cost in its `capture`. A folder that already
/// holds a capture has its `targets/` and manifest replaced; a folder that
/// holds anything else is refused, so a wrong path can't empty it.
pub fn write_capture(
    dir: &Path,
    description: &mut FrameDescription,
    frame: CapturedFrame,
) -> io::Result<WrittenCapture> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    write_capture_on(dir, description, frame, threads)
}

/// [`write_capture`] encoding on at most `threads` threads: a window's
/// writer (#1086) leaves the frame thread its cores, so the game it
/// captures doesn't stutter while the images encode.
pub fn write_capture_on(
    dir: &Path,
    description: &mut FrameDescription,
    frame: CapturedFrame,
    threads: usize,
) -> io::Result<WrittenCapture> {
    write_capture_with(dir, description, frame, threads, &|| {})
}

/// [`write_capture_on`], calling `each_thread` first on every thread that
/// encodes (#1178): the player's writer puts each in the background, so
/// the frame thread keeps the cores and the disk while the images encode.
pub fn write_capture_with(
    dir: &Path,
    description: &mut FrameDescription,
    frame: CapturedFrame,
    threads: usize,
    each_thread: &(dyn Fn() + Sync),
) -> io::Result<WrittenCapture> {
    prepare_dir(dir)?;
    let started = Instant::now();
    // Each image's copy time on the GPU (#1160), on its description
    description.each_pass_mut(&mut |pass| {
        for described in &mut pass.images {
            described.gpu_copy_ms = frame.image(described).and_then(|i| i.gpu_ms);
        }
    });
    let jobs: Vec<(PathBuf, &CapturedImage)> = description
        .images()
        .into_iter()
        .filter_map(|d| Some((dir.join(&d.file), frame.image(d)?)))
        .collect();
    let next = AtomicUsize::new(0);
    let threads = threads.max(1).min(jobs.len().max(1));
    let results: Vec<io::Result<u64>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    each_thread();
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((path, image)) = jobs.get(i) else {
                            break;
                        };
                        out.push((i, encode(path, image)));
                    }
                    out
                })
            })
            .collect();
        let mut all: Vec<(usize, io::Result<u64>)> = workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect();
        all.sort_by_key(|(i, _)| *i);
        all.into_iter().map(|(_, r)| r).collect()
    });
    let mut file_bytes = 0;
    for result in results {
        file_bytes += result?;
    }
    let raw_bytes = frame.images.iter().map(|i| i.bytes.len() as u64).sum();
    description.capture = Some(CaptureDescription {
        images: jobs.len() as u32,
        raw_bytes,
        file_bytes,
        untapped: frame.untapped.clone(),
        record_ms: frame.record_ms,
        gpu_copy_ms: frame.gpu_copy_ms,
        gpu_copies_timed: frame.gpu_copies_timed,
        wait_ms: frame.wait_ms,
        copy_out_ms: frame.copy_out_ms,
        encode_ms: started.elapsed().as_secs_f64() * 1000.0,
    });
    let manifest = dir.join("manifest.json");
    let json = serde_json::to_string_pretty(description).map_err(io::Error::other)?;
    std::fs::write(&manifest, json)?;
    Ok(WrittenCapture {
        manifest,
        images: jobs.len(),
        file_bytes,
        raw_bytes,
    })
}

/// What a capture's folder may hold: what [`write_capture`] writes, and the
/// player's trace (`FrameDescription::trace`).
const CAPTURE_FILES: [&str; 4] = ["manifest.json", "targets", "replay", "trace.json"];

/// Make `dir` ready for a capture: made if missing, an earlier capture's
/// files cleared, anything else refused.
fn prepare_dir(dir: &Path) -> io::Result<()> {
    if dir.exists() {
        let mut foreign = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name();
            // `trace.json` is the player's look-back beside the capture
            // (#1097), written on a thread of its own that can reach the
            // folder before this one does (#1178): kept, never refused
            if !CAPTURE_FILES.iter().any(|f| name == *f) {
                foreign.push(name.to_string_lossy().into_owned());
            }
        }
        if !foreign.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} holds more than a frame capture ({}): name a new folder",
                    dir.display(),
                    foreign.join(", ")
                ),
            ));
        }
        // The manifest goes first: a folder without one is incomplete
        let manifest = dir.join("manifest.json");
        if manifest.exists() {
            std::fs::remove_file(manifest)?;
        }
        let targets = dir.join("targets");
        if targets.exists() {
            std::fs::remove_dir_all(targets)?;
        }
    }
    std::fs::create_dir_all(dir.join("targets"))
}

/// Encode one image to `path`; its size on disk.
fn encode(path: &Path, image: &CapturedImage) -> io::Result<u64> {
    let (w, h) = (image.width, image.height);
    let bytes = &image.bytes;
    match Storage::of(image.format) {
        Storage::Png8 { channels, bgra } => {
            let color = match channels {
                1 => image::ExtendedColorType::L8,
                2 => image::ExtendedColorType::La8,
                _ => image::ExtendedColorType::Rgba8,
            };
            let swizzled;
            let texels: &[u8] = if bgra {
                swizzled = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[2], p[1], p[0], p[3]])
                    .collect::<Vec<u8>>();
                &swizzled
            } else {
                bytes
            };
            // The same encoder `flint render` saves its output with, so a
            // capture's last image is that file byte for byte
            image::save_buffer(path, texels, w, h, color).map_err(io::Error::other)?;
        }
        Storage::Png16 { channels } => {
            // `image` takes 16-bit samples in native order; the GPU's copy
            // is little-endian, as is every target Flint builds for. Two
            // channels (the style mask's Rg16Uint) go as grey and alpha.
            let texels: Vec<u8> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|b| u16::from_le_bytes(*b).to_ne_bytes())
                .collect();
            let color = match channels {
                1 => image::ExtendedColorType::L16,
                2 => image::ExtendedColorType::La16,
                _ => image::ExtendedColorType::Rgba16,
            };
            image::save_buffer(path, &texels, w, h, color).map_err(io::Error::other)?;
        }
        Storage::ExrHalf { channels } => {
            let samples = |c: usize| {
                let n = channels as usize;
                exr::prelude::FlatSamples::F16(
                    bytes
                        .chunks_exact(2 * n)
                        .map(|t| {
                            exr::prelude::f16::from_bits(u16::from_le_bytes([
                                t[2 * c],
                                t[2 * c + 1],
                            ]))
                        })
                        .collect(),
                )
            };
            write_exr(path, w, h, channel_names(channels, false), samples)?;
        }
        Storage::ExrF32 { channels, depth } => {
            let samples = |c: usize| {
                let n = channels as usize;
                exr::prelude::FlatSamples::F32(
                    bytes
                        .chunks_exact(4 * n)
                        .map(|t| {
                            f32::from_le_bytes([t[4 * c], t[4 * c + 1], t[4 * c + 2], t[4 * c + 3]])
                        })
                        .collect(),
                )
            };
            write_exr(path, w, h, channel_names(channels, depth), samples)?;
        }
        Storage::ExrU32 { channels } => {
            let samples = |c: usize| {
                let n = channels as usize;
                exr::prelude::FlatSamples::U32(
                    bytes
                        .chunks_exact(4 * n)
                        .map(|t| {
                            u32::from_le_bytes([t[4 * c], t[4 * c + 1], t[4 * c + 2], t[4 * c + 3]])
                        })
                        .collect(),
                )
            };
            write_exr(path, w, h, channel_names(channels, false), samples)?;
        }
        Storage::Raw => std::fs::write(path, bytes)?,
    }
    Ok(std::fs::metadata(path)?.len())
}

/// A texture's channels as OpenEXR names them: `R`, `G`, `B`, `A`, or `Z`
/// for depth.
fn channel_names(channels: u8, depth: bool) -> &'static [&'static str] {
    if depth {
        return &["Z"];
    }
    match channels {
        1 => &["R"],
        2 => &["R", "G"],
        3 => &["R", "G", "B"],
        _ => &["R", "G", "B", "A"],
    }
}

/// One layer of flat channels, ZIP-compressed scan lines.
fn write_exr(
    path: &Path,
    width: u32,
    height: u32,
    names: &[&str],
    samples: impl Fn(usize) -> exr::prelude::FlatSamples,
) -> io::Result<()> {
    use exr::prelude::*;
    let channels: SmallVec<[AnyChannel<FlatSamples>; 4]> = names
        .iter()
        .enumerate()
        .map(|(c, name)| AnyChannel::new(*name, samples(c)))
        .collect();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let layer = Layer::new(
        (width as usize, height as usize),
        LayerAttributes::named(name.as_str()),
        Encoding::SMALL_LOSSLESS,
        AnyChannels::sort(channels),
    );
    Image::from_layer(layer)
        .write()
        .non_parallel()
        .to_file(path)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_capture::{ImageDescription, PassDescription};

    fn image(tap: usize, format: wgpu::TextureFormat, bytes: Vec<u8>) -> CapturedImage {
        CapturedImage {
            tap,
            target: format!("t{tap}"),
            format,
            width: 2,
            height: 1,
            bytes,
            // Each copy's GPU time, told apart by its tap (#1160)
            gpu_ms: Some(0.5 * (tap + 1) as f64),
        }
    }

    #[test]
    fn a_capture_is_its_images_then_its_manifest() {
        let dir = std::env::temp_dir().join(format!("flint-capture-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let half = |v: f32| exr::prelude::f16::from_f32(v).to_bits().to_le_bytes();
        let hdr: Vec<u8> = [0.5f32, 1.0, 2.0, 1.0, 4.0, 0.0, 0.25, 1.0]
            .iter()
            .flat_map(|v| half(*v))
            .collect();
        let frame = CapturedFrame {
            images: vec![
                image(0, wgpu::TextureFormat::Rgba16Float, hdr),
                image(
                    1,
                    wgpu::TextureFormat::Bgra8UnormSrgb,
                    vec![1, 2, 3, 255, 4, 5, 6, 255],
                ),
                image(
                    2,
                    wgpu::TextureFormat::Depth32Float,
                    [0.25f32, 1.0]
                        .iter()
                        .flat_map(|v| v.to_le_bytes())
                        .collect(),
                ),
            ],
            untapped: vec!["Grass: grass_instances (a buffer, not a texture)".into()],
            gpu_copy_ms: Some(3.0),
            gpu_copies_timed: 3,
            record_ms: 0.1,
            wait_ms: Some(1.0),
            copy_out_ms: 0.2,
        };
        let described = |tap: usize, file: &str, stored: &str| ImageDescription {
            target: format!("t{tap}"),
            file: file.into(),
            stored: stored.into(),
            size: [2, 1],
            tap: Some(tap),
            ..Default::default()
        };
        let mut d = FrameDescription::default();
        d.passes.push(PassDescription {
            name: "Scene".into(),
            ran: true,
            images: vec![
                described(0, "targets/00-hdr.exr", "exr-half"),
                described(2, "targets/00-depth.exr", "exr-f32"),
            ],
            ..Default::default()
        });
        d.passes.push(PassDescription {
            name: "Composite".into(),
            ran: true,
            images: vec![described(1, "targets/01-surface.png", "png")],
            ..Default::default()
        });
        let written = write_capture(&dir, &mut d, frame).unwrap();
        assert_eq!(written.images, 3);
        let png = image::open(dir.join("targets/01-surface.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(
            png.as_raw(),
            &[3, 2, 1, 255, 6, 5, 4, 255],
            "BGRA stored as RGBA"
        );
        use exr::prelude::*;
        let hdr = read_first_flat_layer_from_file(dir.join("targets/00-hdr.exr")).unwrap();
        let r = hdr
            .layer_data
            .channel_data
            .list
            .iter()
            .find(|c| c.name.eq("R"))
            .unwrap();
        match &r.sample_data {
            FlatSamples::F16(v) => assert_eq!([v[0].to_f32(), v[1].to_f32()], [0.5, 4.0]),
            other => panic!("half expected, got {other:?}"),
        }
        let depth = read_first_flat_layer_from_file(dir.join("targets/00-depth.exr")).unwrap();
        let z = &depth.layer_data.channel_data.list[0];
        assert_eq!(z.name, Text::from("Z"));
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["capture"]["images"], 3);
        assert_eq!(
            manifest["passes"][1]["images"][0]["file"],
            "targets/01-surface.png"
        );
        // Each image carries its own copy's GPU time, the capture the sum
        assert_eq!(manifest["passes"][0]["images"][0]["gpu_copy_ms"], 0.5);
        assert_eq!(manifest["passes"][0]["images"][1]["gpu_copy_ms"], 1.5);
        assert_eq!(manifest["passes"][1]["images"][0]["gpu_copy_ms"], 1.0);
        assert_eq!(manifest["capture"]["gpu_copy_ms"], 3.0);
        assert_eq!(manifest["capture"]["gpu_copies_timed"], 3);
        // A second capture into the folder replaces the first
        let again = CapturedFrame {
            images: vec![image(0, wgpu::TextureFormat::R8Unorm, vec![7, 8])],
            untapped: vec![],
            gpu_copy_ms: None,
            gpu_copies_timed: 0,
            record_ms: 0.0,
            wait_ms: None,
            copy_out_ms: 0.0,
        };
        let mut d2 = FrameDescription::default();
        d2.passes.push(PassDescription {
            images: vec![described(0, "targets/00-ssao.png", "png")],
            ..Default::default()
        });
        write_capture(&dir, &mut d2, again).unwrap();
        assert!(!dir.join("targets/00-hdr.exr").exists());
        // and a folder holding anything else is refused
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        let refused = write_capture(
            &dir,
            &mut d2.clone(),
            CapturedFrame {
                images: vec![],
                untapped: vec![],
                gpu_copy_ms: None,
                gpu_copies_timed: 0,
                record_ms: 0.0,
                wait_ms: None,
                copy_out_ms: 0.0,
            },
        );
        assert!(refused.is_err());
        assert!(dir.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #1178: the player's trace writer can reach the folder first; the
    /// capture is written beside its `trace.json`, which is kept.
    #[test]
    fn a_trace_written_first_is_kept_beside_the_capture() {
        let dir = std::env::temp_dir().join(format!("flint-capture-trace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("trace.json"), "{}").unwrap();
        let mut d = FrameDescription {
            trace: Some("trace.json".into()),
            ..Default::default()
        };
        let empty = CapturedFrame {
            images: vec![],
            untapped: vec![],
            gpu_copy_ms: None,
            gpu_copies_timed: 0,
            record_ms: 0.0,
            wait_ms: None,
            copy_out_ms: 0.0,
        };
        write_capture(&dir, &mut d, empty).expect("written beside the trace");
        assert!(dir.join("manifest.json").is_file());
        assert_eq!(
            std::fs::read_to_string(dir.join("trace.json")).unwrap(),
            "{}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
