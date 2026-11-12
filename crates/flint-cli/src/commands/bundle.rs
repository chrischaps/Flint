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
//! scene, prefab, particle effect, glyph set or script names must be packed. A missing
//! file fails here rather than as a pink texture deep in play.
//!
//! The checks are `flint check`'s (#687): `crate::check` run over the files
//! about to be packed, as a `MemorySource`, with `Checks::BUNDLE`. Nothing
//! here checks anything; [`check_packed`] is the one call.
//!
//! `bundle.toml` in the game root holds the packing rules (every key
//! optional). What the project already says in its manifest it need not
//! say again (ADR 0114, decision 1): `name`, `scene` and `schemas` default
//! to `flint.toml`'s `[project] id`, `scene` and `schemas`. The manifest is
//! packed like any file, so the player finds the same project in the
//! bundle that a command finds on the disk.
//!
//! ```toml
//! name = "float"                           # else flint.toml's id, else the folder's name
//! scene = "scenes/float.scene.toml"        # else flint.toml's scene, else the only scene
//! schemas = ["engine/schemas", "schemas"]  # else flint.toml's, else these two
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
//!
//! Two more tables (#106):
//!
//! ```toml
//! [mount]                                  # a folder outside the game, packed at a path inside it
//! "engine/schemas" = "../schemas"          # (Flint's examples/, whose schemas are the repo's)
//! [web]                                    # what `flint export web` puts on the page
//! title = "Flint demos"
//! line = "One sentence under the title."
//! ```
//!
//! A game with no `engine/schemas` of its own and no mount for it packs the
//! engine's schemas from the Flint tree this CLI was built from, so a scene
//! an agent wrote in an empty folder packs as it plays.
//!
//! **One scene** (#382): given a `.scene.toml` instead of a folder, the
//! packer takes that scene and only what it reaches, from its project
//! (`flint.toml`'s root; in the engine checkout, the checkout):
//!
//! ```bash
//! flint bundle examples/ascii_entities.scene.toml   # examples/dist/ascii_entities.flintpack
//! ```
//!
//! See [`gather_scene`] for what "reaches" means. The bundle's paths are the
//! project's, so the scene plays from the bundle as it does from the disk.

use crate::check::{self, Checks, Schemas, Subject};
use anyhow::{bail, Context, Result};
use clap::Args;
use flint_core::bundle::{Bundle, BundleManifest, LateBundle, EXTENSION};
use flint_core::project::{AssetKind, Project};
use flint_core::vfs::{self, normalize, MemorySource};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Args)]
pub struct BundleArgs {
    /// The game's root folder, or one `.scene.toml` to pack alone with the
    /// files it reaches
    #[arg(default_value = ".")]
    pub game: PathBuf,

    /// Where to write the bundles (default: `dist/` in the game root, or
    /// beside a scene packed alone)
    #[arg(long)]
    pub out_dir: Option<PathBuf>,

    /// The scene the player starts (overrides bundle.toml, which overrides
    /// flint.toml's `[project] scene`)
    #[arg(long)]
    pub scene: Option<String>,

    /// The bundle's name (overrides bundle.toml, which overrides
    /// flint.toml's `[project] id`; default: the folder's name, or a scene
    /// packed alone's file stem)
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

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleConfig {
    pub name: Option<String>,
    scene: Option<String>,
    schemas: Option<Vec<String>>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    include: Vec<String>,
    late: Option<BTreeMap<String, Vec<String>>>,
    /// Bundle path → a folder relative to the game root, packed there.
    #[serde(default)]
    mount: BTreeMap<String, String>,
    /// The page's words and film, for `flint export web`.
    #[serde(default)]
    pub web: WebMeta,
    /// `[late]`'s keys in the order the file lists them (a `BTreeMap`
    /// forgets it).
    #[serde(skip)]
    late_order: Vec<String>,
}

/// `bundle.toml`'s `[web]`: one game's entry in the page's `games.json`
/// (the chooser's card, the gate's words, the film for a browser without
/// WebGPU). `tools/play-site/games.toml` documents each key.
#[derive(Debug, Default, Clone, Deserialize, serde::Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controls: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub film: Option<String>,
}

