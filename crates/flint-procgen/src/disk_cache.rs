//! The procgen cache's disk tier (#181): outputs kept between runs.
//!
//! The in-memory [`ProcGenCache`](crate::ProcGenCache) only helps on a
//! revisit within one run; without this tier every launch of the player
//! regenerates every procgen texture and mesh (about 3 s of a 4 s start on
//! `showcase_game/levels/tavern_hub.scene.toml`). Underneath it, each output
//! is kept as one file in a directory, usually `<project>/.flint/procgen-cache/`.
//!
//! - **The key** is whatever the memory cache is keyed on, the spec's hash
//!   (which the resolver folds the generator's name and
//!   [`revision`](crate::Generator::revision) into) and the seed, plus this
//!   file format's own version. Bump a generator's revision and the next
//!   launch misses on every spec it builds, then regenerates.
//! - **`Seed::Random` never reaches the disk.** A random spec changes on each
//!   run, which is the point of it.
//! - **Reads go through [`flint_core::vfs`]**, writes through `std::fs`: a file
//!   is written beside its final name and renamed into place, so a crash or a
//!   second player mid-write never leaves a half file under a real name.
//! - **Size cap, LRU on mtime.** A hit touches the file's modification time;
//!   a write that takes the directory over its budget removes the files least
//!   recently touched until it fits.
//! - A file that doesn't decode (a different format, a truncated write, a
//!   type whose layout changed) is a miss, and is removed.
//!
//! Native only. The web player has no disk to keep it on, so there
//! [`DiskCache::open`] returns `None` and the tier is never there.

use std::path::{Path, PathBuf};

use flint_core::ContentHash;

use crate::output::GeneratorOutput;
use crate::Seed;

/// Where the disk tier lives and how much it may keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskCacheConfig {
    /// The directory the output files go in. Made on the first write.
    pub dir: PathBuf,
    /// The most the directory may hold, in bytes (default 1 GiB).
    pub max_bytes: u64,
}

impl DiskCacheConfig {
    /// The default budget: 1 GiB. tavern_hub's whole set is about 50 MB.
    pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024;

    /// A disk tier in `dir` with the default budget.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_bytes: Self::DEFAULT_MAX_BYTES,
        }
    }

    /// The conventional place in a project: `<project>/.flint/procgen-cache`.
    pub fn in_project(project_dir: &Path) -> Self {
        Self::new(project_dir.join(".flint").join("procgen-cache"))
    }
}

/// What the disk tier has done since it was opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskStats {
    /// Outputs read back from disk.
    pub hits: u64,
    /// Lookups that found no usable file.
    pub misses: u64,
    /// Outputs written.
    pub writes: u64,
    /// Files removed to stay under budget.
    pub evictions: u64,
}

/// File header: what the bytes are, then which layout of them.
const MAGIC: &[u8; 4] = b"FPGC";
/// Bump when the encoding (or anything in `GeneratorOutput`'s types) changes
/// shape: every file written under another version is then a miss.
pub const DISK_FORMAT: u32 = 1;
const HEADER_LEN: usize = 8;
const EXTENSION: &str = "fpgc";

/// The name a key's file goes under: one hash over the format, spec hash
/// and seed, so the name says nothing a reader could misread as meaning.
fn file_name(spec_hash: ContentHash, seed: &Seed) -> String {
    let key = format!("{DISK_FORMAT}|{}|{seed:?}", spec_hash.to_hex());
    let hex = ContentHash::from_str(&key).to_hex();
    format!("{}.{EXTENSION}", &hex[..32])
}

/// The procgen cache's disk tier. See the [module docs](self).
pub struct DiskCache {
    config: DiskCacheConfig,
    stats: DiskStats,
}

