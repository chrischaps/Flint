//! `flint bundle`: pack a game folder into `.flintpack` files for the web
//! player (#98; format in `flint_core::bundle`, docs/design/web.md).
//!
//! ```bash
//! flint bundle ../Float                  # dist/float.flintpack + dist/float.audio.flintpack
//! flint bundle ../Float --list           # every file, its size and its bundle
//! flint bundle . --out-dir web --scene scenes/title.scene.toml
//! ```
//!
//! The packer walks the game root, drops what a player never reads (the
//! default excludes below, plus `bundle.toml`'s), and puts audio in a late
//! bundle the web player fetches once the game is running: a browser can't
//! play sound before the first click anyway. Then it checks the bundle
//! against itself before writing anything: every scene must load with its
//! prefabs, every archetype must be in the bundled schemas, and every file a
//! scene, prefab, particle effect or script names must be packed. A missing
//! file fails here rather than as a pink texture deep in play.
//!
//! `bundle.toml` in the game root (every key optional):
//!
//! ```toml
//! name = "float"
//! scene = "scenes/float.scene.toml"
//! schemas = ["engine/schemas", "schemas"]
//! exclude = ["scenes/snap_*.scene.toml"]   # added to the defaults
//! include = []                             # wins over any exclude
//! [late]                                   # replaces the default { audio = [...] }
//! ambience = ["audio/ocean_loop.ogg"]     # fetched first: the sea at Begin
//! audio = ["audio/**"]
//! ```
//!
//! Late parts are fetched one after another in the order `[late]` lists
//! them, and a file goes to the first part whose patterns match it. Put
//! what is heard first in a small part of its own (#124).

use anyhow::{bail, Context, Result};
use clap::Args;
use flint_core::bundle::{Bundle, BundleManifest, LateBundle, EXTENSION};
use flint_core::vfs::{self, normalize, MemorySource};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Args)]
pub struct BundleArgs {
    /// The game's root folder
    #[arg(default_value = ".")]
    pub game: PathBuf,

    /// Where to write the bundles (default: `dist/` in the game root)
    #[arg(long)]
    pub out_dir: Option<PathBuf>,

    /// The scene the player starts (overrides bundle.toml)
    #[arg(long)]
    pub scene: Option<String>,

    /// The bundle's name (overrides bundle.toml; default: the folder's name)
    #[arg(long)]
    pub name: Option<String>,

    /// Print every packed file with its size and bundle
    #[arg(long)]
    pub list: bool,

    /// Check and report, but write nothing
    #[arg(long)]
    pub dry_run: bool,
}

/// What nobody plays: tooling, docs, captures, source art, the engine's own
/// tree. `engine/schemas` comes back through [`DEFAULT_INCLUDE`].
const DEFAULT_EXCLUDE: &[&str] = &[
    ".*/**",
    "**/.*",
    "target/**",
    "dist/**",
    "docs/**",
    "screenshots/**",
    "snapshots/**",
    "renders/**",
    "reference/**",
    "references/**",
    "scratch/**",
    "tmp/**",
    "logs/**",
    "tools/**",
    "src/**",
    "engine/**",
    "Cargo.toml",
    "Cargo.lock",
    "LICENSE*",
    "bundle.toml",
    "**/*.md",
    "**/*.bat",
    "**/*.sh",
    "**/*.ps1",
    "**/*.py",
    "**/*.blend",
    "**/*.blend1",
    "**/*.psd",
    "**/*.kra",
    "**/*.xcf",
    "**/*.log",
    "**/*.tmp",
    "**/*.flintpack",
];

/// Files a game reads from the engine subtree.
const DEFAULT_INCLUDE: &[&str] = &["engine/schemas/**"];

/// Audio: fetched after the first frame.
const DEFAULT_LATE_AUDIO: &[&str] = &["**/*.ogg", "**/*.wav", "**/*.mp3", "**/*.flac"];

/// Extensions that mark a string in a scene or script as a file to pack.
const ASSET_EXTENSIONS: &[&str] = &[
    "glb", "gltf", "png", "jpg", "jpeg", "hdr", "ktx2", "ogg", "wav", "mp3", "flac", "rhai", "ttf",
    "otf", "toml", "wgsl",
];

