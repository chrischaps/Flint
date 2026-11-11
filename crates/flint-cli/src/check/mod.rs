//! The pre-flight (#687, ADR 0115 step 5): one pass over a project, a scene
//! or a packed bundle that reports everything wrong with it as
//! [`Diagnostic`]s, never stopping at the first.
//!
//! Three commands call it, and none has a copy of a check:
//!
//! - `flint check <dir|scene>` runs all of it ([`Checks::ALL`]) over the
//!   disk and prints the list as text or JSON;
//! - `flint bundle` runs [`Checks::BUNDLE`] over the files it is about to
//!   pack, as a `MemorySource`, and writes nothing if there is an error;
//! - `flint validate <scene>` runs [`Checks::VALIDATE`] over one scene
//!   ([`scene`]) and adds what it alone does: the constraint report with
//!   `--fix` and `--dry-run`.
//!
//! Everything is read through `flint_core::vfs`, which is what lets one
//! implementation serve the disk and a bundle. It lives in the CLI crate
//! because it needs most of the engine at once: `flint-scene`'s diagnostics,
//! `flint-constraint`, `flint-render`'s style and look loaders (the shader
//! compiler included), `flint-procgen`'s specs and `flint-player`'s species
//! check. Nothing below the CLI depends on all of those.
//!
//! What each check is, and which are new with `flint check`:
//!
//! | Check | From | Codes |
//! |---|---|---|
//! | the scene reads, parses, expands its prefabs | `flint_scene::diagnose` (#683) | `file-unreadable` `toml-parse` `bad-prefab` |
//! | archetypes, components and fields fit the schemas | the same | `unknown-archetype` `unknown-component` `unknown-field` `bad-type` `bad-enum-value` `out-of-range` `missing-field` `no-schemas` |
//! | entity and asset references (every `parent`, every field a schema types `entity` or `asset`; an `ik_two_bone.tip` that is not a child) | `flint_scene::diagnose` (#684; the tip, #687) | `unknown-entity` `missing-asset` `not-a-child` |
//! | the scene builds | new | `scene-load` |
//! | the assets a scene names by kind, through the resolver | new ([`scene::ASSET_FIELDS`]) | `missing-asset` |
//! | the procgen spec behind a model or texture parses | new | `bad-spec` |
//! | the input config is there and parses | new | `missing-asset` `bad-input-config` |
//! | every other file a scene, prefab, effect, glyph set, chunk, style or script names | `flint bundle` | `missing-file` `toml-parse` |
//! | user styles load and compile | `flint bundle` | `bad-user-style` |
//! | `[post_process.styles]`, the look | `flint validate`, `flint bundle` | `bad-style-setting` `bad-look` |
//! | `[wooze]`, `[living.species]` | `flint validate` | `bad-wooze` `bad-species` |
//! | constraints, and that each can be evaluated | `flint validate`; the second is new | `constraint` `bad-constraint` |
//! | the manifest reads, its schema directories exist, its scene is there | new | `bad-manifest` `bad-schemas` `missing-asset` |
//! | the scripts compile, and their calls, callbacks and imports fit the script API | `flint script check` (#694, [`scripts`]) | `script-syntax` `unknown-function` `wrong-arity` `bad-argument` `unknown-callback` `callback-arity` `missing-module` `deprecated-function` `mixed-numbers` |
//!
//! The scripts pass reads every `.rhai` a project holds (or that a scene
//! runs) and the modules they import; their string literals are also read
//! for the files they name ([`references`]).

mod references;
mod report;
pub mod scene;
pub mod scripts;

pub use report::{json, text};

use flint_constraint::ConstraintRegistry;
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::project::{shown, Project, SchemaDirs};
use flint_core::vfs::{self, normalize};
use flint_schema::SchemaRegistry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Which checks a run makes. The module table says what each one is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checks {
    /// Every component and field against the schemas, and the entity
    /// references. Off, a scene is asked only whether it loads and whether
    /// its archetypes are known.
    pub schema: bool,
    /// Files and assets: the scene's assets through the resolver, and every
    /// file named by path.
    pub files: bool,
    /// User styles, `[post_process.styles]` and the look.
    pub styles: bool,
    /// `[wooze]` and `[living.species]`.
    pub tables: bool,
    /// The constraints of the schema directories.
    pub constraints: bool,
    /// The scripts, against the script API (#694).
    pub scripts: bool,
}

