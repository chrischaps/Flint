//! A terrain made from its recipe at load (#1697).
//!
//! A `terrain` component may name its `.terrain.toml` (`spec`). The recipe
//! is then the terrain's source: its `[geometry]`, `[textures]` and
//! `[grass]` are the terrain's settings (a key written on the component
//! wins: [`TerrainConfig::from_spec`], [`TerrainConfig::over`]), and its
//! height layers and splat rules are baked through [`bake`](crate::bake)
//! into the two maps the scene used to name as PNGs.
//!
//! **The bake is cached** ([`baked`]). The key is what the bake reads and
//! nothing else ([`BakeKey`]: the geometry, the height layers, the splat
//! rules and the seed, with [`BAKE_REVISION`]), so an edit to a comment, a
//! texture or the grass never bakes again. A process keeps the last few
//! bakes in memory; under that is a disk tier in the procgen cache's own
//! folder (`.flint/procgen-cache/` at the project root, or
//! `FLINT_PROCGEN_CACHE`, whose `0` turns it off), as files of their own
//! extension, which procgen's trim leaves alone. A hit is the same bytes as
//! the bake: the heights as f32s, the splat as it was made.
//!
//! **The heights are the PNG's**: [`TerrainMaps::png_heights`] quantizes
//! them as `encode_heightmap_png` does, so a terrain made from its recipe
//! and one made from the PNGs `flint gen` wrote from it are the same mesh,
//! bit for bit.

use crate::bake::{height_samples, TerrainMaps};
use crate::spec::{SpecIssue, TerrainSpec};
use crate::{GrassConfig, Heightmap, HeightmapFiles, TerrainConfig};
use flint_core::project::Project;
use flint_core::ContentHash;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Bump when the bake's output changes for the same spec (a new layer
/// rule, a fixed bug): every cached bake is then a miss.
pub const BAKE_REVISION: u32 = 1;

/// The ending of a cached bake's file in the cache folder.
pub const CACHE_EXTENSION: &str = "terrain-maps";

/// Bakes a process keeps in memory.
const MEMORY_BAKES: usize = 4;

/// Bakes the disk tier keeps (the most recently made or used).
const DISK_BAKES: usize = 16;

/// A recipe as a scene's terrain reads it: the spec, what in it nothing
/// reads, and where it is.
#[derive(Debug, Clone)]
pub struct SpecSource {
    pub path: PathBuf,
    pub spec: TerrainSpec,
    pub issues: Vec<SpecIssue>,
}

impl SpecSource {
    /// Read the spec at `path` through the vfs.
    pub fn read(path: &Path) -> Result<Self, String> {
        let (spec, issues) = TerrainSpec::from_file_checked(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            spec,
            issues,
        })
    }

    /// The seed the bake takes: the component's `seed` when it writes one,
    /// else the spec's `[seed] value`. A spec whose seed mode is `random`
    /// still bakes its value here: a scene is drawn the same every load.
    pub fn seed(&self, component_seed: Option<u64>) -> u64 {
        component_seed.unwrap_or(self.spec.seed.value)
    }

    /// The folder the spec is in, which its `[textures]` paths are from.
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new(""))
    }
}

/// What the bake reads of a spec, and the seed: the cache's key.
#[derive(serde::Serialize)]
pub struct BakeKey<'a> {
    revision: u32,
    seed: u64,
    geometry: &'a crate::spec::GeometryConfig,
    height_layers: &'a [crate::spec::HeightLayer],
    splat_rules: &'a [crate::spec::SplatRule],
}

impl<'a> BakeKey<'a> {
    pub fn of(spec: &'a TerrainSpec, seed: u64) -> Self {
        Self {
            revision: BAKE_REVISION,
            seed,
            geometry: &spec.geometry,
            height_layers: &spec.height_layers,
            splat_rules: &spec.splat_rules,
        }
    }

    /// The key's hash: its TOML's, so two spellings of one recipe agree.
    pub fn hash(&self) -> ContentHash {
        let text = toml::to_string(self).unwrap_or_default();
        ContentHash::from_str(&text)
    }
}

