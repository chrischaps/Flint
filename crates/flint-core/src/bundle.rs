//! Game bundles: a game's shipped files in one fetchable blob (#98).
//!
//! A browser can't list or open files, so the web player fetches a game as
//! one `.flintpack` file and serves the [`vfs`](crate::vfs) from it. The
//! packer is `flint bundle` (flint-cli); this module is the format, read and
//! written in memory with no I/O of its own.
//!
//! ```text
//! offset 0   b"FLINTPAK"                      magic
//!        8   u32 LE   format version (1)
//!       12   u32 LE   manifest length, bytes
//!       16   manifest (UTF-8 TOML, a [`BundleManifest`])
//!       16+m data: every file's bytes, back to back, uncompressed
//! ```
//!
//! Each [`BundleEntry`] gives its file's offset into the data section and its
//! size. The data is left uncompressed: most of a game's bytes are PNG, GLB
//! and Ogg, which are compressed already, and a host can still gzip the whole
//! file on the wire. [`total_len`] reads the length of the whole bundle from
//! its first bytes, so a loader can show progress before the rest arrives.
//!
//! A game ships as a **main** bundle (scenes, schemas, scripts, models,
//! textures) and, optionally, **late** bundles listed in the main manifest
//! (audio, by default) that the player fetches once it is running. Each
//! bundle is complete on its own: [`Bundle::insert_into`] adds its files to a
//! [`MemorySource`], and the web player installs the grown source again.
//!
//! ```
//! use flint_core::bundle::{Bundle, BundleManifest};
//!
//! let manifest = BundleManifest { name: "demo".into(), ..Default::default() };
//! let bytes = Bundle::write(manifest, [("levels/a.scene.toml", b"[scene]".to_vec())]);
//! let bundle = Bundle::parse(bytes).unwrap();
//! assert_eq!(bundle.file("levels/a.scene.toml"), Some(&b"[scene]"[..]));
//! ```

use crate::vfs::{normalize, MemorySource};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

/// The first eight bytes of every bundle.
pub const MAGIC: &[u8; 8] = b"FLINTPAK";
/// The format version this build reads and writes.
pub const FORMAT_VERSION: u32 = 1;
/// Magic, version and manifest length.
pub const HEADER_LEN: usize = 16;
/// The file extension the packer gives bundles.
pub const EXTENSION: &str = "flintpack";

/// What a bundle holds and how to start it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleManifest {
    /// The game's name (its folder, unless `bundle.toml` says otherwise).
    pub name: String,
    /// `"main"` or the late bundle's name (e.g. `"audio"`).
    #[serde(default = "main_part")]
    pub part: String,
    /// The scene to start, relative to the game root (main bundle only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// Schema directories in load order, later overriding earlier, as
    /// `--schemas` takes them (main bundle only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<String>,
    /// Bundles to fetch once the game is running, relative to this one's URL.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub late: Vec<LateBundle>,
    /// Bytes in the data section.
    #[serde(default)]
    pub data_size: u64,
    /// Every file, in data order.
    #[serde(default, rename = "file")]
    pub files: Vec<BundleEntry>,
}

fn main_part() -> String {
    "main".into()
}

impl Default for BundleManifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            part: main_part(),
            scene: None,
            schemas: Vec::new(),
            late: Vec::new(),
            data_size: 0,
            files: Vec::new(),
        }
    }
}

/// A late bundle, as the main manifest lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LateBundle {
    /// Its part name (`"audio"`).
    pub part: String,
    /// Its file name, next to the main bundle.
    pub url: String,
    /// Its whole size in bytes, for progress before its header arrives.
    pub size: u64,
}

/// One file in a bundle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BundleEntry {
    /// Its path from the game root, [`normalize`]d.
    pub path: String,
    /// Where its bytes start in the data section.
    pub offset: u64,
    pub size: u64,
}

/// Why bytes are not a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// Fewer bytes than the header or manifest says there are.
    Truncated { have: usize, need: usize },
    /// Doesn't start with [`MAGIC`].
    NotABundle,
    /// A format this build doesn't read.
    Version(u32),
    /// The manifest isn't UTF-8 TOML of the expected shape.
    Manifest(String),
    /// A file entry points outside the data section.
    BadEntry(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { have, need } => {
                write!(f, "bundle is truncated: {have} of {need} bytes")
            }
            Self::NotABundle => write!(f, "not a Flint bundle (no FLINTPAK header)"),
            Self::Version(v) => write!(
                f,
                "bundle format {v}; this build reads format {FORMAT_VERSION}"
            ),
            Self::Manifest(e) => write!(f, "bundle manifest: {e}"),
            Self::BadEntry(path) => write!(f, "bundle entry {path} runs past the data"),
        }
    }
}

