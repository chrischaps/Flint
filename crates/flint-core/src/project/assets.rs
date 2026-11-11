//! The one asset search (ADR 0114, decision 4; #706).
//!
//! Every kind of asset is looked for in the same places, in the same order:
//!
//! 1. the scene's own directory;
//! 2. each directory above it, up to and including the project root;
//! 3. the mounted engine directory, `<root>/engine/`, when the project has
//!    one (a game keeps the engine as a subtree there). In an engine checkout
//!    the root already is the engine, so there is no third place.
//!
//! At each directory `D` of that walk the candidates are
//! `D/<folder>/<name><extension>`, for each of the kind's folders and then
//! each of its extensions. A name that already ends in one of the kind's
//! extensions is taken as written, and a name may carry subfolders
//! (`fx/fire` is `D/particles/fx/fire.particles.toml`). The first candidate
//! that is a file wins.
//!
//! A kind is data: a label, its folders and its extensions ([`KindInfo`]).
//! Nothing else may differ from one kind to the next, and the walk never
//! looks at which kind it is serving.
//!
//! Two edges of the walk, the same for every kind:
//!
//! - A scene outside the project's root (`--project` naming another folder)
//!   is searched beside itself, then at the root, then in the engine.
//! - A project found by no marker at all ([`Found::Fallback`]) has the
//!   scene's directory as its root. Every search the engine had before there
//!   were projects also looked one level above the scene, and with no
//!   manifest the engine behaves as it did, so that level is searched too.
//!
//! ```
//! use flint_core::project::{AssetKind, Project};
//! use flint_core::vfs::{self, MemorySource};
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! let mut files = MemorySource::new();
//! files.insert("flint.toml", "");
//! files.insert("particles/fire.particles.toml", "");
//! files.insert("scenes/act1/cave/a.scene.toml", "");
//! vfs::scoped(Arc::new(files), || {
//!     let scene_dir = Path::new("scenes/act1/cave");
//!     let project = Project::discover(scene_dir).unwrap();
//!     let found = project.resolve(AssetKind::Particles, "fire", scene_dir).unwrap();
//!     assert_eq!(found, Path::new("particles/fire.particles.toml"));
//!     let missing = project.resolve(AssetKind::Particles, "ice", scene_dir).unwrap_err();
//!     assert_eq!(missing.tried.len(), 4); // cave, act1, scenes, the root
//! });
//! ```

use super::{anchor, parent_dir, same_dir_key, Found, Project};
use crate::vfs;
use std::collections::HashSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// A kind of asset the resolver can find. What a kind *is* lives in its
/// [`KindInfo`]; this is only its name in code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AssetKind {
    /// Particle effects: `particles/<name>.particles.toml`.
    Particles,
}

/// Everything that tells one kind from another: where its files sit under
/// each directory of the walk, and how their names end.
#[derive(Debug)]
pub struct KindInfo {
    /// What a person types: `flint asset where <label> <name>`.
    pub label: &'static str,
    /// The kind in a sentence: "particle effect".
    pub noun: &'static str,
    /// Folders under each directory of the walk, in the order tried. An
    /// empty string is the directory itself.
    pub folders: &'static [&'static str],
    /// File endings, in the order tried, each with its dot
    /// (`.particles.toml`). A name already ending in one is taken as it is.
    pub extensions: &'static [&'static str],
    /// TEMPORARY, for one release (ADR 0114 decision 3, #765): the search
    /// this kind had before the one rule, as the file it would have picked.
    /// `None` once the release has passed, or when the kind never differed.
    old_rule: Option<OldRule>,
}

/// A kind's search before the one rule: the file it would have picked for
/// `name` from a scene in `scene_dir`, if any.
type OldRule = fn(name: &str, scene_dir: &Path) -> Option<PathBuf>;

static PARTICLES: KindInfo = KindInfo {
    label: "particles",
    noun: "particle effect",
    folders: &["particles"],
    extensions: &[".particles.toml"],
    old_rule: Some(old_particles),
};