impl WebMeta {
    /// The keys, in the order `games.json` lists them after `id`.
    pub const KEYS: &'static [&'static str] = &[
        "title",
        "line",
        "controls",
        "touch",
        "orientation",
        "page",
        "video",
        "poster",
        "film",
    ];

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Option<String>> {
        Some(match key {
            "title" => &mut self.title,
            "line" => &mut self.line,
            "controls" => &mut self.controls,
            "touch" => &mut self.touch,
            "orientation" => &mut self.orientation,
            "page" => &mut self.page,
            "video" => &mut self.video,
            "poster" => &mut self.poster,
            "film" => &mut self.film,
            _ => return None,
        })
    }
}

/// The engine's own schemas, in the Flint tree this CLI was built from.
fn engine_schemas() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    dir.join("components").is_dir().then_some(dir)
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

/// Every file the rules keep, as its bundle path and the file on disk:
/// those under `root`, then each mount's folder under its bundle path. A
/// mounted file wins over one the game holds at the same path.
fn collect(
    root: &Path,
    rules: &Rules,
    mounts: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut out: BTreeMap<String, PathBuf> = collect_under(root, rules)?
        .into_iter()
        .map(|key| {
            let path = root.join(&key);
            (key, path)
        })
        .collect();
    for (at, dir) in mounts {
        if !dir.is_dir() {
            bail!("[mount] \"{at}\" = {}: not a folder", dir.display());
        }
        let at = normalize(Path::new(at));
        // A mount replaces what the game has there.
        out.retain(|key, _| !key.starts_with(&format!("{at}/")));
        let mut stack = vec![PathBuf::new()];
        while let Some(rel) = stack.pop() {
            for entry in std::fs::read_dir(dir.join(&rel))
                .with_context(|| format!("reading {}", dir.join(&rel).display()))?
            {
                let entry = entry?;
                let rel_path = rel.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    stack.push(rel_path);
                    continue;
                }
                let key = normalize(&Path::new(&at).join(&rel_path));
                // The excludes still hold (a README, a dotfile), but a
                // mount needs no include to be packed: it was asked for.
                let any = |list: &[String]| list.iter().any(|p| glob_match(p, &key));
                let excluded_by_name = rules
                    .exclude
                    .iter()
                    .filter(|p| p.starts_with("**/"))
                    .any(|p| glob_match(p, &key));
                if any(&rules.include) || !excluded_by_name {
                    out.insert(key, dir.join(&rel_path));
                }
            }
        }
    }
    Ok(out)
}