impl std::error::Error for BundleError {}

/// The manifest's length and where the data starts, from the header.
fn read_header(bytes: &[u8]) -> Result<usize, BundleError> {
    if bytes.len() < HEADER_LEN {
        // Check what there is of the magic, so a short HTML error page is
        // reported as "not a bundle", not "truncated".
        let n = bytes.len().min(MAGIC.len());
        if bytes[..n] != MAGIC[..n] {
            return Err(BundleError::NotABundle);
        }
        return Err(BundleError::Truncated {
            have: bytes.len(),
            need: HEADER_LEN,
        });
    }
    if &bytes[..8] != MAGIC {
        return Err(BundleError::NotABundle);
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(BundleError::Version(version));
    }
    Ok(u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize)
}

/// The length of the whole bundle, once its header and manifest have
/// arrived: `Ok(None)` until then, an error if `prefix` can't be a bundle.
pub fn total_len(prefix: &[u8]) -> Result<Option<usize>, BundleError> {
    let manifest_len = match read_header(prefix) {
        Ok(len) => len,
        Err(BundleError::Truncated { .. }) => return Ok(None),
        Err(e) => return Err(e),
    };
    let end = HEADER_LEN + manifest_len;
    if prefix.len() < end {
        return Ok(None);
    }
    let manifest = parse_manifest(&prefix[HEADER_LEN..end])?;
    Ok(Some(end + manifest.data_size as usize))
}

fn parse_manifest(bytes: &[u8]) -> Result<BundleManifest, BundleError> {
    let text = std::str::from_utf8(bytes).map_err(|e| BundleError::Manifest(e.to_string()))?;
    toml::from_str(text).map_err(|e| BundleError::Manifest(e.to_string()))
}

/// A parsed bundle: its manifest and the bytes the entries point into.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub manifest: BundleManifest,
    bytes: Vec<u8>,
    data_start: usize,
}

impl Bundle {
    /// Lay `files` out after a header and `manifest`, whose `files` and
    /// `data_size` are filled in here. Paths are [`normalize`]d; a later
    /// file with the same path replaces an earlier one.
    pub fn write<P, I>(mut manifest: BundleManifest, files: I) -> Vec<u8>
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = (P, Vec<u8>)>,
    {
        let mut ordered: std::collections::BTreeMap<String, Vec<u8>> = Default::default();
        for (path, bytes) in files {
            ordered.insert(normalize(path.as_ref()), bytes);
        }
        manifest.files.clear();
        let mut offset = 0u64;
        for (path, bytes) in &ordered {
            let size = bytes.len() as u64;
            manifest.files.push(BundleEntry {
                path: path.clone(),
                offset,
                size,
            });
            offset += size;
        }
        manifest.data_size = offset;

        let text = toml::to_string(&manifest).expect("a manifest always serializes");
        let mut out = Vec::with_capacity(HEADER_LEN + text.len() + offset as usize);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        for bytes in ordered.into_values() {
            out.extend_from_slice(&bytes);
        }
        out
    }

    /// Check the header, read the manifest and make sure every entry lies
    /// inside the data. Takes the bytes as fetched; no copy.
    pub fn parse(bytes: Vec<u8>) -> Result<Self, BundleError> {
        let manifest_len = read_header(&bytes)?;
        let data_start = HEADER_LEN + manifest_len;
        if bytes.len() < data_start {
            return Err(BundleError::Truncated {
                have: bytes.len(),
                need: data_start,
            });
        }
        let manifest = parse_manifest(&bytes[HEADER_LEN..data_start])?;
        let need = data_start + manifest.data_size as usize;
        if bytes.len() < need {
            return Err(BundleError::Truncated {
                have: bytes.len(),
                need,
            });
        }
        for entry in &manifest.files {
            if entry.offset + entry.size > manifest.data_size {
                return Err(BundleError::BadEntry(entry.path.clone()));
            }
        }
        Ok(Self {
            manifest,
            bytes,
            data_start,
        })
    }

    /// One file's bytes, by any spelling of its path.
    pub fn file(&self, path: impl AsRef<Path>) -> Option<&[u8]> {
        let key = normalize(path.as_ref());
        self.manifest
            .files
            .iter()
            .find(|e| e.path == key)
            .map(|e| self.slice(e))
    }

    /// Every file, as `(path, bytes)`, in data order.
    pub fn files(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.manifest
            .files
            .iter()
            .map(|e| (e.path.as_str(), self.slice(e)))
    }