/// TEMPORARY (see [`KindInfo::old_rule`]). Particles before ADR 0114:
/// `particles/` beside the scene if that directory exists, else `particles/`
/// one level up if that exists, and only that one directory, whatever it
/// holds (`flint_particles::resolve_particles_dir` as it was).
fn old_particles(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let local = scene_dir.join("particles");
    let dir = if vfs::is_dir(&local) {
        local
    } else {
        let up = scene_dir.parent()?.join("particles");
        vfs::is_dir(&up).then_some(up)?
    };
    let file = dir.join(format!("{name}.particles.toml"));
    vfs::is_file(&file).then_some(file)
}

impl AssetKind {
    /// Every kind, in the order `flint asset where --help` lists them.
    pub const ALL: &'static [AssetKind] = &[AssetKind::Particles];

    pub fn info(self) -> &'static KindInfo {
        match self {
            AssetKind::Particles => &PARTICLES,
        }
    }

    /// The kind's label: `particles`.
    pub fn label(self) -> &'static str {
        self.info().label
    }

    /// The kind a label names, without case.
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.label().eq_ignore_ascii_case(label))
    }
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl KindInfo {
    /// The file names a name stands for: itself when it already ends in one
    /// of the kind's extensions (or the kind has none), else the name with
    /// each extension in turn.
    pub fn file_names(&self, name: &str) -> Vec<String> {
        if self.extensions.is_empty() || self.extensions.iter().any(|e| name.ends_with(e)) {
            return vec![name.to_string()];
        }
        self.extensions
            .iter()
            .map(|e| format!("{name}{e}"))
            .collect()
    }

    /// A file's asset name: its file name less the kind's extension. `None`
    /// for a file of another kind.
    pub fn name_of(&self, file: &Path) -> Option<String> {
        let file_name = file.file_name()?.to_str()?;
        if self.extensions.is_empty() {
            return Some(file_name.to_string());
        }
        self.extensions
            .iter()
            .find_map(|e| file_name.strip_suffix(e))
            .filter(|stem| !stem.is_empty())
            .map(str::to_string)
    }
}

/// No file of this kind and name anywhere the engine looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotFound {
    pub kind: AssetKind,
    pub name: String,
    /// Every path tried, in the order tried.
    pub tried: Vec<PathBuf>,
}

impl fmt::Display for NotFound {
    /// One line saying what is missing, then each path tried on its own.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} '{}' not found. Tried:",
            self.kind.info().noun,
            self.name
        )?;
        for path in &self.tried {
            write!(f, "\n  {}", shown(path))?;
        }
        Ok(())
    }
}

impl std::error::Error for NotFound {}

/// TEMPORARY, for one release (ADR 0114 decision 3, #765): the kind's old
/// search would have picked another file than the one rule does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderChange {
    pub kind: AssetKind,
    pub name: String,
    /// The file the old rule picked.
    pub old: PathBuf,
    /// The file the one rule picks; `None` when it finds none (the old file
    /// lies outside the project).
    pub new: Option<PathBuf>,
}

impl fmt::Display for OrderChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} '{}': the search order changed (ADR 0114). Before, the engine loaded {}; ",
            self.kind.info().noun,
            self.name,
            shown(&self.old)
        )?;
        match &self.new {
            Some(new) => write!(
                f,
                "now it loads {}. Move or rename one of them if that is the wrong file",
                shown(new)
            ),
            None => f.write_str(
                "now it loads none: that file is outside the project. Move it into the \
                 project, or put a flint.toml at the folder that holds both",
            ),
        }
    }
}

impl OrderChange {
    /// Whether this is the first time this process has seen this exact
    /// change (kind, name and both files). The log uses it to speak once.
    pub fn first_time(&self) -> bool {
        static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        // `|` is in no file name the key is built from on any platform
        // that matters here; a clash would only cost one warning.
        let key = format!(
            "{}|{}|{}|{}",
            self.kind,
            self.name,
            same_dir_key(&self.old),
            self.new.as_deref().map(same_dir_key).unwrap_or_default(),
        );
        SEEN.get_or_init(Default::default)
            .lock()
            .map(|mut seen| seen.insert(key))
            .unwrap_or(false)
    }

    /// Warn about it, once a process for each distinct change. Through
    /// `tracing`, which every host sends to stderr or the browser console:
    /// never stdout, which `flint render --report json` owns.
    fn log(&self) {
        if self.first_time() {
            tracing::warn!("{self}");
        }
    }
}

