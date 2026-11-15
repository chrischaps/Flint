//! `flint render --watch` (#432): write the same `--output` again on every
//! save, in one process.
//!
//! An agent tuning a user style edits its `.wgsl` and looks at the PNG. A
//! fresh `flint render` pays for the device, the scene, its models and the
//! renderer's pipelines every time; most of a render is that. Under
//! `--watch` the device lives for the whole run ([`RenderHost`]), and a
//! still or a sweep goes further: it keeps its scene and renderer, and for
//! a save of a user style's sources alone ([`StyleSources`], #429's rule)
//! it asks every user style's pass to look at its files now
//! ([`custom::poll_files_now`]) and draws again. The pass builds the new
//! WGSL itself, as it does in the player sixty times a second. Any other
//! save, or a `.shader.toml` whose description changed (its parameters may
//! have), loads the scene again on the same device.
//!
//! What is watched: the scene's folder (recursively, as `flint edit
//! --watch` does), the style folders the scene's search walks, the schema
//! directories, and each file a flag names. The render's own outputs are
//! not changes ([`Written`]), nor are editors' scratch files.

use super::render::{RenderArgs, RenderHost};
use anyhow::{Context, Result};
use flint_core::project::AssetScope;
use flint_render::stylize::custom;
use flint_viewer::watch::StyleSources;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// How long a folder must be quiet before its saves are one batch: an
/// editor's save is often a truncate and a write, or a write and a rename.
const SETTLE: Duration = Duration::from_millis(100);

/// `flint render --watch`: render, then render again after every save.
pub fn run(args: RenderArgs) -> Result<()> {
    let mut host = RenderHost::default();
    let mut watch = Watch::new(&args)?;
    println!("{}", watch.watching_line());
    let mut cause = None;
    loop {
        watch.begin(cause.take(), false);
        let result = super::render::render(&args, &mut host, Some(&mut watch));
        watch.finish(&result);
        if result.is_err() {
            // A GPU that may be in no state to trust is made again
            host.drop_device();
        }
        let paths = match watch.reload.take() {
            Some(paths) => paths,
            None => match watch.next()? {
                Change::Styles(p) | Change::Scene(p) => p,
            },
        };
        cause = Some(paths);
    }
}

/// What woke the watcher.
enum Change {
    /// Only user styles' WGSL or descriptions: the renderer can stay.
    Styles(Vec<PathBuf>),
    /// Anything else: the scene loads again.
    Scene(Vec<PathBuf>),
}

/// The render being timed, for its line.
struct Pending {
    started: Instant,
    cause: Option<Vec<PathBuf>>,
    /// Drawn again on the renderer that drew the last frame.
    kept: bool,
}

/// The file watcher and the run's count of renders.
pub struct Watch {
    _debouncer: Debouncer<RecommendedWatcher>,
    rx: Receiver<DebounceEventResult>,
    styles: StyleSources,
    written: Vec<Written>,
    /// Folders and files watched, for the first line.
    watched: Vec<PathBuf>,
    /// A change [`Watch::redraws`] handed back: the scene loads again.
    reload: Option<Vec<PathBuf>>,
    pending: Option<Pending>,
    renders: u32,
    /// Each changed file's stamp when it last woke the watcher.
    stamps: std::collections::HashMap<PathBuf, Option<flint_core::file_watch::Stamp>>,
}

impl Watch {
    fn new(args: &RenderArgs) -> Result<Self> {
        if args.scene.is_empty() {
            anyhow::bail!("--watch needs a scene (flint render <scene.toml> --watch)");
        }
        let scene = Path::new(&args.scene);
        let assets = AssetScope::for_scene(scene);
        let scene_dir = match scene.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        // Folders, each with all below it; then single files
        let mut dirs = vec![scene_dir];
        dirs.extend(custom::style_dirs(&assets));
        if let Ok((_, schemas)) = super::common_args::load_schemas(Some(&args.scene), &args.schemas)
        {
            dirs.extend(schemas.dirs.iter().cloned());
        }
        let mut files: Vec<PathBuf> = [
            args.variants.as_deref(),
            args.reference.as_deref(),
            args.camera_path.as_deref(),
            args.expression_timeline.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .collect();
        // A look found as a file (a built-in has none)
        if let Some(name) = args.look.as_deref() {
            if let Ok(look) = flint_render::look::find_in(name, &assets) {
                files.push(PathBuf::from(look.source));
            }
        }
        let (tx, rx) = channel();
        let mut debouncer = new_debouncer(SETTLE, tx).context("Failed to start a file watcher")?;
        let mut watched: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            let Ok(dir) = std::path::absolute(&dir) else {
                continue;
            };
            if !dir.is_dir() || watched.iter().any(|w| dir.starts_with(w)) {
                continue;
            }
            // One that holds folders already watched takes their place
            watched.retain(|w| !w.starts_with(&dir));
            watched.push(dir);
        }
        let mut watcher_paths: Vec<(PathBuf, RecursiveMode)> = watched
            .iter()
            .map(|d| (d.clone(), RecursiveMode::Recursive))
            .collect();
        for file in files {
            let Ok(file) = std::path::absolute(&file) else {
                continue;
            };
            if file.is_file() && !watched.iter().any(|w| file.starts_with(w)) {
                watcher_paths.push((file.clone(), RecursiveMode::NonRecursive));
                watched.push(file);
            }
        }
        for (path, mode) in &watcher_paths {
            debouncer
                .watcher()
                .watch(path, *mode)
                .with_context(|| format!("Failed to watch {}", path.display()))?;
        }
        Ok(Self {
            _debouncer: debouncer,
            rx,
            styles: StyleSources::new(custom::style_dirs(&assets)),
            written: Written::of(args),
            watched,
            reload: None,
            pending: None,
            renders: 0,
            stamps: Default::default(),
        })
    }