    fn slice(&self, entry: &BundleEntry) -> &[u8] {
        let start = self.data_start + entry.offset as usize;
        &self.bytes[start..start + entry.size as usize]
    }

    /// Add every file to `source` (replacing any already at its path).
    pub fn insert_into(&self, source: &mut MemorySource) {
        for (path, bytes) in self.files() {
            source.insert(path, bytes);
        }
    }

    /// A memory source holding this bundle's files.
    pub fn to_memory_source(&self) -> MemorySource {
        let mut source = MemorySource::new();
        self.insert_into(&mut source);
        source
    }

    /// The whole bundle's size in bytes.
    pub fn len(&self) -> usize {
        self.data_start + self.manifest.data_size as usize
    }

    pub fn is_empty(&self) -> bool {
        self.manifest.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs;
    use std::sync::Arc;

    fn sample() -> Vec<u8> {
        let manifest = BundleManifest {
            name: "float".into(),
            scene: Some("scenes/float.scene.toml".into()),
            schemas: vec!["engine/schemas".into(), "schemas".into()],
            late: vec![LateBundle {
                part: "audio".into(),
                url: "float.audio.flintpack".into(),
                size: 7,
            }],
            ..Default::default()
        };
        Bundle::write(
            manifest,
            [
                (
                    "scenes\\float.scene.toml",
                    b"[scene]\nname = \"Float\"\n".to_vec(),
                ),
                ("./scripts/raft.rhai", b"fn on_init() {}".to_vec()),
                ("models/empty.glb", Vec::new()),
            ],
        )
    }

    #[test]
    fn round_trips_files_and_manifest() {
        let bytes = sample();
        let len = bytes.len();
        let bundle = Bundle::parse(bytes).unwrap();
        assert_eq!(bundle.len(), len);
        assert_eq!(bundle.manifest.part, "main");
        assert_eq!(
            bundle.manifest.scene.as_deref(),
            Some("scenes/float.scene.toml")
        );
        assert_eq!(bundle.manifest.late[0].url, "float.audio.flintpack");
        let paths: Vec<&str> = bundle.files().map(|(p, _)| p).collect();
        assert_eq!(
            paths,
            [
                "models/empty.glb",
                "scenes/float.scene.toml",
                "scripts/raft.rhai"
            ],
            "paths are normalized and sorted"
        );
        assert_eq!(
            bundle.file("scripts\\raft.rhai"),
            Some(&b"fn on_init() {}"[..])
        );
        assert_eq!(bundle.file("models/empty.glb"), Some(&[][..]));
        assert_eq!(bundle.file("nope"), None);
    }

    #[test]
    fn total_len_waits_for_the_manifest() {
        let bytes = sample();
        assert_eq!(total_len(&bytes[..4]), Ok(None));
        assert_eq!(total_len(&bytes[..HEADER_LEN + 3]), Ok(None));
        assert_eq!(total_len(&bytes), Ok(Some(bytes.len())));
        let manifest_end =
            HEADER_LEN + u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        assert_eq!(total_len(&bytes[..manifest_end]), Ok(Some(bytes.len())));
    }

    #[test]
    fn rejects_what_is_not_a_bundle() {
        assert_eq!(
            Bundle::parse(b"<!doctype html>".to_vec()).unwrap_err(),
            BundleError::NotABundle
        );
        assert_eq!(total_len(b"<html>"), Err(BundleError::NotABundle));
        let mut wrong = sample();
        wrong[8] = 9;
        assert_eq!(Bundle::parse(wrong).unwrap_err(), BundleError::Version(9));
        let mut short = sample();
        short.pop();
        assert!(matches!(
            Bundle::parse(short).unwrap_err(),
            BundleError::Truncated { .. }
        ));
    }

    #[test]
    fn serves_the_vfs_and_late_bundles_add_to_it() {
        let main = Bundle::parse(sample()).unwrap();
        let late = Bundle::parse(Bundle::write(
            BundleManifest {
                name: "float".into(),
                part: "audio".into(),
                ..Default::default()
            },
            [("audio/ocean_loop.ogg", b"OggS".to_vec())],
        ))
        .unwrap();

        let mut source = main.to_memory_source();
        vfs::scoped(Arc::new(source.clone()), || {
            assert!(vfs::is_file("scenes/float.scene.toml"));
            assert!(!vfs::exists("audio/ocean_loop.ogg"));
        });
        late.insert_into(&mut source);
        vfs::scoped(Arc::new(source), || {
            assert!(vfs::is_file("scripts/raft.rhai"));
            assert_eq!(vfs::read("audio/ocean_loop.ogg").unwrap(), b"OggS");
        });
    }
}