/// The folders the runtime looks a bare name up in, beside a scene and one
/// level above it (`scene_loading.rs`, `model_loader.rs`, `sync.rs`).
const ASSET_DIRS: &[&str] = &[
    "",
    "models",
    "scripts",
    "audio",
    "textures",
    "sprites",
    "particles",
    "animations",
    "prefabs",
    "fonts",
    "terrain",
    "specs",
    "levels",
    "scenes",
];

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleConfig {
    name: Option<String>,
    scene: Option<String>,
    schemas: Option<Vec<String>>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    include: Vec<String>,
    late: Option<BTreeMap<String, Vec<String>>>,
    /// `[late]`'s keys in the order the file lists them (a `BTreeMap`
    /// forgets it).
    #[serde(skip)]
    late_order: Vec<String>,
}

impl BundleConfig {
    fn parse(text: &str) -> Result<Self> {
        let mut config: Self = toml::from_str(text)?;
        let doc: toml_edit::DocumentMut = text.parse()?;
        if let Some(late) = doc.get("late").and_then(|l| l.as_table_like()) {
            config.late_order = late.iter().map(|(k, _)| k.to_string()).collect();
        }
        Ok(config)
    }
}

/// Which files go where.
#[derive(Debug)]
struct Rules {
    exclude: Vec<String>,
    include: Vec<String>,
    /// Part name → patterns. A file goes to the first part that matches.
    late: Vec<(String, Vec<String>)>,
}

impl Rules {
    fn from_config(config: &BundleConfig) -> Self {
        let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut exclude = owned(DEFAULT_EXCLUDE);
        exclude.extend(config.exclude.iter().cloned());
        let mut include = owned(DEFAULT_INCLUDE);
        include.extend(config.include.iter().cloned());
        let late = match &config.late {
            Some(parts) => {
                let mut parts: Vec<_> = parts.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let rank = |k: &str| config.late_order.iter().position(|o| o == k);
                parts.sort_by_key(|(k, _)| rank(k));
                parts
            }
            None => vec![("audio".to_string(), owned(DEFAULT_LATE_AUDIO))],
        };
        Self {
            exclude,
            include,
            late,
        }
    }

    fn packs(&self, path: &str) -> bool {
        let any = |list: &[String]| list.iter().any(|p| glob_match(p, path));
        any(&self.include) || !any(&self.exclude)
    }

    /// `None` for the main bundle, or the late part's name.
    fn part_of(&self, path: &str) -> Option<&str> {
        self.late
            .iter()
            .find(|(_, patterns)| patterns.iter().any(|p| glob_match(p, path)))
            .map(|(part, _)| part.as_str())
    }
}

/// Match a `/`-separated path against a glob: `*` and `?` stay inside one
/// segment, `**` spans any number of them (none included).
fn glob_match(pattern: &str, path: &str) -> bool {
    fn segments(pattern: &[&str], path: &[&str]) -> bool {
        match pattern.split_first() {
            None => path.is_empty(),
            Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
            Some((first, rest)) => match path.split_first() {
                Some((seg, tail)) => {
                    segment(first.as_bytes(), seg.as_bytes()) && segments(rest, tail)
                }
                None => false,
            },
        }
    }
    fn segment(p: &[u8], s: &[u8]) -> bool {
        match (p.split_first(), s.split_first()) {
            (None, None) => true,
            (Some((b'*', rest)), _) => (0..=s.len()).any(|skip| segment(rest, &s[skip..])),
            (Some((b'?', rest)), Some((_, tail))) => segment(rest, tail),
            (Some((c, rest)), Some((d, tail))) if c == d => segment(rest, tail),
            _ => false,
        }
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&pattern, &path)
}

/// Every file under `root`, as normalized paths relative to it, that the
/// rules keep. Excluded directories are not descended into unless an
/// include could reach inside them.
fn collect(root: &Path, rules: &Rules) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let dir = root.join(&rel);
        let entries =
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let rel_path = rel.join(entry.file_name());
            let key = normalize(&rel_path);
            if entry.file_type()?.is_dir() {
                let probe = format!("{key}/x");
                let reachable = rules.packs(&probe)
                    || rules
                        .include
                        .iter()
                        .any(|p| p.starts_with(&format!("{key}/")) || p.starts_with("**"));
                if reachable {
                    stack.push(rel_path);
                }
            } else if rules.packs(&key) {
                out.push(key);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Whether a string names a file: a known extension after a non-empty stem,
/// and nothing that can't be in a path. `".ogg"` (a suffix a script appends
/// to) and URLs don't count.
fn looks_like_file(s: &str) -> bool {
    if s.contains("://") || s.contains(char::is_whitespace) || s.contains(['{', '}', '"', '*']) {
        return false;
    }
    let name = s.rsplit(['/', '\\']).next().unwrap_or(s);
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty() && ASSET_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        }
        None => false,
    }
}