/// The whole answer to "which file?": what `flint asset where` prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub kind: AssetKind,
    pub name: String,
    /// The file the engine loads, if there is one.
    pub found: Option<PathBuf>,
    /// The paths tried and not found, in order: all that came before
    /// `found`, or every candidate when nothing was found.
    pub tried: Vec<PathBuf>,
    /// TEMPORARY (see [`OrderChange`]).
    pub order_change: Option<OrderChange>,
}

/// One file [`Project::list`] found: the name a scene would use for it, and
/// where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub path: PathBuf,
}

impl Project {
    /// The file the engine loads for `name`, an asset of `kind`, in a scene
    /// whose directory is `scene_dir`; or everywhere it looked. The one
    /// search rule is in the module docs. Paths keep the spelling of
    /// `scene_dir` (relative stays relative) for as long as it lasts.
    pub fn resolve(
        &self,
        kind: AssetKind,
        name: &str,
        scene_dir: &Path,
    ) -> Result<PathBuf, NotFound> {
        let resolution = self.explain(kind, name, scene_dir);
        if let Some(change) = &resolution.order_change {
            change.log();
        }
        match resolution.found {
            Some(path) => Ok(path),
            None => Err(NotFound {
                kind,
                name: resolution.name,
                tried: resolution.tried,
            }),
        }
    }

    /// [`resolve`](Self::resolve) with its working shown and nothing logged.
    pub fn explain(&self, kind: AssetKind, name: &str, scene_dir: &Path) -> Resolution {
        let info = kind.info();
        let names = info.file_names(name);
        let mut tried = Vec::new();
        let mut found = None;
        'search: for dir in self.search_dirs(kind, scene_dir) {
            for file_name in &names {
                let path = dir.join(file_name);
                if vfs::is_file(&path) {
                    found = Some(path);
                    break 'search;
                }
                tried.push(path);
            }
        }
        let order_change = order_change(kind, name, scene_dir, found.as_deref());
        Resolution {
            kind,
            name: name.to_string(),
            found,
            tried,
            order_change,
        }
    }

    /// Every file of `kind` a scene in `scene_dir` can see, by the same
    /// rule: a nearer file hides a farther one of the same name. Nearest
    /// first, and by name within a directory. Only files directly in a
    /// kind's folder are listed, not those in subfolders.
    pub fn list(&self, kind: AssetKind, scene_dir: &Path) -> Vec<Listed> {
        let info = kind.info();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for dir in self.search_dirs(kind, scene_dir) {
            let Ok(mut entries) = vfs::read_dir(&dir) else {
                continue;
            };
            entries.sort();
            for path in entries {
                let Some(name) = info.name_of(&path) else {
                    continue;
                };
                if !seen.insert(name.clone()) {
                    continue;
                }
                if let Some(change) = order_change(kind, &name, scene_dir, Some(&path)) {
                    change.log();
                }
                out.push(Listed { name, path });
            }
        }
        out
    }

    /// The directories searched for `kind` from a scene in `scene_dir`, in
    /// order, whether or not they exist: the kind's folders under each
    /// directory of the walk, then under the engine mount.
    pub fn search_dirs(&self, kind: AssetKind, scene_dir: &Path) -> Vec<PathBuf> {
        let folders = kind.info().folders;
        let (mut levels, root) = self.levels(scene_dir);
        levels.extend(self.engine_under(&root));
        let mut out = Vec::with_capacity(levels.len() * folders.len());
        for level in &levels {
            for folder in folders {
                out.push(under(level, folder));
            }
        }
        out
    }

    /// The mounted engine directory: `<root>/engine` when it is a directory
    /// and this project isn't the engine checkout itself.
    pub fn engine_mount(&self) -> Option<PathBuf> {
        self.engine_under(&self.root)
    }

    /// [`engine_mount`](Self::engine_mount) with the root spelled as the
    /// caller has it.
    fn engine_under(&self, root: &Path) -> Option<PathBuf> {
        if self.found == Found::Engine {
            return None;
        }
        let engine = under(root, "engine");
        vfs::is_dir(&engine).then_some(engine)
    }

    /// The directories of the walk: the scene's, then each above it up to
    /// the root. See the module docs for a scene outside the root and for a
    /// project with no marker. With them, the root in the walk's own
    /// spelling (relative when the scene's directory was).
    fn levels(&self, scene_dir: &Path) -> (Vec<PathBuf>, PathBuf) {
        let start = if scene_dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            scene_dir.to_path_buf()
        };
        let root_key = same_dir_key(&self.root);
        let mut out = Vec::new();
        let mut reached = false;
        let mut root = self.root.clone();
        let mut dir = start.clone();
        // No tree is this deep; the bound only keeps a strange path finite.
        for _ in 0..128 {
            if same_dir_key(&dir) == root_key {
                root = dir.clone();
                out.push(dir);
                reached = true;
                break;
            }
            let above = parent_dir(&dir).or_else(|| {
                // The spelling ran out (`.`, or it ends in `..`): carry on
                // from where it really is, if that is anywhere.
                let real = fold(&anchor(&dir));
                parent_dir(&real).filter(|_| real != dir)
            });
            out.push(dir);
            match above {
                Some(above) => dir = above,
                None => break,
            }
        }
        if !reached {
            // The scene isn't under the root: beside itself, then the root.
            out.truncate(1);
        }
        if self.found == Found::Fallback && out.len() == 1 {
            let above = parent_dir(&start).or_else(|| parent_dir(&fold(&anchor(&start))));
            out.extend(above);
        }
        if !reached {
            out.push(self.root.clone());
        }
        let mut seen = HashSet::new();
        out.retain(|d| seen.insert(same_dir_key(d)));
        (out, root)
    }

    /// The project a scene belongs to, for a loader that has to carry on
    /// whatever the manifest says: as [`for_path`](Self::for_path), and when
    /// the manifest can't be read, a warning and the project without it.
    pub fn for_scene(scene: &Path) -> Project {
        Self::for_path(scene).unwrap_or_else(|e| {
            tracing::warn!("{e}; searching for assets as if there were no manifest");
            let dir = scene.parent().filter(|p| !p.as_os_str().is_empty());
            Project::bare(
                dir.map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(".")),
            )
        })
    }
}