impl Checks {
    /// Everything: `flint check`.
    pub const ALL: Checks = Checks {
        schema: true,
        files: true,
        styles: true,
        tables: true,
        constraints: true,
        scripts: true,
    };

    /// What makes a bundle a whole one: every scene loads from the packed
    /// files with its archetypes known, every file and asset named is
    /// packed, and the styles and looks load. A field of the wrong type
    /// plays (with a warning) and is `flint check`'s to report.
    pub const BUNDLE: Checks = Checks {
        schema: false,
        files: true,
        styles: true,
        tables: false,
        constraints: false,
        scripts: false,
    };

    /// What `flint validate` has always asked of a scene, less the
    /// constraints, which it evaluates itself (it can fix them).
    pub const VALIDATE: Checks = Checks {
        schema: true,
        files: false,
        styles: true,
        tables: true,
        constraints: false,
        scripts: true,
    };

    /// The scripts alone: `flint script check`.
    pub const SCRIPTS: Checks = Checks {
        schema: false,
        files: false,
        styles: false,
        tables: false,
        constraints: false,
        scripts: true,
    };

    /// Whether anything is asked of the scenes themselves.
    fn scene_pass(self) -> bool {
        self.schema || self.files || self.styles || self.tables || self.constraints
    }
}

/// Where a run's schemas come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schemas {
    /// The one resolver (ADR 0114), for each scene: these `--schemas` flags
    /// if there are any, else the project manifest's list, else discovery.
    Resolve(Vec<String>),
    /// A list the host settled (a bundle's): those directories, plus a
    /// `schemas/` beside the scene, as the player reads them.
    Listed(Vec<String>),
}

/// The schemas one scene is checked against, and the constraints of the
/// same directories.
pub struct SceneSchemas {
    pub registry: SchemaRegistry,
    pub dirs: SchemaDirs,
    pub constraints: ConstraintRegistry,
    /// What is wrong with the schema files themselves: a game's schema
    /// lints (#684: an old type spelling, an unknown type, a key the loader
    /// ignores; warnings, and never about an engine checkout's files), then
    /// a constraint file that doesn't read or a query that doesn't parse
    /// (`bad-constraint`).
    pub problems: Vec<Diagnostic>,
}

impl SceneSchemas {
    /// The schemas the one resolver names for `scene` (or for the working
    /// directory), with their constraints.
    pub fn resolve(scene: Option<&Path>, flags: &[String]) -> flint_core::Result<Self> {
        let (registry, dirs) = SchemaRegistry::resolve(scene, flags)?;
        Ok(Self::with_constraints(registry, dirs))
    }

    /// The schemas of a list the host settled, for `scene`.
    pub fn listed(listed: &[String], scene: &Path) -> flint_core::Result<Self> {
        let dirs = flint_core::project::host_schema_dirs(listed, Some(scene));
        let registry = SchemaRegistry::load_from_directories(&dirs.dirs)?;
        Ok(Self::with_constraints(registry, dirs))
    }

    /// No schema at all: what a scene is checked against when its schemas
    /// could not be loaded (the reason is its own diagnostic).
    fn none() -> Self {
        Self {
            registry: SchemaRegistry::new(),
            dirs: SchemaDirs::default(),
            constraints: ConstraintRegistry::new(),
            problems: Vec::new(),
        }
    }

