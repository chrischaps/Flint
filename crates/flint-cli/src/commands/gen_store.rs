//! `flint gen --register`: put generated files in the content store, record
//! where they came from, and find them again.
//!
//! Each registered run leaves three things behind:
//!
//! - the bytes, in the content store at `.flint/assets/<2 hex>/<hash>.<ext>`;
//! - a sidecar beside each output file, `<file>.asset.toml`, carrying the
//!   provenance (generator, spec hash, seed, timestamp, Flint version);
//! - a provenance entry at `.flint/procgen/<spec hash>/<seed>.toml` listing
//!   the hashes that spec + seed produced.
//!
//! The entry is what makes the cache a property of the store rather than of
//! whatever happens to sit at the output path: a later run with the same spec
//! and seed finds it, checks the files on disk against its hashes, and copies
//! back from the store any that were deleted or changed, without generating.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flint_asset::{AssetFile, AssetMeta, AssetType, ContentStore};
use flint_core::ContentHash;
use flint_procgen::{OutputKind, ProcGenSpec};

/// Content store root, shared with `flint asset` and the AI asset pipeline.
pub const STORE_ROOT: &str = ".flint/assets";
/// Provenance entries: which hashes a spec + seed produced.
pub const INDEX_ROOT: &str = ".flint/procgen";

/// One file a run wrote. `role` names an image-set map (`color`, `normal`,
/// ...) and is `None` for a run that writes a single file.
#[derive(Debug, Clone, PartialEq)]
pub struct WrittenFile {
    pub role: Option<String>,
    pub path: PathBuf,
}

/// What a spec + seed produced, as recorded when it was registered.
#[derive(Debug, Clone, PartialEq)]
pub struct Provenance {
    pub generator: String,
    pub spec_hash: String,
    pub seed: u64,
    pub timestamp: String,
    pub flint_version: String,
    /// (role, content hash) for each file the run wrote.
    pub files: Vec<(Option<String>, String)>,
}

/// What a cache lookup could do without generating.
#[derive(Debug, PartialEq)]
pub enum Reuse {
    /// Every file is on disk with the recorded bytes.
    Present,
    /// Some files were missing or had changed; this many were copied back
    /// from the store.
    Restored(usize),
}

pub struct Store {
    content: ContentStore,
    index_root: PathBuf,
}

impl Store {
    pub fn open_default() -> Self {
        Self::open(Path::new(STORE_ROOT), Path::new(INDEX_ROOT))
    }

    pub fn open(store_root: &Path, index_root: &Path) -> Self {
        Self {
            content: ContentStore::new(store_root),
            index_root: index_root.to_path_buf(),
        }
    }

    fn entry_path(&self, spec_hash: &ContentHash, seed: u64) -> PathBuf {
        self.index_root
            .join(spec_hash.to_hex())
            .join(format!("{seed}.toml"))
    }