    fn watching_line(&self) -> String {
        let names: Vec<String> = self.watched.iter().map(|p| shown(p)).collect();
        format!(
            "watch: {} (Ctrl-C to stop)",
            match names.len() {
                0 => "nothing to watch".to_string(),
                _ => names.join(", "),
            }
        )
    }

    /// Start timing a render.
    fn begin(&mut self, cause: Option<Vec<PathBuf>>, kept: bool) {
        self.pending = Some(Pending {
            started: Instant::now(),
            cause,
            kept,
        });
    }

    /// The render's line, once: its number, time and cause, then every
    /// user style's error as it stands.
    fn finish(&mut self, result: &Result<()>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.renders += 1;
        let seconds = pending.started.elapsed().as_secs_f64();
        let how = match (&pending.cause, pending.kept) {
            (None, _) => String::new(),
            (Some(paths), kept) => format!(
                " ({}; {} changed)",
                if kept {
                    "same renderer"
                } else {
                    "scene loaded again"
                },
                changed_names(paths)
            ),
        };
        match result {
            Ok(()) => println!("watch: render {} in {seconds:.2} s{how}", self.renders),
            Err(e) => println!(
                "watch: render {} failed after {seconds:.2} s{how}: {e:#}",
                self.renders
            ),
        }
        let mut errors = custom::errors();
        errors.extend(flint_render::stylize::dev_shader::errors());
        for (style, message) in errors {
            println!("  style '{style}': {}", message.replace('\n', "\n    "));
        }
    }

    /// The still's loop (#432): with its first frame written, draw it again
    /// with `draw` on the same renderer for each save of user styles'
    /// sources alone, and return for anything that needs the scene loaded
    /// again (kept in [`Self::reload`]).
    pub(super) fn redraws(
        &mut self,
        assets: &AssetScope,
        mut draw: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        self.finish(&Ok(()));
        loop {
            let paths = match self.next()? {
                Change::Scene(paths) => {
                    self.reload = Some(paths);
                    return Ok(());
                }
                Change::Styles(paths) => paths,
            };
            self.begin(Some(paths.clone()), true);
            // A description read again may bring new parameters, which the
            // scene's tables were checked against: those load it again
            let reload = custom::reload_changed_for(assets);
            for message in &reload.messages {
                eprintln!("warning: {message}");
            }
            if reload.changed() {
                self.pending = None;
                self.reload = Some(paths);
                return Ok(());
            }
            custom::poll_files_now();
            let result = draw();
            self.finish(&result);
        }
    }

    /// Wait for a batch of saves that aren't the render's own.
    fn next(&mut self) -> Result<Change> {
        loop {
            let events = match self.rx.recv().context("the file watcher stopped")? {
                Ok(events) => events,
                Err(e) => {
                    eprintln!("watch: {e}");
                    continue;
                }
            };
            tracing::debug!(
                "watch: {:?}",
                events.iter().map(|e| (&e.path, e.kind)).collect::<Vec<_>>()
            );
            let mut paths: Vec<PathBuf> = events
                .into_iter()
                .map(|e| e.path)
                .filter(|p| !self.ignores(p))
                .collect();
            paths.sort();
            paths.dedup();
            // One save can arrive as two batches (the debouncer splits a
            // burst that outlasts SETTLE): a file whose stamp is the one
            // already drawn for is no change
            paths.retain(|p| {
                let now = flint_core::file_watch::stamp_now(p);
                self.stamps.insert(p.clone(), now) != Some(now)
            });
            if paths.is_empty() {
                continue;
            }
            let scene = self
                .styles
                .wants_scene_reload(paths.iter().map(PathBuf::as_path));
            return Ok(match scene {
                true => Change::Scene(paths),
                false => Change::Styles(paths),
            });
        }
    }

    /// Whether a changed path is no reason to render: one the render wrote,
    /// a folder (its files come as their own events), or editor scratch.
    fn ignores(&self, path: &Path) -> bool {
        path.is_dir() || is_scratch(path) || self.written.iter().any(|w| w.covers(path))
    }
}