    fn with_constraints(registry: SchemaRegistry, dirs: SchemaDirs) -> Self {
        let mut constraints = ConstraintRegistry::new();
        let mut problems = registry.game_lints();
        for dir in &dirs.dirs {
            // Spelled from here, so a constraint's file reads as the
            // scene's does in a report
            let dir = &from_here(dir);
            match ConstraintRegistry::load_from_directory(dir) {
                Ok(loaded) => constraints.extend(loaded),
                // The message names the file
                Err(e) => problems.push(
                    Diagnostic::error(codes::BAD_CONSTRAINT, e.to_string())
                        .file(dir.join("constraints"))
                        .hint("no constraint of this directory was checked"),
                ),
            }
        }
        for bad in constraints.problems() {
            let d = Diagnostic::error(
                codes::BAD_CONSTRAINT,
                format!(
                    "constraint `{}`: `{}` can't be evaluated: {}",
                    bad.name, bad.query, bad.error
                ),
            )
            .field(bad.field)
            .hint(
                "a query is one resource and at most one condition: \
                 `entities where archetype == 'door'`; the constraint is not checked until \
                 it parses",
            );
            problems.push(match &bad.file {
                Some(file) => d.file(file),
                None => d,
            });
        }
        Self {
            registry,
            dirs,
            constraints,
            problems,
        }
    }
}

/// What a run looks at: a set of files under one root, and the scenes
/// among them. Paths are `/`-separated and relative to `root`, which is
/// empty for a bundle.
#[derive(Debug, Clone)]
pub struct Subject {
    pub root: PathBuf,
    /// The files read for the references they hold.
    pub files: Vec<String>,
    /// The scenes checked.
    pub scenes: Vec<String>,
    pub schemas: Schemas,
    /// `--strict`: a field no schema declares is an error.
    pub strict: bool,
    /// A single scene's check goes on to the scripts, prefabs and effects
    /// the scene names; a project's has them all in `files` already.
    pub follow: bool,
    /// What was wrong before a file was read: a manifest that doesn't.
    pub preliminary: Vec<Diagnostic>,
    /// What the scripts are held to: the engine's API when `None`, else
    /// with a game's functions added (`--allow`, `--api`).
    pub script_api: Option<flint_script::check::Api>,
}

impl Subject {
    /// The files of a bundle about to be written, read from the vfs.
    pub fn packed(files: Vec<String>, schemas: Schemas) -> Self {
        let scenes = files
            .iter()
            .filter(|p| p.ends_with(".scene.toml"))
            .cloned()
            .collect();
        Self {
            root: PathBuf::new(),
            files,
            scenes,
            schemas,
            strict: false,
            follow: false,
            preliminary: Vec::new(),
            script_api: None,
        }
    }

    /// Every scene under `dir` on the disk, and every file there that names
    /// others. `walk` lists `dir`'s files, relative to it: the caller's, so
    /// that what a project *is* (no `target/`, no `dist/`, no engine
    /// subtree) is said in one place, `flint bundle`'s default excludes.
    pub fn project(dir: &Path, files_under_dir: Vec<String>, flags: &[String]) -> Self {
        let (project, preliminary) = project_of(dir);
        // Paths are spelled from the project root, so the walk up from a
        // file reaches it; a folder outside its project is its own root.
        let (root, prefix) = match relative_to(dir, &project.root) {
            Some(prefix) => (project.root.clone(), prefix),
            None => (dir.to_path_buf(), String::new()),
        };
        let files: Vec<String> = files_under_dir
            .into_iter()
            .map(|file| match prefix.is_empty() {
                true => file,
                false => format!("{prefix}/{file}"),
            })
            .collect();
        let mut subject = Self::packed(files, Schemas::Resolve(flags.to_vec()));
        subject.root = from_here(&root);
        subject.preliminary = preliminary;
        // The manifest's own promises
        if let Some(scene) = project.scene_path() {
            if !vfs::is_file(&scene) {
                subject.preliminary.push(
                    Diagnostic::error(
                        codes::MISSING_ASSET,
                        format!("the start scene `{}` is not there", shown(&scene)),
                    )
                    .file(project.manifest_path())
                    .field("project.scene")
                    .tried([scene])
                    .hint("`flint play` with no scene opens this one"),
                );
            }
        }
        subject
    }