/// Where a bake came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baked {
    /// This process baked it before.
    Memory,
    /// The disk tier had it.
    Disk,
    /// Baked now.
    Now,
}

impl Baked {
    pub fn words(self) -> &'static str {
        match self {
            Baked::Memory => "from memory",
            Baked::Disk => "from the disk cache",
            Baked::Now => "baked",
        }
    }
}

static MEMORY: Mutex<Vec<(ContentHash, Arc<TerrainMaps>)>> = Mutex::new(Vec::new());

/// The maps `spec` bakes at `seed`: from memory, else from the disk tier in
/// `disk` (when there is one), else baked now and kept in both.
pub fn baked(spec: &TerrainSpec, seed: u64, disk: Option<&Path>) -> (Arc<TerrainMaps>, Baked) {
    let key = BakeKey::of(spec, seed).hash();
    if let Some(maps) = remembered(&key) {
        return (maps, Baked::Memory);
    }
    if let Some(maps) = disk.and_then(|dir| read_cached(dir, &key)) {
        let maps = Arc::new(maps);
        remember(key, maps.clone());
        return (maps, Baked::Disk);
    }
    let maps = Arc::new(crate::bake(spec, seed));
    if let Some(dir) = disk {
        write_cached(dir, &key, &maps);
    }
    remember(key, maps.clone());
    (maps, Baked::Now)
}

fn remembered(key: &ContentHash) -> Option<Arc<TerrainMaps>> {
    let mut memory = MEMORY.lock().ok()?;
    let at = memory.iter().position(|(k, _)| k == key)?;
    // The newest last
    let entry = memory.remove(at);
    let maps = entry.1.clone();
    memory.push(entry);
    Some(maps)
}

fn remember(key: ContentHash, maps: Arc<TerrainMaps>) {
    if let Ok(mut memory) = MEMORY.lock() {
        memory.retain(|(k, _)| *k != key);
        memory.push((key, maps));
        let over = memory.len().saturating_sub(MEMORY_BAKES);
        memory.drain(..over);
    }
}

/// Forget every bake this process keeps in memory (tests, and a tool that
/// wants the disk tier's word).
pub fn forget_bakes() {
    if let Ok(mut memory) = MEMORY.lock() {
        memory.clear();
    }
}

/// Where the disk tier is for a scene in `scene_dir`: the procgen cache's
/// folder, by its rule (`FLINT_PROCGEN_CACHE=<dir>`, `0` for none, else
/// `.flint/procgen-cache/` at the project root). None on the web.
pub fn cache_dir(project: &flint_core::project::Project, scene_dir: &Path) -> Option<PathBuf> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    match flint_core::env::PROCGEN_CACHE.get() {
        Some(flint_core::env::Value::Off) => None,
        Some(flint_core::env::Value::Path(dir)) => Some(dir),
        _ => Some(
            project
                .root_for(scene_dir)
                .join(".flint")
                .join("procgen-cache"),
        ),
    }
}

/// The file magic and layout version of a cached bake.
const MAGIC: &[u8; 4] = b"FTMC";
const FORMAT: u32 = 1;

fn cache_file(dir: &Path, key: &ContentHash) -> PathBuf {
    dir.join(format!("{}.{CACHE_EXTENSION}", key.to_hex()))
}

/// A bake's bytes: magic, format, resolution, seed, then the heights as
/// little-endian f32s and the splat as it is.
pub fn encode_maps(maps: &TerrainMaps) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + maps.heights.len() * 4 + maps.splat.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&maps.resolution.to_le_bytes());
    out.extend_from_slice(&maps.seed.to_le_bytes());
    for h in &maps.heights {
        out.extend_from_slice(&h.to_le_bytes());
    }
    out.extend_from_slice(&maps.splat);
    out
}