/// Where the runtime would look for `name` named from a file in `from_dir`.
fn candidates(name: &str, from_dirs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for dir in from_dirs {
        let dir = Path::new(dir);
        for base in [Some(dir), dir.parent()].into_iter().flatten() {
            for sub in ASSET_DIRS {
                out.push(normalize(&base.join(sub).join(name)));
            }
        }
    }
    for sub in ASSET_DIRS {
        out.push(normalize(&Path::new(sub).join(name)));
    }
    out
}

/// One unresolved reference: who names what.
#[derive(Debug, PartialEq)]
struct Missing {
    from: String,
    what: String,
}

/// Collect every string in a TOML value that names a file, plus each
/// `model.asset` (a bare model name).
fn toml_refs(value: &toml::Value, key: &str, out: &mut Vec<String>) {
    match value {
        toml::Value::String(s) if looks_like_file(s) => out.push(s.clone()),
        toml::Value::Table(t) => {
            for (k, v) in t {
                if key == "model" && k == "asset" {
                    if let Some(name) = v.as_str() {
                        if !looks_like_file(name) {
                            out.push(format!("{name}.glb"));
                            continue;
                        }
                    }
                }
                toml_refs(v, k, out);
            }
        }
        toml::Value::Array(a) => a.iter().for_each(|v| toml_refs(v, key, out)),
        _ => {}
    }
}

/// String literals in a Rhai script that name files.
fn rhai_refs(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(['"', '\n']) else {
            break;
        };
        let literal = &after[..end];
        if looks_like_file(literal) {
            out.push(literal.to_string());
        }
        rest = &after[(end + 1).min(after.len())..];
    }
    // `// "foo.png"` in a comment is still worth packing; no need to skip.
    out
}

/// Every reference in the packed files that names nothing packed.
fn check_references(files: &BTreeMap<String, Vec<u8>>) -> Vec<Missing> {
    let scene_dirs: Vec<String> = files
        .keys()
        .filter(|p| p.ends_with(".scene.toml"))
        .map(|p| parent_of(p))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let model_ok = |name: &str| {
        // A bare model name may be a glTF, or a procgen spec the player builds.
        let stem = name.trim_end_matches(".glb");
        files.keys().any(|p| {
            p.ends_with(&format!("/{stem}.gltf"))
                || p.ends_with(&format!("/{stem}.procgen.toml"))
                || p == &format!("{stem}.procgen.toml")
        })
    };

    let mut missing = Vec::new();
    for (path, bytes) in files {
        let refs = if path.ends_with(".scene.toml")
            || path.ends_with(".prefab.toml")
            || path.ends_with(".particles.toml")
            || path.ends_with(".chunk.toml")
        {
            let Ok(text) = std::str::from_utf8(bytes) else {
                continue;
            };
            let Ok(value) = text.parse::<toml::Value>() else {
                missing.push(Missing {
                    from: path.clone(),
                    what: "(not valid TOML)".into(),
                });
                continue;
            };
            let mut refs = Vec::new();
            toml_refs(&value, "", &mut refs);
            refs
        } else if path.ends_with(".rhai") {
            rhai_refs(&String::from_utf8_lossy(bytes))
        } else {
            continue;
        };

        let mut from_dirs = vec![parent_of(path)];
        if path.ends_with(".rhai") {
            from_dirs.extend(scene_dirs.iter().cloned());
        }
        for name in refs.into_iter().collect::<BTreeSet<_>>() {
            let found = candidates(&name, &from_dirs)
                .iter()
                .any(|c| files.contains_key(c));
            if !found && !(name.ends_with(".glb") && model_ok(&name)) {
                missing.push(Missing {
                    from: path.clone(),
                    what: name,
                });
            }
        }
    }
    missing
}