    /// One scene on the disk, and what it names.
    pub fn scene(scene: &Path, flags: &[String]) -> Self {
        let (project, preliminary) = project_of(scene);
        let dir = scene
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let (root, key) = match relative_to(scene, &project.root) {
            Some(key) => (project.root.clone(), key),
            None => (
                dir.to_path_buf(),
                scene
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
        };
        let mut subject = Self::packed(vec![key], Schemas::Resolve(flags.to_vec()));
        subject.root = from_here(&root);
        subject.follow = true;
        subject.preliminary = preliminary;
        subject
    }

    /// The path of one of the subject's files, as the vfs reads it.
    pub fn path(&self, key: &str) -> PathBuf {
        if self.root.as_os_str().is_empty() || self.root == Path::new(".") {
            PathBuf::from(key)
        } else {
            self.root.join(key)
        }
    }
}

/// `path` as it is spelled from the working directory when it is under it
/// (a project root is found as an absolute path), so that a report reads
/// `scenes/dock.scene.toml` and not the whole path to the checkout.
fn from_here(path: &Path) -> PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
}

/// The project `path` belongs to, or the one without its manifest and the
/// `bad-manifest` that says why.
fn project_of(path: &Path) -> (Project, Vec<Diagnostic>) {
    match Project::for_path(path) {
        Ok(project) => (project, Vec::new()),
        Err(e) => (
            Project::for_scene(path),
            vec![Diagnostic::error(codes::BAD_MANIFEST, e.to_string())
                .hint("the project is checked as if it had no manifest")],
        ),
    }
}

/// `path` as a `/`-separated path under `root`, if it is under it (`""` for
/// the root itself). The two may be spelled differently (one relative, one
/// absolute), so they are compared as absolute paths.
fn relative_to(path: &Path, root: &Path) -> Option<String> {
    if let Ok(rel) = path.strip_prefix(root) {
        return Some(normalize(rel));
    }
    let absolute = |p: &Path| std::path::absolute(p).ok();
    let (path, root) = (absolute(path)?, absolute(root)?);
    path.strip_prefix(&root).ok().map(normalize)
}

/// What a run found.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// The scenes checked, as they were read.
    pub scenes: Vec<String>,
    /// How many files were read for references.
    pub files: usize,
    /// The scripts the scripts pass read, modules included.
    pub scripts: Vec<String>,
    /// Everything wrong, in the order found, each once, with `/` in every
    /// path on every platform.
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        diagnostic::has_errors(&self.diagnostics)
    }

    /// `2 errors, 1 warning`, or `no problems`.
    pub fn counts(&self) -> String {
        diagnostic::counts_line(&self.diagnostics)
    }
}

/// Run `checks` over `subject`, through the vfs that is current: the disk,
/// or a bundle's files inside `vfs::scoped`.
pub fn run(subject: &Subject, checks: Checks) -> Report {
    let mut found = subject.preliminary.clone();
    let mut memo = scene::Memo::default();
    let mut loaded: BTreeMap<PathBuf, Rc<SceneSchemas>> = BTreeMap::new();
    let mut files: BTreeSet<String> = subject.files.iter().cloned().collect();
    let mut scenes = Vec::new();
    // A scene's scripts first, so each is checked with its scene's search
    let mut script_files: Vec<flint_script::check::ScriptFile> = Vec::new();

    for key in &subject.scenes {
        let path = subject.path(key);
        scenes.push(shown(&path));
        // Scenes of one folder share their schemas
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let schemas = match loaded.get(&dir) {
            Some(schemas) => schemas.clone(),
            None => {
                let schemas = match &subject.schemas {
                    Schemas::Resolve(flags) => SceneSchemas::resolve(Some(&path), flags),
                    Schemas::Listed(listed) => SceneSchemas::listed(listed, &path),
                };
                let schemas = Rc::new(schemas.unwrap_or_else(|e| {
                    found.push(
                        Diagnostic::error(codes::BAD_SCHEMAS, e.to_string())
                            .file(&path)
                            .hint("the scene is checked without schemas until they load"),
                    );
                    SceneSchemas::none()
                }));
                loaded.insert(dir, schemas.clone());
                schemas
            }
        };
        if checks.scripts {
            // A missing source is the asset pass's, when it runs
            let (of_scene, missing) = scripts::of_scene(&path, &schemas.registry, !checks.files);
            script_files.extend(of_scene);
            found.extend(missing);
        }
        if !checks.scene_pass() {
            continue;
        }
        let scene = scene::check(&path, &schemas, checks, subject.strict, &mut memo);
        found.extend(scene.diagnostics);
        if subject.follow {
            files.extend(
                scene
                    .reached
                    .iter()
                    .filter_map(|path| references::key_under(&subject.root, path))
                    .filter(|key| references::is_scanned(key) && !key.ends_with(".scene.toml")),
            );
        }
    }
    if checks.files {
        found.extend(references::check(subject, &files));
    }
    let mut scripts_read = Vec::new();
    if checks.scripts {
        // A project's every script, a scene's own
        if !subject.follow {
            script_files.extend(
                files
                    .iter()
                    .filter(|key| key.ends_with(".rhai"))
                    .map(|key| flint_script::check::ScriptFile::found(subject.path(key))),
            );
        }
        let api = match &subject.script_api {
            Some(api) => api.clone(),
            None => scripts::engine_api().clone(),
        };
        let (diagnostics, read) = scripts::check(&api, script_files);
        found.extend(diagnostics);
        scripts_read = read;
    }

    // Each problem once (scenes of a folder share their constraint files and
    // their styles), and every path with `/`
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for mut d in found {
        let spelled = |path: &Path| PathBuf::from(shown(&from_here(path)));
        d.file = d.file.map(|file| spelled(&file));
        d.tried = d.tried.iter().map(|path| spelled(path)).collect();
        if !diagnostics.contains(&d) {
            diagnostics.push(d);
        }
    }
    Report {
        scenes,
        files: files.iter().filter(|f| references::is_scanned(f)).count(),
        scripts: scripts_read,
        diagnostics,
    }
}