/// [`encode_maps`] read back; `None` for anything else.
pub fn decode_maps(bytes: &[u8]) -> Option<TerrainMaps> {
    let (head, rest) = bytes.split_at_checked(20)?;
    if &head[..4] != MAGIC || u32::from_le_bytes(head[4..8].try_into().ok()?) != FORMAT {
        return None;
    }
    let resolution = u32::from_le_bytes(head[8..12].try_into().ok()?);
    let seed = u64::from_le_bytes(head[12..20].try_into().ok()?);
    let texels = resolution as usize * resolution as usize;
    if rest.len() != texels * 8 {
        return None;
    }
    let (heights, splat) = rest.split_at(texels * 4);
    let heights = heights
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    Some(TerrainMaps {
        heights,
        splat: splat.to_vec(),
        resolution,
        seed,
    })
}

fn read_cached(dir: &Path, key: &ContentHash) -> Option<TerrainMaps> {
    let path = cache_file(dir, key);
    let bytes = flint_core::vfs::read(&path).ok()?;
    let maps = decode_maps(&bytes);
    if maps.is_some() {
        touch(&path);
    } else {
        remove(&path);
    }
    maps
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)] // the engine's own cache folder on the disk, which no bundle holds
fn touch(path: &Path) {
    if let Ok(file) = std::fs::File::options().append(true).open(path) {
        let _ = file.set_modified(web_time::SystemTime::now());
    }
}

#[cfg(target_arch = "wasm32")]
fn touch(_path: &Path) {}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)] // the engine's own cache folder on the disk, which no bundle holds
fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(target_arch = "wasm32")]
fn remove(_path: &Path) {}