/// Every file under `root`, as normalized paths relative to it, that the
/// rules keep. Excluded directories are not descended into unless an
/// include could reach inside them.
fn collect_under(root: &Path, rules: &Rules) -> Result<Vec<String>> {
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

/// What `flint check` finds in the files about to be packed, read as the
/// player will read them: every scene loads from the bundle with its
/// prefabs and its archetypes known, every file and asset a scene, prefab,
/// particle effect, glyph set, chunk, user style or script names is packed,
/// and the user styles and looks load ([`Checks::BUNDLE`]).
pub fn check_packed(files: &BTreeMap<String, Vec<u8>>, schemas: &[String]) -> check::Report {
    let source: MemorySource = files.iter().map(|(p, b)| (p.as_str(), b.clone())).collect();
    let subject = Subject::packed(
        files.keys().cloned().collect(),
        Schemas::Listed(schemas.to_vec()),
    );
    vfs::scoped(Arc::new(source), || check::run(&subject, Checks::BUNDLE))
}

/// The files of the project at `root` that `flint check` reads: what the
/// default rules would pack (no `target/`, `dist/`, dotfiles, docs or
/// engine subtree), whatever `bundle.toml` leaves out of the bundle. A file
/// kept off the web is still part of the game on the disk.
pub fn project_files(root: &Path) -> Result<Vec<String>> {
    collect_under(root, &Rules::from_config(&BundleConfig::default()))
}

fn mb(bytes: u64) -> String {
    format!("{:.2} MB", bytes as f64 / 1_000_000.0)
}

/// The game's `bundle.toml`, or every default without one.
pub fn read_config(root: &Path) -> Result<BundleConfig> {
    let config_path = root.join("bundle.toml");
    if config_path.is_file() {
        let text = std::fs::read_to_string(&config_path)?;
        BundleConfig::parse(&text).with_context(|| format!("reading {}", config_path.display()))
    } else {
        Ok(BundleConfig::default())
    }
}

/// The project manifest of the game at `root` (`flint.toml`, ADR 0114):
/// what `bundle.toml` may leave unsaid. A game with no manifest has none of
/// it; a manifest that doesn't read is an error naming the file and key.
pub fn read_project(root: &Path) -> Result<flint_core::project::Project> {
    flint_core::project::Project::at(root)
        .map_err(|e| anyhow::anyhow!("reading the project manifest: {e}"))
}

/// What a pack wrote (or, in a dry run, would have).
pub struct Packed {
    /// The scene the player starts.
    pub scene: String,
    /// The files, in the order written: the main bundle first.
    pub files: Vec<(String, u64)>,
    /// Where they went.
    pub out_dir: PathBuf,
}

pub fn run(args: BundleArgs) -> Result<()> {
    let dry_run = args.dry_run;
    let packed = pack(args)?;
    if !dry_run {
        println!("written to {}", packed.out_dir.display());
    }
    Ok(())
}

/// What goes in a bundle, before it is checked and written.
struct Gathered {
    name: String,
    scene: String,
    schemas: Vec<String>,
    files: BTreeMap<String, Vec<u8>>,
    rules: Rules,
    out_dir: PathBuf,
    /// Files that stay in the main bundle whatever `[late]` says.
    main_only: BTreeSet<String>,
}

pub fn pack(args: BundleArgs) -> Result<Packed> {
    let gathered = if is_scene_file(&args.game) {
        gather_scene(&args)?
    } else {
        gather_game(&args)?
    };
    write(gathered, args.list, args.dry_run)
}

/// Whether `flint bundle` was handed one scene rather than a game folder.
fn is_scene_file(path: &Path) -> bool {
    path.is_file() && normalize(path).ends_with(".scene.toml")
}

fn gather_game(args: &BundleArgs) -> Result<Gathered> {
    let root = args.game.as_path();
    if !root.is_dir() {
        bail!("{} is not a folder or a .scene.toml", root.display());
    }
    let config = read_config(root)?;
    let rules = Rules::from_config(&config);
    // flint.toml says what bundle.toml leaves out (ADR 0114, decision 1)
    let project = read_project(root)?;

    let name = args
        .name
        .clone()
        .or(config.name.clone())
        .or(project.id.clone())
        .or_else(|| {
            std::fs::canonicalize(root)
                .ok()?
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
        })
        .unwrap_or_else(|| "game".into());

    let mut mounts: BTreeMap<String, PathBuf> = config
        .mount
        .iter()
        .map(|(at, from)| (normalize(Path::new(at)), root.join(from)))
        .collect();
    if !mounts.contains_key("engine/schemas") && !root.join("engine/schemas").is_dir() {
        if let Some(dir) = engine_schemas() {
            println!(
                "no engine/schemas in the game: packing the engine's, from {}",
                dir.display()
            );
            mounts.insert("engine/schemas".into(), dir);
        }
    }

    let paths = collect(root, &rules, &mounts)?;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (path, from) in &paths {
        let bytes = std::fs::read(from).with_context(|| format!("reading {}", from.display()))?;
        files.insert(path.clone(), bytes);
    }

    let scenes: Vec<String> = files
        .keys()
        .filter(|p| p.ends_with(".scene.toml"))
        .cloned()
        .collect();
    let manifest_scene = project
        .scene
        .as_ref()
        .map(|s| s.to_string_lossy().into_owned());
    let scene = match args
        .scene
        .clone()
        .or(config.scene.clone())
        .or(manifest_scene)
    {
        Some(s) => normalize(Path::new(&s)),
        None if scenes.len() == 1 => scenes[0].clone(),
        None => bail!(
            "{} scenes are packed; name the one to start with --scene, `scene =` in \
             bundle.toml or `[project] scene` in flint.toml:\n  {}",
            scenes.len(),
            scenes.join("\n  ")
        ),
    };
    if !files.contains_key(&scene) {
        bail!("the start scene {scene} is not packed");
    }
    // bundle.toml's list, else the project manifest's (ADR 0114), else the
    // game-repo layout; as bundle paths, and only those that were packed
    let manifest_schemas: Option<Vec<String>> = (!project.schemas.is_empty()).then(|| {
        project
            .schemas
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    });
    let schemas: Vec<String> = match config.schemas.clone() {
        Some(listed) => listed,
        None => manifest_schemas.unwrap_or_else(|| vec!["engine/schemas".into(), "schemas".into()]),
    }
    .into_iter()
    .map(|s| normalize(Path::new(&s)))
    .filter(|s| files.keys().any(|p| p.starts_with(&format!("{s}/"))))
    .collect();

    Ok(Gathered {
        name,
        scene,
        schemas,
        files,
        rules,
        out_dir: args.out_dir.clone().unwrap_or_else(|| root.join("dist")),
        main_only: BTreeSet::new(),
    })
}

/// The kinds a scene names one file at a time, and the bundle's check
/// reports by name when the file isn't packed, with every path tried: a
/// model, a texture, a sound, a script. A scene packed alone takes these
/// only when it reaches them. Every other kind is small and is found by a
/// bare name the check doesn't follow (a palette, a look, a particle
/// effect: `particle_effect.effect` is a plain string) or loaded whole
/// (animations, user styles, specs), so it goes in as the scene's search
/// sees it. A script's `import`s are not checked by a bundle either: the
/// `scripts/lib/` modules go in the same way.
const BY_REFERENCE: &[AssetKind] = &[
    AssetKind::Models,
    AssetKind::Textures,
    AssetKind::Sprites,
    AssetKind::Audio,
    AssetKind::Scripts,
    AssetKind::Scenes,
    AssetKind::Files,
    AssetKind::ParticleTextures,
    // A missing prefab is a `bad-prefab` with no paths tried: a packed
    // scene's `[prefabs]` are resolved here (`prefabs_of`)
    AssetKind::Prefabs,
];

/// How many times the reach is widened before giving up: each round adds
/// what the last round's files name, so this is the depth of the chain
/// (scene -> script -> the scene it loads -> its models ...).
const MAX_ROUNDS: usize = 64;

/// One scene and what it reaches, from its project (#382):
///
/// 1. the scene, the project's `flint.toml` and input config, and its
///    schema directories (a directory outside the project goes in at
///    `engine/schemas`, as a game's subtree would hold it);
/// 2. every kind not [`BY_REFERENCE`], as the scene's search sees it: the
///    kind's folders whole at each step of the walk (a glyph set's `.txt`,
///    a style's `luts/`), or only the kind's own files in a folder a
///    by-reference kind shares (`specs` in `models/`);
/// 3. then, until nothing changes, the bundle's own check is run over what
///    is gathered, and each missing file it reports is added from the first
///    path it tried that is on the disk. The check is the player's search
///    (`flint check`'s, #687), so what it says is missing is what the
///    player would read; a scene, script or prefab added this way is
///    checked in the next round, so the chain is followed to its end.
///
/// `bundle.toml` in the project root still applies: its `exclude` keeps a
/// file out even when reached, and `[late]` splits the parts.
fn gather_scene(args: &BundleArgs) -> Result<Gathered> {
    let scene_path = args.game.as_path();
    if args.scene.is_some() {
        bail!(
            "--scene names a game's start scene; {} is the scene already",
            scene_path.display()
        );
    }
    let project = Project::for_path(scene_path)
        .map_err(|e| anyhow::anyhow!("reading the project manifest: {e}"))?;
    let root = project.root.clone();
    let scene = key_under(scene_path, &root).with_context(|| {
        format!(
            "{} is not under its project root {}",
            scene_path.display(),
            root.display()
        )
    })?;
    let config = read_config(&root)?;
    let rules = Rules::from_config(&config);
    let user_excluded = |key: &str| config.exclude.iter().any(|p| glob_match(p, key));

    let name = args.name.clone().unwrap_or_else(|| {
        scene
            .rsplit('/')
            .next()
            .unwrap_or(&scene)
            .trim_end_matches(".scene.toml")
            .to_lowercase()
    });

    // Bundle path -> the file on the disk
    let mut paths: BTreeMap<String, PathBuf> = BTreeMap::new();
    paths.insert(scene.clone(), scene_path.to_path_buf());

    // The schemas the scene resolves by, at their place in the project
    let dirs = flint_core::project::schema_dirs(Some(scene_path), &[] as &[&str])
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .dirs;
    let mut mounts: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut schemas = Vec::new();
    for dir in &dirs {
        let at = match key_under(dir, &root) {
            Some(key) => key,
            None => {
                // A game naming the engine's schemas by path: where its
                // bundle.toml mounts them, else where a subtree would be
                let mounted = config.mount.iter().find_map(|(at, from)| {
                    same_dir(&root.join(from), dir).then(|| normalize(Path::new(at)))
                });
                let at = mounted.unwrap_or_else(|| match mounts.contains_key("engine/schemas") {
                    false => "engine/schemas".into(),
                    true => format!("external/{}", mounts.len()),
                });
                mounts.insert(at.clone(), dir.clone());
                at
            }
        };
        // All of it but notes and dotfiles: the directory was asked for
        for rel in files_under(dir)? {
            if !rel.split('/').any(|s| s.starts_with('.')) && !rel.ends_with(".md") {
                paths.insert(join_key(&at, &rel), dir.join(&rel));
            }
        }
        schemas.push(at);
    }
    if dirs.is_empty() {
        if let Some(dir) = engine_schemas() {
            println!(
                "{scene} resolves no schemas: packing the engine's, from {}",
                dir.display()
            );
            for rel in files_under(&dir)? {
                paths.insert(join_key("engine/schemas", &rel), dir.join(&rel));
            }
            mounts.insert("engine/schemas".into(), dir);
            schemas.push("engine/schemas".into());
        }
    }

    // Where a bundle path is on the disk: under a mount, else the project
    let disk = |key: &str| -> PathBuf {
        mounts
            .iter()
            .filter_map(|(at, dir)| {
                key.strip_prefix(&format!("{at}/"))
                    .map(|rest| (at.len(), dir.join(rest)))
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, path)| path)
            .unwrap_or_else(|| root.join(key))
    };
    let add = |paths: &mut BTreeMap<String, PathBuf>, key: String, path: PathBuf| {
        if !user_excluded(&key) && !paths.contains_key(&key) {
            paths.insert(key, path);
            true
        } else {
            false
        }
    };

    // The project's own files the player opens with no scene naming them
    let scene_dir = scene_path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut fixed: Vec<PathBuf> = vec![project.manifest_path()];
    fixed.extend(project.input_path());
    fixed.extend(
        project
            .resolve(AssetKind::Files, "config/input.toml", scene_dir)
            .ok(),
    );
    for path in fixed.into_iter().filter(|p| p.is_file()) {
        if let Some(key) = key_under(&path, &root) {
            add(&mut paths, key, path);
        }
    }

    // The small kinds, as the scene's search sees them
    let shared: BTreeSet<&str> = BY_REFERENCE
        .iter()
        .flat_map(|kind| kind.info().folders.iter().copied())
        .filter(|f| !f.is_empty())
        .collect();
    for kind in AssetKind::ALL.iter().filter(|k| !BY_REFERENCE.contains(k)) {
        for listed in project.list(*kind, scene_dir) {
            if let Some(key) = key_under(&listed.path, &root) {
                add(&mut paths, key, listed.path);
            }
        }
        // `search_dirs` is each of the kind's folders under each step of
        // the walk, in that order. A folder of `""` is the step itself,
        // and a shared one holds media: both were listed above.
        let folders = kind.info().folders;
        let dirs = project.search_dirs(*kind, scene_dir);
        for (i, dir) in dirs.iter().enumerate() {
            let folder = folders[i % folders.len()];
            if folder.is_empty() || shared.contains(folder) || !dir.is_dir() {
                continue;
            }
            let Some(at) = key_under(dir, &root) else {
                continue;
            };
            for rel in files_under(dir)? {
                let key = join_key(&at, &rel);
                if rules.packs(&key) {
                    add(&mut paths, key, dir.join(&rel));
                }
            }
        }
    }

    // The modules a script may import (`import "m"` is `scripts/lib/m.rhai`)
    for dir in project.search_dirs(AssetKind::Scripts, scene_dir) {
        let lib = dir.join("lib");
        let Some(at) = key_under(&lib, &root).filter(|_| lib.is_dir()) else {
            continue;
        };
        for rel in files_under(&lib)? {
            let key = join_key(&at, &rel);
            if rules.packs(&key) {
                add(&mut paths, key, lib.join(&rel));
            }
        }
    }

    // Then what the scene names, round by round
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut reached = 0usize;
    for round in 0.. {
        for (key, path) in &paths {
            if !files.contains_key(key) {
                let bytes =
                    std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
                files.insert(key.clone(), bytes);
            }
        }
        if round == MAX_ROUNDS {
            break;
        }
        let mut grew = false;
        // The prefabs each packed scene is made from
        let scenes: Vec<String> = files
            .keys()
            .filter(|k| k.ends_with(".scene.toml"))
            .cloned()
            .collect();
        for key in scenes {
            for path in prefabs_of(&files[&key], &disk(&key)) {
                if let Some(prefab) = key_under(&path, &root) {
                    if add(&mut paths, prefab, path) {
                        reached += 1;
                        grew = true;
                    }
                }
            }
        }
        if grew {
            continue;
        }
        let report = check_packed(&files, &schemas);
        for d in report.diagnostics.iter().filter(|d| d.is_error()) {
            let found = d.tried.iter().find_map(|tried| {
                let key = normalize(tried);
                let path = disk(&key);
                (!key.starts_with("../") && path.is_file()).then_some((key, path))
            });
            if let Some((key, path)) = found {
                if add(&mut paths, key, path) {
                    reached += 1;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    // Only the directories that hold something, as a game's bundle lists them
    schemas.retain(|s| files.keys().any(|p| p.starts_with(&format!("{s}/"))));
    let bytes: usize = files.values().map(Vec::len).sum();
    println!(
        "{scene}: {} files from {} ({} reached by name, {})",
        files.len(),
        flint_core::project::shown(&root),
        reached,
        mb(bytes as u64)
    );

    // A music session checks its stems when the scene loads, before a late
    // part has arrived, and plays nothing if one is missing: a scene pack
    // has no bundle.toml to say so, so the stems stay in the main bundle
    let main_only = files
        .iter()
        .filter(|(key, _)| key.ends_with(".suite.toml"))
        .flat_map(|(_, bytes)| suite_stems(bytes))
        .filter(|key| files.contains_key(key))
        .collect();

    let out_dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| scene_dir.join("dist"));
    Ok(Gathered {
        name,
        scene,
        schemas,
        files,
        rules,
        out_dir,
        main_only,
    })
}

/// The stems a music suite's buses name (`[buses.<name>] file`), as bundle
/// paths: a suite's paths are from the project root.
fn suite_stems(text: &[u8]) -> Vec<String> {
    let Ok(doc) = String::from_utf8_lossy(text).parse::<toml::Value>() else {
        return Vec::new();
    };
    let buses = doc.get("buses").and_then(|b| b.as_table());
    buses
        .into_iter()
        .flat_map(|buses| buses.values())
        .filter_map(|bus| bus.get("file")?.as_str())
        .map(|file| normalize(Path::new(file)))
        .collect()
}

/// The prefab files a scene's `[prefabs]` name, as the loader finds them
/// from the scene's place on the disk. One that isn't found is left for
/// the bundle's check to report.
fn prefabs_of(text: &[u8], scene: &Path) -> Vec<PathBuf> {
    let Ok(file) = flint_scene::SceneFile::parse(&String::from_utf8_lossy(text)) else {
        return Vec::new();
    };
    let project = Project::for_scene(scene);
    let dir = scene.parent().unwrap_or(Path::new("."));
    let templates: BTreeSet<&str> = file.prefabs.values().map(|p| p.template.as_str()).collect();
    templates
        .into_iter()
        .filter_map(|t| project.resolve(AssetKind::Prefabs, t, dir).ok())
        .collect()
}

/// Every file under `dir`, as `/`-separated paths relative to it, sorted.
fn files_under(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let here = dir.join(&rel);
        for entry in
            std::fs::read_dir(&here).with_context(|| format!("reading {}", here.display()))?
        {
            let entry = entry?;
            let rel_path = rel.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                stack.push(rel_path);
            } else {
                out.push(normalize(&rel_path));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn join_key(at: &str, rel: &str) -> String {
    match at.is_empty() {
        true => rel.to_string(),
        false => format!("{at}/{rel}"),
    }
}

/// `path` as a bundle path under `root` (`/`-separated, no `..`), if it is
/// under it. The two may be spelled differently (one relative, one
/// absolute), so they are compared as absolute paths.
fn key_under(path: &Path, root: &Path) -> Option<String> {
    let rel = match path.strip_prefix(root) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => {
            let absolute = |p: &Path| std::path::absolute(p).ok();
            absolute(path)?
                .strip_prefix(absolute(root)?)
                .ok()?
                .to_path_buf()
        }
    };
    let key = normalize(&rel);
    (!key.is_empty() && !key.split('/').any(|s| s == "..")).then_some(key)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Check the gathered files as the player will read them, then split them
/// into the main bundle and its late parts and write each.
fn write(gathered: Gathered, list: bool, dry_run: bool) -> Result<Packed> {
    let Gathered {
        name,
        scene,
        schemas,
        files,
        rules,
        out_dir,
        main_only,
    } = gathered;
    // Check before writing: a bundle with a hole in it fails here. The
    // checks are `flint check`'s, over the packed files (#687).
    let report = check_packed(&files, &schemas);
    for warning in report.diagnostics.iter().filter(|d| !d.is_error()) {
        eprintln!("warning: {}", warning.one_line());
    }
    if report.has_errors() {
        let errors: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .cloned()
            .collect();
        let mut msg = format!(
            "{} unresolved reference(s); nothing written:\n",
            errors.len()
        );
        msg.push_str(&check::text(&check::Report {
            diagnostics: errors,
            ..report
        }));
        msg.push_str("Pack the missing files, or leave the file that names them out with `exclude` in bundle.toml.");
        bail!(msg);
    }

    // Split into the main bundle and the late parts.
    let mut parts: BTreeMap<Option<String>, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    parts.entry(None).or_default();
    for (path, bytes) in files {
        let part = match main_only.contains(&path) {
            true => None,
            false => rules.part_of(&path).map(str::to_string),
        };
        parts.entry(part).or_default().push((path, bytes));
    }

    if list {
        for (part, list) in &parts {
            let label = part.as_deref().unwrap_or("main");
            for (path, bytes) in list {
                println!("{:>10}  {label:<6}  {path}", bytes.len());
            }
        }
        println!();
    }

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
    let packed = Packed {
        scene,
        files: written
            .iter()
            .map(|(file, _, bytes)| (file.clone(), bytes.len() as u64))
            .collect(),
        out_dir: out_dir.clone(),
    };
    if dry_run {
        println!("(dry run: nothing written)");
        return Ok(packed);
    }
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    for (file, _, bytes) in &written {
        std::fs::write(out_dir.join(file), bytes)?;
    }
    Ok(packed)
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

    fn pack(files: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        files
            .iter()
            .map(|(p, b)| (p.to_string(), b.as_bytes().to_vec()))
            .collect()
    }

    /// What the bundle's check says of `files`: `(file, code, message)`.
    fn problems(
        files: &BTreeMap<String, Vec<u8>>,
        schemas: &[&str],
    ) -> Vec<(String, &'static str, String)> {
        let schemas: Vec<String> = schemas.iter().map(|s| s.to_string()).collect();
        check_packed(files, &schemas)
            .diagnostics
            .into_iter()
            .filter(|d| d.is_error())
            .map(|d| {
                let file = d.file.as_ref().map(|f| f.to_string_lossy().into_owned());
                (file.unwrap_or_default(), d.code, d.message)
            })
            .collect()
    }

    /// The reference checks are `flint check`'s (`check::references` has
    /// their own tests): a scene's assets by kind, a script's by literal.
    #[test]
    fn references_resolve_like_the_runtime() {
        let found = problems(
            &pack(&[
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
                (
                    "specs/tree.procgen.toml",
                    "generator = \"tree_v1\"\n[meta]\nname = \"tree\"\nversion = \"1.0.0\"\n\
                     [seed]\nmode = \"fixed\"\nvalue = 7\n[params]\n",
                ),
                (
                    "scripts/raft.rhai",
                    r#"play_sound("audio/creak_1.ogg"); draw_sprite("logo.png")"#,
                ),
                ("audio/rain_loop.ogg", ""),
                ("audio/creak_1.ogg", ""),
            ]),
            &[],
        );
        let said: Vec<(&str, &str)> = found.iter().map(|(f, c, _)| (f.as_str(), *c)).collect();
        assert_eq!(
            said,
            [
                ("scenes/float.scene.toml", "missing-asset"),
                ("scripts/raft.rhai", "missing-file"),
            ],
            "{found:?}"
        );
        assert!(found[0].2.contains("`whale`"), "{found:?}");
        assert!(found[1].2.contains("`logo.png`"), "{found:?}");
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
        let problems = problems(&files, &["schemas"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(problems[0].1, "unknown-archetype");
        assert!(problems[0].2.contains("`ghost`"));
        assert_eq!(problems[1].0, "levels/b.scene.toml");
        assert_eq!(problems[1].1, "bad-prefab");
    }

    /// #356: a scene's look is found in the packed files or among the
    /// built-ins, or the bundle says so.
    #[test]
    fn a_scenes_look_is_checked_from_the_bundle() {
        let files = pack(&[
            (
                "styles/dusk.look.toml",
                "[look]\ndescription = \"d\"\n[post_process]\nexposure = 0.6\n",
            ),
            (
                "levels/a.scene.toml",
                "[scene]\nname = \"a\"\n[post_process]\nlook = \"dusk\"\n",
            ),
            (
                "levels/b.scene.toml",
                "[scene]\nname = \"b\"\n[post_process]\nlook = \"graphite\"\n",
            ),
            (
                "levels/c.scene.toml",
                "[scene]\nname = \"c\"\n[post_process]\nlook = \"gone\"\n",
            ),
        ]);
        let problems = problems(&files, &["schemas"]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].0, "levels/c.scene.toml");
        assert_eq!(problems[0].1, "bad-look");
        assert!(problems[0].2.contains("look = \"gone\""), "{problems:?}");
    }

    /// #275: a user style must pack its shader, and the shader must
    /// compile, since the web player would only draw a passthrough.
    #[test]
    fn user_styles_are_checked_from_the_bundle() {
        let files = pack(&[
            (
                "styles/bundle_ok.shader.toml",
                "version = 1\nname = \"bundle_ok\"\nshader = \"ok.wgsl\"\n",
            ),
            (
                "styles/ok.wgsl",
                "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color;\n}\n",
            ),
            (
                "styles/bundle_broken.shader.toml",
                "version = 1\nname = \"bundle_broken\"\nshader = \"broken.wgsl\"\n",
            ),
            (
                "styles/broken.wgsl",
                "fn style(p: StylePixel) -> vec4<f32> {\n    return p.colour;\n}\n",
            ),
            (
                "styles/bundle_unpacked.shader.toml",
                "version = 1\nname = \"bundle_unpacked\"\nshader = \"gone.wgsl\"\n",
            ),
            ("levels/a.scene.toml", "[scene]\nname = \"a\"\n"),
        ]);
        let problems = problems(&files, &[]);
        // The description names its shader: a missing one is a reference
        let references: Vec<_> = problems.iter().filter(|p| p.1 == "missing-file").collect();
        assert_eq!(references.len(), 1, "{problems:?}");
        assert_eq!(references[0].0, "styles/bundle_unpacked.shader.toml");
        assert!(references[0].2.contains("`gone.wgsl`"), "{problems:?}");
        // ...and loading the styles from the bundle says what else is wrong
        let styles: Vec<_> = problems
            .iter()
            .filter(|p| p.1 == "bad-user-style")
            .collect();
        assert_eq!(styles.len(), 2, "{problems:?}");
        assert!(
            styles.iter().any(|p| p.0 == "styles/broken.wgsl"
                && p.2.contains("line 2, ")
                && p.2.contains("colour")),
            "{problems:?}"
        );
        assert!(
            styles.iter().any(|p| p.0.contains("gone.wgsl")),
            "{problems:?}"
        );
    }

    /// #413: an image a user style reads must be packed: its `file` is a
    /// reference, and loading from the bundle reports it missing.
    #[test]
    fn user_style_images_are_checked_from_the_bundle() {
        let files = pack(&[
            (
                "styles/bundle_paper.shader.toml",
                "version = 1\nname = \"bundle_paper\"\nshader = \"paper.wgsl\"\n\
                 [images.paper]\nfile = \"papers/gone.png\"\n\
                 [options.grade]\nkind = \"image\"\ndefault = \"luts/a.png\"\n",
            ),
            (
                "styles/paper.wgsl",
                "fn style(p: StylePixel) -> vec4<f32> {\n    \
                 return flint_image_paper(p.uv) * vec4<f32>(flint_lut_grade(p.color.rgb), 1.0);\n}\n",
            ),
            ("styles/luts/a.png", "png bytes"),
            ("levels/a.scene.toml", "[scene]\nname = \"a\"\n"),
        ]);
        let problems = problems(&files, &[]);
        let styles: Vec<_> = problems
            .iter()
            .filter(|p| p.1 == "bad-user-style")
            .collect();
        assert_eq!(styles.len(), 1, "{problems:?}");
        assert!(
            styles[0].0.ends_with("papers/gone.png") && styles[0].2.contains("image not found"),
            "{problems:?}"
        );
    }
}