/// A path for a message or a report: with forward slashes on every platform,
/// so a walk begun from `scenes/act1` is not spelled `scenes/act1\particles`
/// on Windows, and a report reads the same everywhere.
pub fn shown(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// `dir/rel`, with `.` left out of the spelling.
fn under(dir: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        dir.to_path_buf()
    } else if dir == Path::new(".") {
        PathBuf::from(rel)
    } else {
        dir.join(rel)
    }
}

/// `path` with `.` dropped and each `..` folded into what precedes it.
fn fold(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// TEMPORARY (see [`OrderChange`]): what the kind's old rule would have
/// picked, when that is another file than `new`.
fn order_change(
    kind: AssetKind,
    name: &str,
    scene_dir: &Path,
    new: Option<&Path>,
) -> Option<OrderChange> {
    let old = (kind.info().old_rule?)(name, scene_dir)?;
    if new.is_some_and(|new| same_dir_key(new) == same_dir_key(&old)) {
        return None;
    }
    Some(OrderChange {
        kind,
        name: name.to_string(),
        old,
        new: new.map(Path::to_path_buf),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemorySource;
    use std::sync::Arc;

    const FIRE: &str = "fire.particles.toml";

    /// Run `f` over a memory source holding `files`; a path ending in `/`
    /// is a directory.
    fn with_files<R>(files: &[&str], f: impl FnOnce() -> R) -> R {
        let mut source = MemorySource::new();
        for path in files {
            if path.ends_with('/') {
                source.insert(format!("{path}.keep"), "");
            } else {
                source.insert(path, "");
            }
        }
        vfs::scoped(Arc::new(source), f)
    }

    fn project(start: &str) -> Project {
        Project::discover_with_home(Path::new(start), None).unwrap()
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    fn fire(project: &Project, scene_dir: &str) -> Result<PathBuf, NotFound> {
        project.resolve(AssetKind::Particles, "fire", Path::new(scene_dir))
    }

    #[test]
    fn scene_beats_parent_beats_root_beats_engine() {
        let all = [
            "game/flint.toml",
            "game/scenes/act1/a.scene.toml",
            "game/scenes/act1/particles/fire.particles.toml",
            "game/scenes/particles/fire.particles.toml",
            "game/particles/fire.particles.toml",
            "game/engine/particles/fire.particles.toml",
        ];
        let expect = [
            "game/scenes/act1/particles",
            "game/scenes/particles",
            "game/particles",
            "game/engine/particles",
        ];
        // Take the nearest away each time: the next one is found, and the
        // ones taken away are the tried list, in order.
        for gone in 0..expect.len() {
            let files: Vec<&str> = all
                .iter()
                .copied()
                .filter(|f| !expect[..gone].iter().any(|dir| f.starts_with(dir)))
                .chain(["game/engine/.keep"])
                .collect();
            with_files(&files, || {
                let p = project("game/scenes/act1/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("game/scenes/act1"));
                assert_eq!(r.found, Some(Path::new(expect[gone]).join(FIRE)));
                let missed: Vec<PathBuf> = expect[..gone]
                    .iter()
                    .map(|d| Path::new(d).join(FIRE))
                    .collect();
                assert_eq!(r.tried, missed);
            });
        }
    }

    #[test]
    fn a_scene_two_folders_deep_finds_the_projects_particles() {
        with_files(
            &[
                "game/flint.toml",
                "game/particles/fire.particles.toml",
                "game/scenes/act1/cave/a.scene.toml",
                "game/scenes/b.scene.toml",
            ],
            || {
                let deep = project("game/scenes/act1/cave/a.scene.toml");
                let shallow = project("game/scenes/b.scene.toml");
                let want = Path::new("game/particles").join(FIRE);
                assert_eq!(fire(&deep, "game/scenes/act1/cave").unwrap(), want);
                assert_eq!(fire(&shallow, "game/scenes").unwrap(), want);
            },
        );
    }

    #[test]
    fn not_found_lists_every_path_in_the_order_tried() {
        with_files(
            &[
                "game/flint.toml",
                "game/engine/schemas/",
                "game/scenes/act1/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/act1/a.scene.toml");
                let err = fire(&p, "game/scenes/act1").unwrap_err();
                assert_eq!(err.kind, AssetKind::Particles);
                assert_eq!(err.name, "fire");
                let dirs = [
                    "game/scenes/act1/particles",
                    "game/scenes/particles",
                    "game/particles",
                    "game/engine/particles",
                ];
                let want: Vec<PathBuf> = dirs.iter().map(|d| Path::new(d).join(FIRE)).collect();
                assert_eq!(err.tried, want);
                let text = err.to_string();
                let lines: Vec<&str> = text.lines().collect();
                assert_eq!(lines[0], "particle effect 'fire' not found. Tried:");
                assert_eq!(lines.len(), 5, "{text}");
                for (line, path) in lines[1..].iter().zip(&want) {
                    assert_eq!(*line, format!("  {}", shown(path)));
                }
            },
        );
    }

    #[test]
    fn no_engine_folder_no_engine_place_and_an_engine_checkout_has_none() {
        with_files(&["game/flint.toml", "game/scenes/a.scene.toml"], || {
            let p = project("game/scenes/a.scene.toml");
            assert_eq!(p.engine_mount(), None);
            assert_eq!(
                p.search_dirs(AssetKind::Particles, Path::new("game/scenes")),
                paths(&["game/scenes/particles", "game/particles"])
            );
        });
        // The engine's own checkout, with an `engine/` folder that is not
        // a mount of anything.
        with_files(
            &[
                "flint/crates/flint-core/Cargo.toml",
                "flint/engine/particles/fire.particles.toml",
                "flint/demo/a.scene.toml",
            ],
            || {
                let p = project("flint/demo/a.scene.toml");
                assert_eq!(p.found, Found::Engine);
                assert_eq!(p.engine_mount(), None);
                assert!(fire(&p, "flint/demo").is_err());
            },
        );
    }

    #[test]
    fn names_with_an_extension_or_a_subfolder() {
        with_files(
            &[
                "game/flint.toml",
                "game/particles/fire.particles.toml",
                "game/particles/fx/spark.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let dir = Path::new("game/scenes");
                let k = AssetKind::Particles;
                assert_eq!(
                    p.resolve(k, "fire.particles.toml", dir).unwrap(),
                    Path::new("game/particles").join(FIRE)
                );
                assert_eq!(
                    p.resolve(k, "fx/spark", dir).unwrap(),
                    Path::new("game/particles").join("fx/spark.particles.toml")
                );
                // Never the bare name, and never doubled.
                let err = p.resolve(k, "ice.particles.toml", dir).unwrap_err();
                assert!(err
                    .tried
                    .iter()
                    .all(|t| t.ends_with("particles/ice.particles.toml")));
            },
        );
    }

    #[test]
    fn inside_a_bundle_paths_are_the_bundles_own() {
        // A bundle's root is `.`: the manifest and the folders sit at the
        // top of the file source, and engine files are mounted at `engine/`.
        with_files(
            &[
                "flint.toml",
                "engine/schemas/",
                "engine/particles/dust.particles.toml",
                "particles/fire.particles.toml",
                "scenes/act1/a.scene.toml",
            ],
            || {
                let p = project("scenes/act1/a.scene.toml");
                assert_eq!(p.root, Path::new("."));
                let dir = Path::new("scenes/act1");
                assert_eq!(
                    fire(&p, "scenes/act1").unwrap(),
                    Path::new("particles").join(FIRE)
                );
                assert_eq!(
                    p.search_dirs(AssetKind::Particles, dir),
                    paths(&[
                        "scenes/act1/particles",
                        "scenes/particles",
                        "particles",
                        "engine/particles"
                    ])
                );
                assert_eq!(
                    p.resolve(AssetKind::Particles, "dust", dir).unwrap(),
                    Path::new("engine/particles").join("dust.particles.toml")
                );
                // A scene at the bundle's root.
                assert_eq!(fire(&p, "").unwrap(), Path::new("particles").join(FIRE));
                assert_eq!(fire(&p, ".").unwrap(), Path::new("particles").join(FIRE));
            },
        );
    }

    #[test]
    fn a_project_with_no_marker_still_looks_one_level_up() {
        with_files(
            &[
                "loose/particles/fire.particles.toml",
                "loose/scenes/a.scene.toml",
            ],
            || {
                let p = project("loose/scenes/a.scene.toml");
                assert_eq!(p.found, Found::Fallback);
                assert_eq!(p.root, Path::new("loose/scenes"));
                assert_eq!(
                    fire(&p, "loose/scenes").unwrap(),
                    Path::new("loose/particles").join(FIRE)
                );
                assert_eq!(
                    p.search_dirs(AssetKind::Particles, Path::new("loose/scenes")),
                    paths(&["loose/scenes/particles", "loose/particles"])
                );
            },
        );
    }

    #[test]
    fn a_scene_outside_the_root_is_searched_beside_itself_then_at_the_root() {
        with_files(
            &[
                "game/flint.toml",
                "game/particles/fire.particles.toml",
                "elsewhere/deep/a.scene.toml",
            ],
            || {
                let p = project("game");
                assert_eq!(
                    p.search_dirs(AssetKind::Particles, Path::new("elsewhere/deep")),
                    paths(&["elsewhere/deep/particles", "game/particles"])
                );
            },
        );
    }

    #[test]
    fn listing_shadows_by_name_nearest_first() {
        with_files(
            &[
                "game/flint.toml",
                "game/engine/particles/dust.particles.toml",
                "game/engine/particles/fire.particles.toml",
                "game/particles/fire.particles.toml",
                "game/particles/rain.particles.toml",
                "game/particles/notes.txt",
                "game/scenes/particles/fire.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let listed = p.list(AssetKind::Particles, Path::new("game/scenes"));
                let got: Vec<(String, PathBuf)> =
                    listed.into_iter().map(|l| (l.name, l.path)).collect();
                assert_eq!(
                    got,
                    [
                        (
                            "fire".to_string(),
                            Path::new("game/scenes/particles").join(FIRE)
                        ),
                        (
                            "rain".to_string(),
                            Path::new("game/particles").join("rain.particles.toml")
                        ),
                        (
                            "dust".to_string(),
                            Path::new("game/engine/particles").join("dust.particles.toml")
                        ),
                    ]
                );
                // Each listed file is the one `resolve` gives for its name.
                for l in p.list(AssetKind::Particles, Path::new("game/scenes")) {
                    assert_eq!(
                        p.resolve(AssetKind::Particles, &l.name, Path::new("game/scenes")),
                        Ok(l.path)
                    );
                }
            },
        );
    }

    #[test]
    fn the_order_change_fires_when_the_old_rule_picked_another_file() {
        // The scene sits at the project root, and the old rule's "one
        // level up" is outside the project.
        with_files(
            &[
                "work/particles/fire.particles.toml",
                "work/game/flint.toml",
                "work/game/a.scene.toml",
            ],
            || {
                let p = project("work/game/a.scene.toml");
                let dir = Path::new("work/game");
                let r = p.explain(AssetKind::Particles, "fire", dir);
                assert_eq!(r.found, None);
                let change = r.order_change.expect("the old rule found one");
                assert_eq!(change.old, Path::new("work/particles").join(FIRE));
                assert_eq!(change.new, None);
                let text = change.to_string();
                assert!(text.contains("the search order changed"), "{text}");
                assert!(text.contains("outside the project"), "{text}");
            },
        );
        // The same, with an engine copy the new rule falls through to: the
        // warning names both files.
        with_files(
            &[
                "work/particles/fire.particles.toml",
                "work/game/flint.toml",
                "work/game/engine/particles/fire.particles.toml",
                "work/game/a.scene.toml",
            ],
            || {
                let p = project("work/game/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("work/game"));
                let change = r.order_change.expect("the old rule picked another");
                let new = Path::new("work/game/engine/particles").join(FIRE);
                assert_eq!(change.new.as_deref(), Some(new.as_path()));
                let text = change.to_string();
                assert!(text.contains(&shown(&change.old)), "{text}");
                let shown = shown(change.new.as_ref().unwrap());
                assert!(text.contains(&shown), "{text}");
            },
        );
    }

    #[test]
    fn the_order_change_stays_quiet_when_both_rules_agree_or_the_old_found_none() {
        // Both pick the file beside the scene.
        with_files(
            &[
                "game/flint.toml",
                "game/scenes/particles/fire.particles.toml",
                "game/particles/fire.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("game/scenes"));
                assert!(r.found.is_some());
                assert_eq!(r.order_change, None);
            },
        );
        // Both pick the one a level up.
        with_files(
            &[
                "game/flint.toml",
                "game/particles/fire.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("game/scenes"));
                assert_eq!(r.order_change, None);
            },
        );
        // The old rule stopped at the first `particles/` that existed and
        // found nothing there; the new one finds the project's. Something
        // found where nothing was is not a change of order.
        with_files(
            &[
                "game/flint.toml",
                "game/scenes/particles/rain.particles.toml",
                "game/particles/fire.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("game/scenes"));
                assert_eq!(r.found, Some(Path::new("game/particles").join(FIRE)));
                assert_eq!(r.order_change, None);
            },
        );
    }

    #[test]
    fn a_change_is_new_once_a_process() {
        let change = OrderChange {
            kind: AssetKind::Particles,
            name: "only_in_this_test".into(),
            old: PathBuf::from("a/particles/only_in_this_test.particles.toml"),
            new: None,
        };
        assert!(change.first_time());
        assert!(!change.first_time());
        let other = OrderChange {
            new: Some(PathBuf::from(
                "b/particles/only_in_this_test.particles.toml",
            )),
            ..change.clone()
        };
        assert!(other.first_time());
        assert!(!other.first_time());
    }

    #[test]
    fn kinds_are_data() {
        for kind in AssetKind::ALL {
            let info = kind.info();
            assert_eq!(AssetKind::from_label(info.label), Some(*kind));
            assert!(!info.folders.is_empty());
        }
        assert_eq!(
            AssetKind::from_label("PARTICLES"),
            Some(AssetKind::Particles)
        );
        assert_eq!(AssetKind::from_label("nope"), None);
        let info = AssetKind::Particles.info();
        assert_eq!(info.file_names("fire"), ["fire.particles.toml"]);
        assert_eq!(info.file_names("a.particles.toml"), ["a.particles.toml"]);
        assert_eq!(
            info.name_of(Path::new("x/fire.particles.toml")).as_deref(),
            Some("fire")
        );
        assert_eq!(info.name_of(Path::new("x/fire.toml")), None);
    }
}