fn parent_of(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

/// Load each scene from the packed files the way the player would: schemas
/// from the bundle, prefabs expanded through the vfs, every archetype known.
fn check_scenes(source: Arc<MemorySource>, scenes: &[String], schemas: &[String]) -> Vec<Missing> {
    vfs::scoped(source, || {
        let mut problems = Vec::new();
        let mut registry = match flint_schema::SchemaRegistry::load_from_directories(schemas) {
            Ok(r) => r,
            Err(e) => {
                problems.push(Missing {
                    from: schemas.join(", "),
                    what: format!("(schemas don't load: {e})"),
                });
                return problems;
            }
        };
        // `flint-player` merges a scene's own schemas/ beside it too.
        for scene in scenes {
            let extra: Vec<String> = flint_schema::discover_schema_dirs(scene)
                .into_iter()
                .map(|d| normalize(&d))
                .filter(|d| !schemas.contains(d))
                .collect();
            if !extra.is_empty() {
                let mut all = schemas.to_vec();
                all.extend(extra);
                if let Ok(r) = flint_schema::SchemaRegistry::load_from_directories(&all) {
                    registry = r;
                }
            }
        }
        for scene in scenes {
            match flint_scene::load_scene(scene, &registry) {
                Ok((_, file)) => {
                    let mut unknown: BTreeSet<&str> = BTreeSet::new();
                    for def in file.entities.values() {
                        if let Some(arch) = &def.archetype {
                            if registry.get_archetype(arch).is_none() {
                                unknown.insert(arch);
                            }
                        }
                    }
                    for arch in unknown {
                        problems.push(Missing {
                            from: scene.clone(),
                            what: format!("archetype \"{arch}\" (in no bundled schema)"),
                        });
                    }
                }
                Err(e) => problems.push(Missing {
                    from: scene.clone(),
                    what: format!("(doesn't load: {e})"),
                }),
            }
        }
        problems
    })
}

fn mb(bytes: u64) -> String {
    format!("{:.2} MB", bytes as f64 / 1_000_000.0)
}

pub fn run(args: BundleArgs) -> Result<()> {
    let root = args.game.as_path();
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    let config_path = root.join("bundle.toml");
    let config: BundleConfig = if config_path.is_file() {
        let text = std::fs::read_to_string(&config_path)?;
        BundleConfig::parse(&text).with_context(|| format!("reading {}", config_path.display()))?
    } else {
        BundleConfig::default()
    };
    let rules = Rules::from_config(&config);

    let name = args
        .name
        .clone()
        .or(config.name.clone())
        .or_else(|| {
            std::fs::canonicalize(root)
                .ok()?
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
        })
        .unwrap_or_else(|| "game".into());

    let paths = collect(root, &rules)?;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for path in &paths {
        let bytes = std::fs::read(root.join(path)).with_context(|| format!("reading {path}"))?;
        files.insert(path.clone(), bytes);
    }

    let scenes: Vec<String> = files
        .keys()
        .filter(|p| p.ends_with(".scene.toml"))
        .cloned()
        .collect();
    let scene = match args.scene.clone().or(config.scene.clone()) {
        Some(s) => normalize(Path::new(&s)),
        None if scenes.len() == 1 => scenes[0].clone(),
        None => bail!(
            "{} scenes are packed; name the one to start with --scene or `scene =` in bundle.toml:\n  {}",
            scenes.len(),
            scenes.join("\n  ")
        ),
    };
    if !files.contains_key(&scene) {
        bail!("the start scene {scene} is not packed");
    }
    let schemas: Vec<String> = config
        .schemas
        .clone()
        .unwrap_or_else(|| vec!["engine/schemas".into(), "schemas".into()])
        .into_iter()
        .map(|s| normalize(Path::new(&s)))
        .filter(|s| files.keys().any(|p| p.starts_with(&format!("{s}/"))))
        .collect();

    // Check before writing: a bundle with a hole in it fails here.
    let mut problems = check_references(&files);
    let source: MemorySource = files.iter().map(|(p, b)| (p.as_str(), b.clone())).collect();
    problems.extend(check_scenes(Arc::new(source), &scenes, &schemas));
    if !problems.is_empty() {
        let mut msg = format!(
            "{} unresolved reference(s); nothing written:\n",
            problems.len()
        );
        for m in &problems {
            msg.push_str(&format!("  {}  ->  {}\n", m.from, m.what));
        }
        msg.push_str("Pack the missing files, or leave the file that names them out with `exclude` in bundle.toml.");
        bail!(msg);
    }

    // Split into the main bundle and the late parts.
    let mut parts: BTreeMap<Option<String>, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    parts.entry(None).or_default();
    for (path, bytes) in files {
        let part = rules.part_of(&path).map(str::to_string);
        parts.entry(part).or_default().push((path, bytes));
    }

    if args.list {
        for (part, list) in &parts {
            let label = part.as_deref().unwrap_or("main");
            for (path, bytes) in list {
                println!("{:>10}  {label:<6}  {path}", bytes.len());
            }
        }
        println!();
    }

    let out_dir = args.out_dir.clone().unwrap_or_else(|| root.join("dist"));
    let file_name = |part: &Option<String>| match part {
        None => format!("{name}.{EXTENSION}"),
        Some(p) => format!("{name}.{p}.{EXTENSION}"),
    };

    let mut late = Vec::new();
    let mut written = Vec::new();
    // In `[late]`'s order: the player fetches them in this order.
    let late_parts = rules.late.iter().filter_map(|(part, _)| {
        let part = Some(part.clone());
        parts.get_key_value(&part).filter(|(_, l)| !l.is_empty())
    });
    for (part, list) in late_parts {
        let manifest = BundleManifest {
            name: name.clone(),
            part: part.clone().unwrap(),
            ..Default::default()
        };
        let bytes = Bundle::write(manifest, list.iter().map(|(p, b)| (p.as_str(), b.clone())));
        late.push(LateBundle {
            part: part.clone().unwrap(),
            url: file_name(part),
            size: bytes.len() as u64,
        });
        written.push((file_name(part), list.len(), bytes));
    }
    let main = &parts[&None];
    let manifest = BundleManifest {
        name: name.clone(),
        scene: Some(scene.clone()),
        schemas: schemas.clone(),
        late,
        ..Default::default()
    };
    let bytes = Bundle::write(manifest, main.iter().map(|(p, b)| (p.as_str(), b.clone())));
    written.insert(0, (file_name(&None), main.len(), bytes));

    println!("{name}: starts {scene}; schemas {}", schemas.join(", "));
    for (file, count, bytes) in &written {
        println!(
            "  {:<32} {:>4} files  {:>9}",
            file,
            count,
            mb(bytes.len() as u64)
        );
    }
    if args.dry_run {
        println!("(dry run: nothing written)");
        return Ok(());
    }
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    for (file, _, bytes) in &written {
        std::fs::write(out_dir.join(file), bytes)?;
    }
    println!("written to {}", out_dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("docs/**", "docs/a/b.md"));
        assert!(glob_match("**/*.md", "README.md"));
        assert!(glob_match("**/*.md", "a/b/c.md"));
        assert!(!glob_match("**/*.md", "a/b/c.mdx"));
        assert!(glob_match(
            "scenes/snap_*.scene.toml",
            "scenes/snap_dusk.scene.toml"
        ));
        assert!(!glob_match(
            "scenes/snap_*.scene.toml",
            "scenes/float.scene.toml"
        ));
        assert!(glob_match("LICENSE*", "LICENSE"));
        assert!(!glob_match("LICENSE*", "docs/LICENSE"));
        assert!(glob_match("**/.*", "audio/.gitkeep"));
        assert!(glob_match(".*/**", ".git/HEAD"));
        assert!(!glob_match(".*/**", "scenes/a.toml"));
        assert!(glob_match("a?c", "abc"));
    }

    #[test]
    fn rules_keep_engine_schemas_and_drop_the_rest() {
        let rules = Rules::from_config(&BundleConfig {
            exclude: vec!["scenes/mixamo_*".into()],
            ..Default::default()
        });
        assert!(rules.packs("engine/schemas/components/transform.toml"));
        assert!(!rules.packs("engine/crates/flint-core/src/lib.rs"));
        assert!(!rules.packs("docs/notes.md"));
        assert!(!rules.packs("CLAUDE.md"));
        assert!(!rules.packs("play.bat"));
        assert!(!rules.packs("scenes/mixamo_spike.scene.toml"));
        assert!(rules.packs("scenes/float.scene.toml"));
        assert!(rules.packs("models/raft.glb"));
        assert_eq!(rules.part_of("audio/music_day.ogg"), Some("audio"));
        assert_eq!(rules.part_of("models/raft.glb"), None);
    }

    #[test]
    fn late_parts_keep_the_order_bundle_toml_gives_them() {
        let config = BundleConfig::parse(
            r#"
            [late]
            sea = ["audio/ocean_loop.ogg", "audio/wind_*.ogg"]
            audio = ["**/*.ogg"]
            "#,
        )
        .unwrap();
        let rules = Rules::from_config(&config);
        let names: Vec<_> = rules.late.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["sea", "audio"], "file order, not alphabetical");
        assert_eq!(rules.part_of("audio/ocean_loop.ogg"), Some("sea"));
        assert_eq!(rules.part_of("audio/wind_light.ogg"), Some("sea"));
        assert_eq!(rules.part_of("audio/music_day.ogg"), Some("audio"));
    }

    #[test]
    fn file_strings() {
        assert!(looks_like_file("audio/wash_loop_1.ogg"));
        assert!(looks_like_file("raft.rhai"));
        assert!(looks_like_file("textures\\rain_drop.png"));
        assert!(!looks_like_file(".ogg"));
        assert!(!looks_like_file("audio/music_"));
        assert!(!looks_like_file("https://x.com/a.png"));
        assert!(!looks_like_file("Press any key.png now"));
        assert!(!looks_like_file("1.5"));
    }

    #[test]
    fn rhai_literals() {
        let src = r#"
            play_sound("audio/splash_" + n + ".ogg");
            let t = "textures/rain_drop.png"; // "c.png"
            draw_sprite("title_setdrift.png", 0.0);
            let s = "no newline
        "#;
        assert_eq!(
            rhai_refs(src),
            ["textures/rain_drop.png", "c.png", "title_setdrift.png"]
        );
    }

    fn pack(files: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        files
            .iter()
            .map(|(p, b)| (p.to_string(), b.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn references_resolve_like_the_runtime() {
        let files = pack(&[
            (
                "scenes/float.scene.toml",
                r#"
                [scene]
                name = "Float"
                [entities.raft.model]
                asset = "raft"
                [entities.raft.script]
                source = "raft.rhai"
                [entities.rain.audio_source]
                file = "audio/rain_loop.ogg"
                [entities.tree.model]
                asset = "tree"
                [entities.lost.model]
                asset = "whale"
                "#,
            ),
            ("models/raft.glb", ""),
            ("specs/tree.procgen.toml", ""),
            (
                "scripts/raft.rhai",
                r#"play_sound("audio/creak_1.ogg"); draw_sprite("logo.png")"#,
            ),
            ("audio/rain_loop.ogg", ""),
            ("audio/creak_1.ogg", ""),
        ]);
        assert_eq!(
            check_references(&files),
            [
                Missing {
                    from: "scenes/float.scene.toml".into(),
                    what: "whale.glb".into()
                },
                Missing {
                    from: "scripts/raft.rhai".into(),
                    what: "logo.png".into()
                },
            ]
        );
    }

    #[test]
    fn scenes_load_from_the_bundle_with_prefabs_and_archetypes() {
        let files = pack(&[
            ("schemas/archetypes/prop.toml", "[archetype.prop]\ncomponents = []\n"),
            (
                "levels/a.scene.toml",
                "[scene]\nname = \"a\"\n[entities.box]\narchetype = \"prop\"\n[entities.ghost]\narchetype = \"ghost\"\n",
            ),
            (
                "levels/b.scene.toml",
                "[scene]\nname = \"b\"\n[prefabs.k]\ntemplate = \"kart\"\nprefix = \"k\"\n",
            ),
        ]);
        let source: MemorySource = files.iter().map(|(p, b)| (p.as_str(), b.clone())).collect();
        let problems = check_scenes(
            Arc::new(source),
            &["levels/a.scene.toml".into(), "levels/b.scene.toml".into()],
            &["schemas".into()],
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].what.contains("\"ghost\""));
        assert_eq!(problems[1].from, "levels/b.scene.toml");
    }
}