    /// The provenance recorded for this spec + seed, if it was registered.
    /// An unreadable entry is reported and treated as absent: the cost of a
    /// miss is one regeneration, which rewrites it.
    pub fn lookup(&self, spec_hash: &ContentHash, seed: u64) -> Option<Provenance> {
        let path = self.entry_path(spec_hash, seed);
        let text = std::fs::read_to_string(&path).ok()?;
        match parse_entry(&text) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("warning: ignoring {}: {e:#}", path.display());
                None
            }
        }
    }

    /// Bring the files at `targets` in line with `prov` from the store alone.
    /// Returns `None`, touching nothing, if the store no longer holds every
    /// file the entry lists (or the entry and targets disagree on the maps).
    pub fn reuse(&self, prov: &Provenance, targets: &[WrittenFile]) -> Result<Option<Reuse>> {
        let mut plan = Vec::new();
        for (role, hash) in &prov.files {
            let Some(target) = targets.iter().find(|t| &t.role == role) else {
                return Ok(None);
            };
            let Some(hash) = ContentHash::from_prefixed_hex(hash) else {
                return Ok(None);
            };
            let intact = target.path.is_file()
                && ContentHash::from_file(&target.path)
                    .map(|h| h == hash)
                    .unwrap_or(false);
            if intact {
                continue;
            }
            match self.content.get(&hash) {
                Some(blob) => plan.push((blob, target.path.clone())),
                None => return Ok(None),
            }
        }

        for (blob, dest) in &plan {
            if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            std::fs::copy(blob, dest).with_context(|| {
                format!(
                    "failed to restore {} from {}",
                    dest.display(),
                    blob.display()
                )
            })?;
        }
        Ok(Some(if plan.is_empty() {
            Reuse::Present
        } else {
            Reuse::Restored(plan.len())
        }))
    }

    /// Store each written file, write its sidecar, and record the entry.
    pub fn register(
        &self,
        spec: &ProcGenSpec,
        spec_hash: &ContentHash,
        seed: u64,
        kind: OutputKind,
        files: &[WrittenFile],
    ) -> Result<Provenance> {
        let mut recorded = Vec::new();
        for f in files {
            let hash = self
                .content
                .store(&f.path)
                .map_err(|e| anyhow::anyhow!("{e}"))
                .with_context(|| format!("failed to store {} in {STORE_ROOT}", f.path.display()))?;
            recorded.push((f.role.clone(), hash.to_prefixed_hex()));
        }

        let prov = Provenance {
            generator: spec.generator.clone(),
            spec_hash: spec_hash.to_prefixed_hex(),
            seed,
            timestamp: flint_asset_gen::time::now_iso8601(),
            flint_version: env!("CARGO_PKG_VERSION").to_string(),
            files: recorded,
        };
        self.write_sidecars(spec, kind, &prov, files)?;

        let entry = self.entry_path(spec_hash, seed);
        if let Some(parent) = entry.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&entry, render_entry(&prov))
            .with_context(|| format!("failed to write {}", entry.display()))?;
        Ok(prov)
    }

    /// Write (or rewrite) the sidecar beside each file from `prov`. Rewriting
    /// from the recorded provenance keeps the original timestamp, so a cache
    /// hit leaves the sidecar as registration wrote it.
    pub fn write_sidecars(
        &self,
        spec: &ProcGenSpec,
        kind: OutputKind,
        prov: &Provenance,
        files: &[WrittenFile],
    ) -> Result<()> {
        for f in files {
            let Some((_, hash)) = prov.files.iter().find(|(r, _)| r == &f.role) else {
                continue;
            };
            let sidecar = sidecar_path_for(&f.path);
            let text = toml::to_string_pretty(&sidecar_for(spec, kind, prov, f, hash))
                .context("failed to serialize asset metadata")?;
            std::fs::write(&sidecar, text)
                .with_context(|| format!("failed to write sidecar {}", sidecar.display()))?;
        }
        Ok(())
    }
}

/// The `.asset.toml` sidecar path for an output file: `tree_42.glb.asset.toml`.
pub fn sidecar_path_for(out_path: &Path) -> PathBuf {
    let mut name = out_path.file_name().unwrap_or_default().to_os_string();
    name.push(".asset.toml");
    out_path.with_file_name(name)
}

/// A file name piece made from a spec name: spaces to `_`, the rest dropped.
pub fn slug(name: &str) -> String {
    name.replace(' ', "_")
        .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "")
}

fn asset_type_for(kind: OutputKind) -> AssetType {
    match kind {
        OutputKind::Mesh => AssetType::Mesh,
        OutputKind::Image => AssetType::Texture,
        OutputKind::Sound => AssetType::Audio,
    }
}

/// A seed as TOML holds it. TOML integers are i64, and a random seed can use
/// all 64 bits, so the top half is written as a string rather than wrapped
/// to a negative number that `--seed` wouldn't reproduce.
fn seed_value(seed: u64) -> toml::Value {
    match i64::try_from(seed) {
        Ok(i) => toml::Value::Integer(i),
        Err(_) => toml::Value::String(seed.to_string()),
    }
}