/// A file the render writes, and the ones named after it (`x_0000.png` for
/// `--time 2,5 -o x.png`, `<out>-<label>.png` for variants, a GIF's
/// `x.gif.frames~`, `f-0001.png` for `-o f-%04d.png`): never a change.
#[derive(Debug, Clone, PartialEq)]
struct Written {
    /// Its folder, in the spellings a watcher event can come in.
    dirs: Vec<PathBuf>,
    /// Its file name.
    name: String,
    /// What the files named after it start with.
    stem: String,
    /// A `%` pattern (`f-%04d.png`): anything after the stem.
    pattern: bool,
}

impl Written {
    fn of(args: &RenderArgs) -> Vec<Self> {
        let report = args.report.as_deref().filter(|r| *r != "json");
        [
            Some(args.output.as_str()),
            report,
            args.ascii_text.as_deref(),
            args.ascii_text_ansi.as_deref(),
            args.gpu_time_json.as_deref(),
            args.flip_json.as_deref(),
            args.refine_out.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|path| Self::new(Path::new(path)))
        .collect()
    }

    fn new(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        let pattern = name.contains('%');
        let stem = match name.find('%') {
            Some(at) => name[..at].to_string(),
            None => match name.find('.') {
                Some(at) => name[..at].to_string(),
                None => name.clone(),
            },
        };
        Some(Self {
            dirs: spellings(dir),
            name,
            stem,
            pattern,
        })
    }

    fn covers(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        let name = name.to_ascii_lowercase();
        let named = name == self.name
            || name
                .strip_prefix(self.stem.as_str())
                .is_some_and(|rest| self.pattern || rest.starts_with(['_', '-', '.']));
        named
            && path
                .parent()
                .is_some_and(|parent| spellings(parent).iter().any(|p| self.dirs.contains(p)))
    }
}

/// An editor's or a tool's scratch file, or a folder no render reads from.
fn is_scratch(path: &Path) -> bool {
    let skipped_dir = path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some(".git" | ".flint" | "target" | "node_modules")
        )
    });
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    skipped_dir
        || name.ends_with('~')
        || name.ends_with(".tmp")
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.contains("___jb_")
        || name == "4913"
}

/// `dir` made absolute, and canonical when it exists.
fn spellings(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    if let Ok(abs) = std::path::absolute(dir) {
        out.push(abs);
    }
    if let Ok(canon) = dir.canonicalize() {
        if !out.contains(&canon) {
            out.push(canon);
        }
    }
    out
}

/// A path as the working directory names it, with `/`.
fn shown(path: &Path) -> String {
    let here = std::env::current_dir().ok();
    let short = here
        .as_deref()
        .and_then(|h| path.strip_prefix(h).ok())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(path);
    short.display().to_string().replace('\\', "/")
}

/// The changed files' names, at most three.
fn changed_names(paths: &[PathBuf]) -> String {
    let mut names: Vec<String> = paths.iter().take(3).map(|p| shown(p)).collect();
    if paths.len() > 3 {
        names.push(format!("{} more", paths.len() - 3));
    }
    names.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flint-render-watch-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_output_and_files_named_after_it_are_not_changes() {
        let dir = temp_dir("written");
        let w = Written::new(&dir.join("shot.png")).unwrap();
        assert!(w.covers(&dir.join("shot.png")));
        assert!(w.covers(&dir.join("SHOT.PNG")));
        assert!(
            w.covers(&dir.join("shot_0000.png")),
            "a moment of --time 2,5"
        );
        assert!(w.covers(&dir.join("shot-warm.png")), "a variant's frame");
        assert!(w.covers(&dir.join("shot.gif.frames~")));
        assert!(!w.covers(&dir.join("shots.png")), "another file");
        assert!(
            !w.covers(&dir.join("styles").join("shot.png")),
            "another folder"
        );
        let seq = Written::new(&dir.join("f-%04d.png")).unwrap();
        assert!(seq.covers(&dir.join("f-0001.png")));
        assert!(!seq.covers(&dir.join("g-0001.png")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_relative_output_matches_its_absolute_event() {
        let w = Written::new(Path::new("render.png")).unwrap();
        let here = std::env::current_dir().unwrap();
        assert!(w.covers(&here.join("render.png")));
        assert!(!w.covers(&here.join("styles").join("render.png")));
    }

    #[test]
    fn scratch_files_and_tool_folders_are_not_changes() {
        assert!(is_scratch(Path::new("styles/x.wgsl~")));
        assert!(is_scratch(Path::new("styles/.x.wgsl.swp")));
        assert!(is_scratch(Path::new("styles/x.wgsl___jb_tmp___")));
        assert!(is_scratch(Path::new("game/.flint/procgen/a.glb")));
        assert!(is_scratch(Path::new("game/target/debug/flint.exe")));
        assert!(!is_scratch(Path::new("styles/x.wgsl")));
        assert!(!is_scratch(Path::new("scenes/a.scene.toml")));
    }
}