impl DiskCache {
    /// Open a disk tier with `config`. The directory need not exist yet.
    /// `None` where there is no disk (the web).
    pub fn open(config: DiskCacheConfig) -> Option<Self> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        Some(Self {
            config,
            stats: DiskStats::default(),
        })
    }

    /// The configuration this tier was opened with.
    pub fn config(&self) -> &DiskCacheConfig {
        &self.config
    }

    /// What this tier has done since it was opened.
    pub fn stats(&self) -> DiskStats {
        self.stats
    }

    /// The path a key's output is kept at.
    pub fn path_for(&self, spec_hash: ContentHash, seed: &Seed) -> PathBuf {
        self.config.dir.join(file_name(spec_hash, seed))
    }

    /// Read a kept output back. A `Seed::Random` key always misses.
    pub fn get(&mut self, spec_hash: ContentHash, seed: &Seed) -> Option<GeneratorOutput> {
        if *seed == Seed::Random {
            return None;
        }
        let path = self.path_for(spec_hash, seed);
        let Ok(bytes) = flint_core::vfs::read(&path) else {
            self.stats.misses += 1;
            return None;
        };
        match decode(&bytes) {
            Some(output) => {
                imp::touch(&path);
                self.stats.hits += 1;
                Some(output)
            }
            None => {
                // Another format or a torn file: useless to anyone.
                imp::remove(&path);
                self.stats.misses += 1;
                None
            }
        }
    }

    /// Keep an output, then trim the directory to its budget. A
    /// `Seed::Random` key, or an output larger than the whole budget, is
    /// not kept. A failed write is reported once and otherwise ignored: the
    /// disk tier is a saving, never a reason to fail a load.
    pub fn put(&mut self, spec_hash: ContentHash, seed: &Seed, output: &GeneratorOutput) {
        if *seed == Seed::Random {
            return;
        }
        let Some(bytes) = encode(output) else {
            return;
        };
        if bytes.len() as u64 > self.config.max_bytes {
            return;
        }
        let path = self.path_for(spec_hash, seed);
        match imp::write_atomic(&self.config.dir, &path, &bytes) {
            Ok(()) => {
                self.stats.writes += 1;
                self.stats.evictions += imp::trim(&self.config.dir, self.config.max_bytes, &path);
            }
            Err(e) => {
                eprintln!(
                    "[procgen] could not keep {} in the disk cache: {e}",
                    path.display()
                );
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn encode(output: &GeneratorOutput) -> Option<Vec<u8>> {
    let size = bincode::serialized_size(output).ok()? as usize;
    let mut bytes = Vec::with_capacity(HEADER_LEN + size);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&DISK_FORMAT.to_le_bytes());
    bincode::serialize_into(&mut bytes, output).ok()?;
    Some(bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn decode(bytes: &[u8]) -> Option<GeneratorOutput> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        return None;
    }
    let format = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if format != DISK_FORMAT {
        return None;
    }
    bincode::deserialize(&bytes[HEADER_LEN..]).ok()
}

#[cfg(target_arch = "wasm32")]
fn encode(_: &GeneratorOutput) -> Option<Vec<u8>> {
    None
}

#[cfg(target_arch = "wasm32")]
fn decode(_: &[u8]) -> Option<GeneratorOutput> {
    None
}

/// The disk writes, and the metadata the LRU reads. `std::fs` on purpose:
/// writes stay off the vfs (CLAUDE.md), and so does mtime polling.
#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::fs;
    use std::io;
    use std::path::Path;

    use super::EXTENSION;

    /// Mark a file most recently used.
    pub fn touch(path: &Path) {
        if let Ok(file) = fs::File::options().write(true).open(path) {
            let _ = file.set_modified(web_time::SystemTime::now());
        }
    }

    pub fn remove(path: &Path) {
        let _ = fs::remove_file(path);
    }

    /// Write beside the final name, then rename into place.
    pub fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = path.with_extension(format!("{EXTENSION}.{}.tmp", std::process::id()));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }

    /// Remove the least recently touched files until `dir` holds at most
    /// `max_bytes`, never `keep` (the file just written). Returns how many
    /// went.
    pub fn trim(dir: &Path, max_bytes: u64, keep: &Path) -> u64 {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        let mut files: Vec<(web_time::SystemTime, u64, std::path::PathBuf)> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let mtime = meta.modified().ok()?;
                Some((mtime, meta.len(), e.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        if total <= max_bytes {
            return 0;
        }
        // Oldest first; ties broken by name so the order is the same each run.
        files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
        let mut removed = 0;
        for (_, len, path) in files {
            if total <= max_bytes {
                break;
            }
            if path == keep {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                removed += 1;
            }
        }
        removed
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::io;
    use std::path::Path;

    pub fn touch(_: &Path) {}
    pub fn remove(_: &Path) {}
    pub fn write_atomic(_: &Path, _: &Path, _: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no disk on the web",
        ))
    }
    pub fn trim(_: &Path, _: u64, _: &Path) -> u64 {
        0
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::types::{ChannelSemantics, ImageData};
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flint_disk_cache_{name}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn pixels(size: u32, fill: u8) -> ImageData {
        let mut img = ImageData::new(size, size, ChannelSemantics::Color);
        img.pixels.iter_mut().for_each(|p| *p = fill);
        img
    }

    fn image(size: u32, fill: u8) -> GeneratorOutput {
        GeneratorOutput::Image(pixels(size, fill))
    }

    fn hash(n: u8) -> ContentHash {
        ContentHash::from_bytes(&[n])
    }

    #[test]
    fn a_kept_output_comes_back_equal() {
        let dir = temp_dir("roundtrip");
        let mut disk = DiskCache::open(DiskCacheConfig::new(&dir)).unwrap();
        let out = GeneratorOutput::ImageSet(vec![pixels(8, 3), pixels(4, 200)]);
        assert!(disk.get(hash(1), &Seed::Fixed(7)).is_none());
        disk.put(hash(1), &Seed::Fixed(7), &out);
        assert_eq!(disk.get(hash(1), &Seed::Fixed(7)), Some(out));
        // Another seed or hash is another file.
        assert!(disk.get(hash(1), &Seed::Fixed(8)).is_none());
        assert!(disk.get(hash(2), &Seed::Fixed(7)).is_none());
        assert_eq!(
            disk.stats(),
            DiskStats {
                hits: 1,
                misses: 3,
                writes: 1,
                evictions: 0
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_kind_of_output_survives() {
        let dir = temp_dir("kinds");
        let mut disk = DiskCache::open(DiskCacheConfig::new(&dir)).unwrap();
        let mut registry = crate::GeneratorRegistry::new();
        crate::register_built_in_generators(&mut registry);
        registry.register(Box::new(crate::mock::MockGenerator));
        let mesh = registry
            .generate_from_spec(
                &crate::ProcGenSpec::parse(
                    "generator = \"mock\"\n[meta]\nname = \"m\"\nversion = \"1\"\n[seed]\nmode = \"fixed\"\nvalue = 1\n",
                )
                .unwrap(),
            )
            .unwrap();
        let outputs = [
            mesh.clone(),
            GeneratorOutput::MeshWithLods(vec![mesh.into_mesh().expect("the mock makes a mesh")]),
            image(2, 9),
            GeneratorOutput::Sound(vec![1, 2, 3]),
        ];
        for (n, out) in outputs.iter().enumerate() {
            disk.put(hash(n as u8), &Seed::Derived("k".into()), out);
            assert_eq!(
                disk.get(hash(n as u8), &Seed::Derived("k".into())).as_ref(),
                Some(out)
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn random_seeds_stay_off_the_disk() {
        let dir = temp_dir("random");
        let mut disk = DiskCache::open(DiskCacheConfig::new(&dir)).unwrap();
        disk.put(hash(1), &Seed::Random, &image(2, 1));
        assert!(disk.get(hash(1), &Seed::Random).is_none());
        assert!(!dir.exists(), "nothing was written");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_of_another_format_is_a_miss_and_goes() {
        let dir = temp_dir("format");
        let mut disk = DiskCache::open(DiskCacheConfig::new(&dir)).unwrap();
        disk.put(hash(1), &Seed::Fixed(1), &image(2, 1));
        let path = disk.path_for(hash(1), &Seed::Fixed(1));
        let mut bytes = fs::read(&path).unwrap();
        bytes[4] = bytes[4].wrapping_add(1); // another DISK_FORMAT
        fs::write(&path, &bytes).unwrap();
        assert!(disk.get(hash(1), &Seed::Fixed(1)).is_none());
        assert!(!path.exists());

        // A torn write, likewise.
        disk.put(hash(2), &Seed::Fixed(1), &image(8, 1));
        let path = disk.path_for(hash(2), &Seed::Fixed(1));
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(disk.get(hash(2), &Seed::Fixed(1)).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_budget_removes_the_least_recently_touched() {
        let dir = temp_dir("budget");
        // 16x16 RGBA = 1024 pixel bytes, about 1050 on disk: room for three.
        let mut disk = DiskCache::open(DiskCacheConfig {
            dir: dir.clone(),
            max_bytes: 3300,
        })
        .unwrap();
        let s = Seed::Fixed(1);
        let t0 = web_time::SystemTime::now();
        for n in 1..=3u8 {
            disk.put(hash(n), &s, &image(16, n));
            // mtimes a minute apart, oldest first: #1, #2, #3.
            let f = fs::File::options()
                .write(true)
                .open(disk.path_for(hash(n), &s))
                .unwrap();
            f.set_modified(t0 - std::time::Duration::from_secs(600 - 60 * n as u64))
                .unwrap();
        }
        // Using #1 makes it the newest, so #2 is the one to go.
        assert!(disk.get(hash(1), &s).is_some());
        disk.put(hash(4), &s, &image(16, 4));
        assert!(disk.path_for(hash(1), &s).exists());
        assert!(!disk.path_for(hash(2), &s).exists());
        assert!(disk.path_for(hash(3), &s).exists());
        assert!(disk.path_for(hash(4), &s).exists());
        assert_eq!(disk.stats().evictions, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_output_over_the_whole_budget_is_not_kept() {
        let dir = temp_dir("oversize");
        let mut disk = DiskCache::open(DiskCacheConfig {
            dir: dir.clone(),
            max_bytes: 100,
        })
        .unwrap();
        disk.put(hash(1), &Seed::Fixed(1), &image(16, 1));
        assert!(disk.get(hash(1), &Seed::Fixed(1)).is_none());
        assert_eq!(disk.stats().writes, 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