/// Write beside the final name, rename into place, then keep the newest
/// [`DISK_BAKES`]. A failure is a cache that didn't fill, nothing more.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)] // writes and lists the engine's own cache folder on the disk
fn write_cached(dir: &Path, key: &ContentHash, maps: &TerrainMaps) {
    let path = cache_file(dir, key);
    let tmp = path.with_extension(format!("{CACHE_EXTENSION}.{}.tmp", std::process::id()));
    let written = std::fs::create_dir_all(dir)
        .and_then(|_| std::fs::write(&tmp, encode_maps(maps)))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut bakes: Vec<(web_time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == CACHE_EXTENSION))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if bakes.len() > DISK_BAKES {
        bakes.sort();
        for (_, old) in &bakes[..bakes.len() - DISK_BAKES] {
            if *old != path {
                let _ = std::fs::remove_file(old);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn write_cached(_dir: &Path, _key: &ContentHash, _maps: &TerrainMaps) {}

/// A recipe read and baked: what a worker thread hands back (#1697), with
/// no world or GPU in it.
#[derive(Debug, Clone)]
pub struct Recipe {
    pub source: SpecSource,
    pub maps: Arc<TerrainMaps>,
    pub baked: Baked,
    pub seed: u64,
    /// How long the read and the bake (or the cache's answer) took.
    pub ms: f64,
}

impl Recipe {
    /// Read the spec at `path` and bake it at the component's `seed` (else
    /// the spec's), through the cache ([`baked`]; `disk` its disk tier).
    pub fn bake(path: &Path, seed: Option<u64>, disk: Option<&Path>) -> Result<Self, String> {
        let start = web_time::Instant::now();
        let source = SpecSource::read(path)?;
        let seed = source.seed(seed);
        let (maps, baked) = baked(&source.spec, seed, disk);
        Ok(Self {
            source,
            maps,
            baked,
            seed,
            ms: start.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// `relief.terrain.toml (seed 70): baked in 840 ms`, for the load line.
    pub fn words(&self) -> String {
        format!(
            "{} (seed {}): {} in {:.0} ms",
            flint_core::project::shown(&self.source.path),
            self.seed,
            self.baked.words(),
            self.ms
        )
    }
}

/// Where a terrain's splat map comes from.
#[derive(Debug, Clone)]
pub enum SplatMap {
    None,
    /// A name the project's search takes from the scene's folder.
    File(String),
    /// Its recipe's bake.
    Baked(Arc<TerrainMaps>),
}

/// A terrain's settings and maps, from its component over its recipe: all
/// a host needs to build it, with no GPU in it.
#[derive(Debug, Clone)]
pub struct MadeTerrain {
    pub config: TerrainConfig,
    pub grass: GrassConfig,
    pub heightmap: Heightmap,
    pub splat: SplatMap,
    /// The recipe it was made from, when it names one.
    pub recipe: Option<Recipe>,
}

/// The recipe a `terrain` component names (`spec`), found by the project's
/// search from the scene's folder: `None` when it names none, `Err` (the
/// paths tried) when it isn't there.
pub fn spec_path(
    view: &flint_ecs::ComponentView<'_>,
    project: &Project,
    scene_dir: &Path,
) -> Option<Result<PathBuf, String>> {
    let name = view.opt_str("spec").filter(|s| !s.is_empty())?;
    Some(
        project
            .resolve(flint_core::project::AssetKind::Files, name, scene_dir)
            .map_err(|not_found| format!("terrain spec: {not_found}")),
    )
}

/// A terrain component made into its settings and maps (#1697): read its
/// recipe and bake it (a cache hit when nothing the bake reads changed),
/// take the recipe's settings, then the component's keys over them. A
/// `heightmap` or `splat_map` the component writes is used in place of the
/// bake's. A spec that can't be read, or no heightmap at all, is `Err`.
pub fn make_terrain(
    view: &flint_ecs::ComponentView<'_>,
    project: &Project,
    scene_dir: &Path,
) -> Result<MadeTerrain, String> {
    let recipe = match spec_path(view, project, scene_dir) {
        None => None,
        Some(path) => {
            let seed = view.opt_i64("seed").map(|s| s as u64);
            Some(Recipe::bake(
                &path?,
                seed,
                cache_dir(project, scene_dir).as_deref(),
            )?)
        }
    };
    made_from(view, recipe, project, scene_dir)
}

/// [`make_terrain`] with its recipe already baked (a hot reload's worker
/// baked it): the settings and the maps.
pub fn made_from(
    view: &flint_ecs::ComponentView<'_>,
    recipe: Option<Recipe>,
    project: &Project,
    scene_dir: &Path,
) -> Result<MadeTerrain, String> {
    let (base, grass_base) = match &recipe {
        Some(recipe) => (
            TerrainConfig::from_spec(&recipe.source, scene_dir),
            recipe.source.spec.grass.clone().unwrap_or_default(),
        ),
        None => (TerrainConfig::default(), GrassConfig::default()),
    };
    let config = TerrainConfig::over(view, base);
    let grass = GrassConfig::over(view, &grass_base);

    let heightmap = if !config.heightmap_path.is_empty() {
        let path = project.file_or_beside("terrain heightmap", &config.heightmap_path, scene_dir);
        Heightmap::from_png(&path)?
    } else if let Some(recipe) = &recipe {
        let maps = &recipe.maps;
        Heightmap::from_raw(maps.png_heights(), maps.resolution, maps.resolution)
    } else {
        return Err("missing heightmap path (name a `spec` or a `heightmap`)".into());
    };
    let splat = if !config.splat_map_path.is_empty() {
        SplatMap::File(config.splat_map_path.clone())
    } else if let Some(recipe) = &recipe {
        SplatMap::Baked(recipe.maps.clone())
    } else {
        SplatMap::None
    };
    Ok(MadeTerrain {
        config,
        grass,
        heightmap,
        splat,
        recipe,
    })
}

impl TerrainMaps {
    /// The heights as a heightmap PNG holds them: each quantized to 16 bits
    /// and read back as `HeightmapFiles::from_png` reads it, so a terrain
    /// from its recipe is the mesh of one from its baked PNG, bit for bit.
    pub fn png_heights(&self) -> Vec<f32> {
        height_samples(&self.heights)
            .into_iter()
            .map(|s| s as f32 / 65535.0)
            .collect()
    }
}

/// `path` as a name the project's search takes from `scene_dir`: `./` and
/// the way from the scene's folder, so a spec's `[textures]` path (from the
/// spec's folder) is found as the editor finds it. Both paths are spelled
/// from one place (the resolver keeps the scene directory's spelling), so
/// the way is worked out from their parts alone.
pub fn from_scene_dir(path: &Path, scene_dir: &Path) -> String {
    use std::path::Component;
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    let (to, from) = (parts(path), parts(scene_dir));
    if path.is_absolute() != scene_dir.is_absolute() {
        return flint_core::project::shown(path);
    }
    let shared = to.iter().zip(&from).take_while(|(a, b)| a == b).count();
    let mut way: Vec<String> = vec![".".into()];
    way.extend(std::iter::repeat_n("..".to_string(), from.len() - shared));
    way.extend(to[shared..].iter().cloned());
    way.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> TerrainSpec {
        toml::from_str(text).unwrap()
    }

    const SMALL: &str = r#"
[meta]
name = "small"

[seed]
value = 9

[geometry]
resolution = 17

[[height_layers]]
op = "noise"
octaves = 2

[[splat_rules]]
layer = 1
height_min = 0.5
"#;

    #[test]
    fn a_cached_bake_reads_back_as_the_bake() {
        let maps = crate::bake(&spec(SMALL), 9);
        assert_eq!(decode_maps(&encode_maps(&maps)), Some(maps.clone()));
        let mut bytes = encode_maps(&maps);
        bytes.pop();
        assert_eq!(decode_maps(&bytes), None);
    }

    #[test]
    fn the_key_is_what_the_bake_reads() {
        let base = spec(SMALL);
        let key = BakeKey::of(&base, 9).hash();
        // Textures, grass, name and comments: the same bake
        let mut dressed = spec(&format!(
            "# a comment\n{SMALL}\n[textures]\ntexture_tile = 3.0\n[grass]\nenabled = true\n"
        ));
        dressed.meta.name = "renamed".into();
        assert_eq!(BakeKey::of(&dressed, 9).hash(), key);
        // A seed, a layer, a rule: another
        assert_ne!(BakeKey::of(&base, 10).hash(), key);
        let mut layered = base.clone();
        layered.height_layers.clear();
        assert_ne!(BakeKey::of(&layered, 9).hash(), key);
        let mut ruled = base.clone();
        ruled.splat_rules[0].height_min = 0.4;
        assert_ne!(BakeKey::of(&ruled, 9).hash(), key);
    }

    #[test]
    fn a_bake_is_kept_in_memory_and_on_the_disk() {
        let dir = std::env::temp_dir().join(format!("flint-terrain-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = spec(SMALL);
        // A seed no other test bakes, so the memory is this test's
        s.seed.value = 0x1697;
        let (made, how) = baked(&s, s.seed.value, Some(&dir));
        assert_eq!(how, Baked::Now);
        let (again, how) = baked(&s, s.seed.value, Some(&dir));
        assert_eq!((how, &again), (Baked::Memory, &made));
        forget_bakes();
        let (read, how) = baked(&s, s.seed.value, Some(&dir));
        assert_eq!((how, &*read), (Baked::Disk, &*made));
        assert_eq!(*made, crate::bake(&s, s.seed.value));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn png_heights_are_the_pngs_heights() {
        use crate::HeightmapFiles;
        let maps = crate::bake(&spec(SMALL), 9);
        let png = maps.heightmap_png().unwrap();
        let path =
            std::env::temp_dir().join(format!("flint-png-heights-{}.png", std::process::id()));
        std::fs::write(&path, png).unwrap();
        let read = crate::Heightmap::from_png(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(read.heights(), maps.png_heights().as_slice());
    }

    #[test]
    fn a_path_from_the_scene_dir() {
        let way = |p: &str, d: &str| from_scene_dir(Path::new(p), Path::new(d));
        assert_eq!(
            way("demos/relief/land/grass.png", "demos/relief"),
            "./land/grass.png"
        );
        assert_eq!(way("land/grass.png", "."), "./land/grass.png");
        assert_eq!(
            way("demos/land/grass.png", "demos/relief"),
            "./../land/grass.png"
        );
        assert_eq!(way("./levels/x.png", "./levels"), "./x.png");
    }
}