/// [`run`] for one scene with schemas already in hand, without the pass
/// over the files it names: what `flint validate` asks.
pub fn scene(
    scene: &Path,
    schemas: &SceneSchemas,
    checks: Checks,
    strict: bool,
    script_api: &flint_script::check::Api,
) -> Vec<Diagnostic> {
    let mut memo = scene::Memo::default();
    let mut found = scene::check(scene, schemas, checks, strict, &mut memo).diagnostics;
    if checks.scripts {
        let (files, missing) = scripts::of_scene(scene, &schemas.registry, !checks.files);
        found.extend(missing);
        found.extend(scripts::check(script_api, files).0);
    }
    // A constraint that can't be evaluated is the schemas' problem whether
    // or not the caller evaluates the rest itself
    for problem in &schemas.problems {
        if !found.contains(problem) {
            found.push(problem.clone());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::vfs::MemorySource;
    use std::sync::Arc;

    const FILES: &[(&str, &str)] = &[
        ("flint.toml", "[project]\nschemas = [\"schemas\"]\n"),
        (
            "schemas/archetypes/prop.toml",
            "[archetype.prop]\ncomponents = []\n",
        ),
        (
            "scenes/act1/dock.scene.toml",
            "[scene]\nname = \"Dock\"\n\n[post_process]\nlook = \"gone\"\n\n\
             [prefabs.one]\ntemplate = \"kart\"\nprefix = \"k\"\n\n\
             [entities.crate]\narchetype = \"porp\"\n\n\
             [entities.boat.model]\nasset = \"boat\"\n\n\
             [entities.sign.script]\nsource = \"sign.rhai\"\n\n\
             [entities.sign.material]\ntexture = \"textures/paint.png\"\n",
        ),
        (
            "scenes/torn.scene.toml",
            "[scene]\nname = \"Torn\"\n\n[entities.sail\n",
        ),
        (
            "prefabs/kart.prefab.toml",
            "[prefab]\nname = \"kart\"\n\n[entities.body.model]\nasset = \"kart_body\"\n",
        ),
        (
            "scripts/sign.rhai",
            "fn on_init() { play_sound(\"audio/creak.ogg\"); }\n",
        ),
        (
            "particles/smoke.particles.toml",
            "[effect]\nname = \"smoke\"\ntexture = \"puff.png\"\n",
        ),
    ];

    /// A diagnostic without its hint (which may name the scene by path),
    /// its paths spelled from `root`.
    fn told(d: &Diagnostic, root: &str) -> String {
        let under = |p: &Path| shown(p).trim_start_matches(root).to_string();
        format!(
            "{} {:?} {d} {:?}",
            d.file.as_deref().map(under).unwrap_or_default(),
            d.field,
            d.tried.iter().map(|p| under(p)).collect::<Vec<_>>()
        )
        .replace(root, "")
    }

    /// One implementation, two sources (#687): the files of a project give
    /// the same findings read from the disk as read from a bundle's
    /// `MemorySource`, down to the paths tried.
    #[test]
    fn the_disk_and_a_bundle_give_the_same_findings() {
        let dir = std::env::temp_dir().join(format!("flint-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut source = MemorySource::new();
        for (path, text) in FILES {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
            source.insert(*path, *text);
        }
        let keys: Vec<String> = FILES.iter().map(|(path, _)| path.to_string()).collect();

        let on_disk = run(&Subject::project(&dir, keys.clone(), &[]), Checks::BUNDLE);
        let packed = Subject::packed(keys, Schemas::Listed(vec!["schemas".to_string()]));
        let in_bundle = vfs::scoped(Arc::new(source), || run(&packed, Checks::BUNDLE));
        let _ = std::fs::remove_dir_all(&dir);

        let root = format!("{}/", shown(&dir));
        let disk: Vec<String> = on_disk.diagnostics.iter().map(|d| told(d, &root)).collect();
        let bundle: Vec<String> = in_bundle.diagnostics.iter().map(|d| told(d, "")).collect();
        assert_eq!(disk, bundle);

        // And they are findings: one of each kind the bundle refuses
        let codes_found: BTreeSet<&str> = in_bundle.diagnostics.iter().map(|d| d.code).collect();
        let expected: BTreeSet<&str> = [
            codes::TOML_PARSE,
            codes::UNKNOWN_ARCHETYPE,
            codes::MISSING_ASSET,
            codes::MISSING_FILE,
            codes::BAD_LOOK,
        ]
        .into();
        assert_eq!(codes_found, expected, "{bundle:#?}");
        assert_eq!(in_bundle.scenes.len(), 2);
        let missing: Vec<&str> = in_bundle
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::MISSING_ASSET || d.code == codes::MISSING_FILE)
            .map(|d| d.message.split('`').nth(1).unwrap_or_default())
            .collect();
        assert_eq!(
            missing,
            [
                "boat",
                "textures/paint.png",
                "puff.png",
                "kart_body.glb",
                "audio/creak.ogg"
            ]
        );
    }

    /// `flint validate`'s set leaves the files alone, and `flint check`'s
    /// adds the schema pass the bundle leaves out.
    #[test]
    fn the_three_callers_ask_for_different_checks_of_one_pass() {
        let mut source = MemorySource::new();
        for (path, text) in FILES {
            source.insert(*path, *text);
        }
        source.insert(
            "scenes/act1/dock.scene.toml",
            "[scene]\nname = \"Dock\"\n\n[entities.boat.model]\nasset = \"boat\"\n\n\
             [entities.buoy.lihgt]\non = true\n",
        );
        let keys: Vec<String> = FILES.iter().map(|(path, _)| path.to_string()).collect();
        let subject = Subject::packed(keys, Schemas::Listed(vec!["schemas".to_string()]));
        let codes_of = |checks: Checks| -> BTreeSet<&'static str> {
            run(&subject, checks)
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect()
        };
        vfs::scoped(Arc::new(source), || {
            let all = codes_of(Checks::ALL);
            let bundle = codes_of(Checks::BUNDLE);
            let validate = codes_of(Checks::VALIDATE);
            assert!(all.contains(codes::UNKNOWN_COMPONENT) && all.contains(codes::MISSING_ASSET));
            assert!(!bundle.contains(codes::UNKNOWN_COMPONENT), "{bundle:?}");
            assert!(bundle.contains(codes::MISSING_ASSET));
            assert!(validate.contains(codes::UNKNOWN_COMPONENT));
            assert!(!validate.contains(codes::MISSING_ASSET), "{validate:?}");
            assert!(bundle.is_subset(&all) && validate.is_subset(&all));
        });
    }
}