fn parse_seed(v: &toml::Value) -> Option<u64> {
    match v {
        toml::Value::Integer(i) => u64::try_from(*i).ok(),
        toml::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn sidecar_for(
    spec: &ProcGenSpec,
    kind: OutputKind,
    prov: &Provenance,
    file: &WrittenFile,
    hash: &str,
) -> AssetFile {
    let mut name = format!("{}_{}", slug(&spec.meta.name), prov.seed);
    let mut properties = HashMap::new();
    let mut put = |k: &str, v: toml::Value| {
        properties.insert(k.to_string(), v);
    };
    put("generator", toml::Value::String(prov.generator.clone()));
    put("spec_hash", toml::Value::String(prov.spec_hash.clone()));
    put("seed", seed_value(prov.seed));
    put("timestamp", toml::Value::String(prov.timestamp.clone()));
    put(
        "flint_version",
        toml::Value::String(prov.flint_version.clone()),
    );
    if let Some(role) = &file.role {
        put("map", toml::Value::String(role.clone()));
        name.push('_');
        name.push_str(role);
    }

    AssetFile {
        asset: AssetMeta {
            name,
            asset_type: asset_type_for(kind),
            hash: hash.to_string(),
            // Forward slashes, so a sidecar written on Windows reads the
            // same everywhere.
            source_path: Some(file.path.to_string_lossy().replace('\\', "/")),
            format: file
                .path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_string),
            tags: spec.meta.tags.clone(),
            properties,
        },
    }
}

fn render_entry(p: &Provenance) -> String {
    let mut t = toml::Table::new();
    t.insert("generator".into(), p.generator.clone().into());
    t.insert("spec_hash".into(), p.spec_hash.clone().into());
    t.insert("seed".into(), seed_value(p.seed));
    t.insert("timestamp".into(), p.timestamp.clone().into());
    t.insert("flint_version".into(), p.flint_version.clone().into());
    let files = p
        .files
        .iter()
        .map(|(role, hash)| {
            let mut f = toml::Table::new();
            if let Some(role) = role {
                f.insert("role".into(), role.clone().into());
            }
            f.insert("hash".into(), hash.clone().into());
            toml::Value::Table(f)
        })
        .collect::<Vec<_>>();
    t.insert("files".into(), toml::Value::Array(files));
    format!(
        "# flint gen --register: what this spec + seed produced\n{}",
        toml::to_string_pretty(&t).unwrap_or_default()
    )
}

fn parse_entry(text: &str) -> Result<Provenance> {
    let t: toml::Table = toml::from_str(text)?;
    let str_of = |k: &str| -> Result<String> {
        t.get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .with_context(|| format!("missing '{k}'"))
    };
    let seed = t
        .get("seed")
        .and_then(parse_seed)
        .context("missing or invalid 'seed'")?;
    let files = t
        .get("files")
        .and_then(|v| v.as_array())
        .context("missing 'files'")?
        .iter()
        .map(|f| {
            let hash = f
                .get("hash")
                .and_then(|v| v.as_str())
                .context("a file has no 'hash'")?;
            let role = f.get("role").and_then(|v| v.as_str()).map(str::to_string);
            Ok((role, hash.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Provenance {
        generator: str_of("generator")?,
        spec_hash: str_of("spec_hash")?,
        seed,
        timestamp: str_of("timestamp")?,
        flint_version: str_of("flint_version")?,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_appends_to_the_whole_name() {
        assert_eq!(
            sidecar_path_for(Path::new("/tmp/tree_42.glb")),
            PathBuf::from("/tmp/tree_42.glb.asset.toml")
        );
        assert_eq!(
            sidecar_path_for(Path::new("wall.png")),
            PathBuf::from("wall.png.asset.toml")
        );
    }

    #[test]
    fn an_entry_round_trips_including_a_full_width_seed() {
        for seed in [0, 42, u64::MAX] {
            let p = Provenance {
                generator: "texture_v1".into(),
                spec_hash: "sha256:ab".into(),
                seed,
                timestamp: "2026-10-31T21:05:37Z".into(),
                flint_version: "0.1.0".into(),
                files: vec![
                    (Some("color".into()), "sha256:01".into()),
                    (Some("normal".into()), "sha256:02".into()),
                ],
            };
            assert_eq!(parse_entry(&render_entry(&p)).unwrap(), p);
        }
    }

    #[test]
    fn big_seeds_are_written_as_strings_not_wrapped() {
        assert_eq!(seed_value(7), toml::Value::Integer(7));
        assert_eq!(
            seed_value(u64::MAX),
            toml::Value::String("18446744073709551615".into())
        );
        assert_eq!(parse_seed(&toml::Value::Integer(-1)), None);
    }
}
