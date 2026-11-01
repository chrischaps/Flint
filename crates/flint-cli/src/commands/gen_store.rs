//! `flint gen --register`: put generated files in the content store, record
//! where they came from, and find them again.
//!
//! Each registered run leaves three things behind:
//!
//! - the bytes, in the content store at `.flint/assets/<2 hex>/<hash>.<ext>`;
//! - a sidecar beside each output file, `<file>.asset.toml`, carrying the
//!   provenance (generator and its revision, spec hash, seed, timestamp,
//!   Flint version);
//! - a provenance entry at `.flint/procgen/<spec hash>/<seed>.toml` listing
//!   the hashes that spec + seed produced.
//!
//! The entry is what makes the cache a property of the store rather than of
//! whatever happens to sit at the output path: a later run with the same spec
//! and seed finds it, checks the files on disk against its hashes, and copies
//! back from the store any that were deleted or changed, without generating.
//!
//! The spec hash covers the spec's bytes, not the code that reads them, so
//! the entry also records the generator's [`revision`]: an entry made by
//! another revision (or by a Flint that didn't record one) is stale, and the
//! run regenerates and says why.
//!
//! [`revision`]: flint_procgen::Generator::revision

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flint_asset::{AssetFile, AssetMeta, AssetType, ContentStore};
use flint_core::ContentHash;
use flint_procgen::{OutputKind, ProcGenSpec};

use flint_asset::Project;

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
    /// The generator's [`revision`](flint_procgen::Generator::revision) when
    /// it made the files. `None` for an entry from before revisions were
    /// recorded, which no current generator can vouch for.
    pub generator_revision: Option<u32>,
    pub spec_hash: String,
    pub seed: u64,
    pub timestamp: String,
    pub flint_version: String,
    /// (role, content hash) for each file the run wrote.
    pub files: Vec<(Option<String>, String)>,
}

impl Provenance {
    /// Why this entry can't stand for what `generator` at `revision` would
    /// make now, or `None` if it can: `tree_v1 rev 3 -> 4`.
    pub fn stale_for(&self, generator: &str, revision: u32) -> Option<String> {
        let recorded = match self.generator_revision {
            Some(r) if self.generator == generator && r == revision => return None,
            Some(r) => format!("rev {r}"),
            None => "rev unrecorded".to_string(),
        };
        Some(if self.generator == generator {
            format!("{generator} {recorded} -> {revision}")
        } else {
            format!(
                "{} {recorded} -> {generator} rev {revision}",
                self.generator
            )
        })
    }
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
    store_root: PathBuf,
    index_root: PathBuf,
}

impl Store {
    /// The project's store: `<root>/.flint/assets` (shared with `flint
    /// asset` and the AI asset pipeline) and `<root>/.flint/procgen`.
    pub fn open_project(project: &Project) -> Self {
        Self::open(&project.store_dir(), &project.procgen_dir())
    }

    pub fn open(store_root: &Path, index_root: &Path) -> Self {
        Self {
            content: ContentStore::new(store_root),
            store_root: store_root.to_path_buf(),
            index_root: index_root.to_path_buf(),
        }
    }

    /// Where the content-addressed bytes live.
    pub fn store_root(&self) -> &Path {
        &self.store_root
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
        generator_revision: u32,
        kind: OutputKind,
        files: &[WrittenFile],
    ) -> Result<Provenance> {
        let mut recorded = Vec::new();
        for f in files {
            let hash = self
                .content
                .store(&f.path)
                .map_err(|e| anyhow::anyhow!("{e}"))
                .with_context(|| {
                    format!(
                        "failed to store {} in {}",
                        f.path.display(),
                        self.store_root.display()
                    )
                })?;
            recorded.push((f.role.clone(), hash.to_prefixed_hex()));
        }

        let prov = Provenance {
            generator: spec.generator.clone(),
            generator_revision: Some(generator_revision),
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
    if let Some(rev) = prov.generator_revision {
        put("generator_revision", toml::Value::Integer(i64::from(rev)));
    }
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
    if let Some(rev) = p.generator_revision {
        t.insert("generator_revision".into(), i64::from(rev).into());
    }
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
    // Absent in entries written before revisions were recorded; anything
    // else that isn't a u32 is a damaged entry.
    let generator_revision = match t.get("generator_revision") {
        None => None,
        Some(v) => Some(
            v.as_integer()
                .and_then(|i| u32::try_from(i).ok())
                .context("invalid 'generator_revision'")?,
        ),
    };
    Ok(Provenance {
        generator: str_of("generator")?,
        generator_revision,
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
                generator_revision: Some(4),
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

    fn entry(generator: &str, generator_revision: Option<u32>) -> Provenance {
        Provenance {
            generator: generator.into(),
            generator_revision,
            spec_hash: "sha256:ab".into(),
            seed: 7,
            timestamp: "2026-10-31T21:05:37Z".into(),
            flint_version: "0.1.0".into(),
            files: vec![(None, "sha256:01".into())],
        }
    }

    #[test]
    fn an_entry_from_another_revision_is_stale_and_says_which() {
        assert_eq!(entry("tree_v1", Some(4)).stale_for("tree_v1", 4), None);
        assert_eq!(
            entry("tree_v1", Some(3)).stale_for("tree_v1", 4).as_deref(),
            Some("tree_v1 rev 3 -> 4")
        );
        // Newer than the running generator (an older checkout) is stale too.
        assert_eq!(
            entry("tree_v1", Some(5)).stale_for("tree_v1", 4).as_deref(),
            Some("tree_v1 rev 5 -> 4")
        );
        assert_eq!(
            entry("tree_v1", Some(4)).stale_for("tree_v2", 4).as_deref(),
            Some("tree_v1 rev 4 -> tree_v2 rev 4")
        );
    }

    #[test]
    fn an_entry_without_a_revision_is_stale_and_still_parses() {
        let mut p = entry("texture_v1", None);
        assert_eq!(
            p.stale_for("texture_v1", 4).as_deref(),
            Some("texture_v1 rev unrecorded -> 4")
        );
        let text = render_entry(&p);
        assert!(!text.contains("generator_revision"), "{text}");
        assert_eq!(parse_entry(&text).unwrap(), p);
        p.generator_revision = Some(2);
        assert_eq!(parse_entry(&render_entry(&p)).unwrap(), p);
    }

    #[test]
    fn a_damaged_revision_is_an_unreadable_entry() {
        let text = render_entry(&entry("tree_v1", Some(1)))
            .replace("generator_revision = 1", "generator_revision = -1");
        assert!(parse_entry(&text).is_err());
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
