//! Loading `styles/*.shader.toml` into a [`StyleRegistry`], hot reload and
//! errors (moved from `custom.rs`, #742).

use super::*;

// ---------------------------------------------------------------------------
// Loading: styles/*.shader.toml
// ---------------------------------------------------------------------------

/// What a user style was loaded as: its descriptor, its description and
/// its errors, shared by the registries that hold it and the pass built
/// from it, and freed with the last of them (#742).
pub(crate) struct Loaded {
    /// The descriptor the registry hands out ([`Style`]): its names and
    /// labels interned ([`intern`]), its parameter and option lists its own.
    pub(crate) descriptor: StyleDescriptor,
    pub(crate) source: StyleSource,
    /// Bind cache keys, pass labels and target names.
    pub(super) names: BindNames,
    /// The last error (load, check or build), for hosts to show.
    pub(super) error: Mutex<Option<String>>,
    /// The images' error (#413): a file missing or that doesn't decode.
    /// Kept apart from the shader's, which a good build clears.
    pub(super) image_error: Mutex<Option<String>>,
    /// The `.shader.toml`'s modified time when last read
    /// ([`StyleRegistry::reload_changed_for`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) toml_modified: Mutex<Option<std::time::SystemTime>>,
    /// Each image option that found its choices in a folder, with what the
    /// reload poll last listed there (#558): another listing is a new
    /// install with the new choices.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) folders: Mutex<Vec<(String, Vec<String>)>>,
}

/// A style's bind cache keys, pass labels and target names. The same
/// names for every install of a style, interned: an install is told from
/// another by its generation, which [`StyleFrame`] mixes into each key
/// (#742).
#[derive(Clone)]
pub(super) struct BindNames {
    pub(super) input: &'static str,
    pub(super) extra: &'static str,
    pub(super) passes: Vec<PassNames>,
    /// Each target's texture names (#412): two for history (ping-ponged),
    /// else the same one twice.
    pub(super) targets: Vec<[&'static str; 2]>,
}

impl BindNames {
    /// Every name the chain's bind cache keeps a group under. The pass
    /// labels and target textures aren't cache keys.
    pub(super) fn bind_groups(&self) -> impl Iterator<Item = &'static str> + '_ {
        [self.input, self.extra]
            .into_iter()
            .chain(self.passes.iter().flat_map(|p| [p.frame, p.targets]))
    }
}

#[derive(Clone, Copy)]
pub(super) struct PassNames {
    pub(super) frame: &'static str,
    /// Group 3 of a pass that reads targets: the images, then its targets.
    pub(super) targets: &'static str,
    /// The render pass label (`--gpu-time` reads the style's own).
    pub(super) pass: &'static str,
}

/// One message from the passes' errors, or `None` when every pass builds.
pub(super) fn join_errors(errors: &[Option<String>]) -> Option<String> {
    let all: Vec<&str> = errors.iter().flatten().map(String::as_str).collect();
    (!all.is_empty()).then(|| all.join("\n"))
}

/// The folders a scene finds styles in, nearest first: `styles/` in the
/// scene's directory, in each directory above it up to the project root,
/// then in the mounted engine (ADR 0114; [`AssetKind::Styles`]).
pub fn style_dirs(scope: &AssetScope) -> Vec<PathBuf> {
    scope.search_dirs(AssetKind::Styles)
}

/// A `.shader.toml` that didn't load (#425): it registered nothing, so no
/// [`Loaded`] holds its error.
#[derive(Clone)]
pub(crate) struct Failure {
    /// The style its file names (`x` for `x.shader.toml`).
    pub(super) name: String,
    pub(super) message: String,
    /// The file's modified time when it was read: the reload poll tries it
    /// again only once that moves, so a broken file reports once.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) modified: Option<std::time::SystemTime>,
}

/// The user styles' half of a [`StyleRegistry`] (#274, #742): loading a
/// scene's `.shader.toml`s, their hot reload and their errors.
impl StyleRegistry {
    /// The built-ins (and any game styles of `self`) with the user styles
    /// `scope` can see: what a host draws a scene with. A style `self`
    /// already loaded from the same file, unchanged since, is shared rather
    /// than read again, so a transition between two scenes of one folder
    /// keeps its built passes; one only another scene could see is not in
    /// the result, so it is neither drawn nor named there (#426). Returns
    /// the registry and [`load_for`](Self::load_for)'s messages.
    pub fn for_scene(&self, scope: &AssetScope) -> (StyleRegistry, Vec<String>) {
        let mut next = self.natives();
        let mut messages = Vec::new();
        for listed in scope.list(AssetKind::Styles) {
            if let Some(kept) = self.unchanged_from(&listed.path) {
                if next.find(kept.source.name.as_str()).is_none() {
                    next.styles.push(Style::user(kept));
                }
                continue;
            }
            if let Err(e) = next.load_file(&listed.path, &mut messages) {
                messages.push(e);
            }
        }
        (next, messages)
    }

    /// [`for_scene`](Self::for_scene) for a scene at `scene_path`, its
    /// project found from there.
    pub fn for_scene_path(&self, scene_path: &Path) -> (StyleRegistry, Vec<String>) {
        self.for_scene(&AssetScope::for_scene(scene_path))
    }

    /// The user style loaded from `path` whose file hasn't moved since.
    pub(super) fn unchanged_from(&self, path: &Path) -> Option<Arc<Loaded>> {
        let loaded = self.loaded_from(path)?;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let then = *loaded
                .toml_modified
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if modified(path) != then {
                return None;
            }
        }
        Some(loaded)
    }

    /// [`load_for`](Self::load_for) for a host that kept only the scene's
    /// directory: the project is found from it.
    pub fn load_dir(&mut self, scene_dir: &Path) -> Vec<String> {
        self.load_for(&AssetScope::for_dir(scene_dir))
    }

    /// Register every user style a scene can see: each `*.shader.toml` the
    /// project's one search lists from `scope`
    /// ([`Project::list`](flint_core::project::Project::list)). Call it
    /// before the scene's `[post_process]` is read against this registry,
    /// or its tables name an unknown style. Returns one message for each
    /// file that couldn't be loaded (and each warning a check raised); a
    /// style whose shader fails still registers, drawing a passthrough.
    ///
    /// Which of two styles of one name wins (#426): the nearest to the
    /// scene, as for every asset. In one `styles/` folder a style is its
    /// file's name, so there is one of a name. A style this registry holds
    /// from another file (loaded for another scope) is replaced in place,
    /// as a hot reload does; a host that moves between scenes takes a
    /// fresh registry with [`for_scene`](Self::for_scene) instead, which
    /// holds only what the new scene sees.
    pub fn load_for(&mut self, scope: &AssetScope) -> Vec<String> {
        let mut messages = Vec::new();
        for listed in scope.list(AssetKind::Styles) {
            if let Err(e) = self.load_file(&listed.path, &mut messages) {
                messages.push(e);
            }
        }
        messages
    }

    /// Register the style `path` describes (see [`load_for`](Self::load_for)):
    /// new, or in place of the style of its name loaded from another file.
    ///
    /// A file that doesn't read (a TOML typo, a `name` that isn't the
    /// file's, an unknown input, a default outside its range, the name of a
    /// game's style) registers nothing: its error is kept by path for
    /// [`errors`](Self::errors) until the file loads or goes (#425), so the
    /// host's card shows it as it shows a shader that fails.
    pub fn load_file(&mut self, path: &Path, messages: &mut Vec<String>) -> Result<(), String> {
        // Loaded from this very file: a changed one is the reload's to read
        if self.loaded_from(path).is_some() {
            return Ok(());
        }
        // Stamped before the read, so a save during it is read at the next poll
        #[cfg(not(target_arch = "wasm32"))]
        let stamp = modified(path);
        let read = read_source(path).and_then(|source| {
            if self.is_game_style(&source.name) {
                Err(format!(
                    "{}: \"{}\" is a style the game registers in Rust (#419); pick another name",
                    shown(path),
                    source.name
                ))
            } else {
                Ok(source)
            }
        });
        let source = match read {
            Ok(source) => source,
            Err(e) => {
                let failure = Failure {
                    name: AssetKind::Styles
                        .info()
                        .name_of(path)
                        .unwrap_or_else(|| shown(path)),
                    message: e.clone(),
                    #[cfg(not(target_arch = "wasm32"))]
                    modified: stamp,
                };
                self.failed.insert(path.to_path_buf(), failure);
                return Err(e);
            }
        };
        self.failed.remove(path);
        match self.user(&source.name) {
            Some(existing) => {
                tracing::info!(
                    "style '{}': {} takes over from {} (the nearest styles/ to the scene wins)",
                    source.name,
                    shown(path),
                    shown(&existing.source.toml_path)
                );
                self.install(source, messages, true);
            }
            None => self.install(source, messages, false),
        }
        Ok(())
    }

    /// What was loaded for user style `name`.
    pub(crate) fn user(&self, name: &str) -> Option<Arc<Loaded>> {
        self.style(name).and_then(Style::loaded).cloned()
    }

    /// Every user style this registry holds, in load order.
    pub(super) fn users(&self) -> impl Iterator<Item = &Arc<Loaded>> + '_ {
        self.styles.iter().filter_map(Style::loaded)
    }

    /// The loaded style whose description is the file at `path`.
    pub(super) fn loaded_from(&self, path: &Path) -> Option<Arc<Loaded>> {
        // The spelling first: the reload poll asks this of every file in
        // every style folder, a few times a second on the frame thread, and
        // lists them from the folders the load read them from, so a loaded
        // file is spelled the same. `same_path` on a mismatch asks the
        // working directory and the disk (`anchor`): with the frame's other
        // threads busy on files that took 16-37 ms a style and stopped the
        // looks tour 150-200 ms a poll (#1121)
        self.users()
            .find(|l| l.source.toml_path == path)
            .or_else(|| {
                self.users()
                    .find(|l| flint_core::project::same_path(&l.source.toml_path, path))
            })
            .cloned()
    }

    /// Make `source`'s descriptor and register it: appended, or with
    /// `replace`, put where the style of the same name stands (#275).
    pub(super) fn install(
        &mut self,
        source: StyleSource,
        messages: &mut Vec<String>,
        replace: bool,
    ) {
        let loaded = Arc::new(build_loaded(source, messages));
        let at = self
            .styles
            .iter()
            .position(|s| replace && s.is_user() && s.name == loaded.descriptor.name);
        match at {
            Some(i) => {
                let old = std::mem::replace(&mut self.styles[i], Style::user(loaded));
                // The old install's groups bind the old pass's buffers and no
                // one asks for its keys again: every cache drops them at its
                // next frame, not at the next resize (#427)
                if let Some(old) = old.loaded() {
                    crate::bind_cache::retire(old.names.bind_groups());
                }
            }
            None => self.styles.push(Style::user(loaded)),
        }
    }

    /// Hot reload of the descriptions (#275), natively: read again every
    /// loaded `.shader.toml` whose modified time moved, and load any new
    /// one in the style folders `scope` searches ([`style_dirs`]), by the
    /// same rule a load follows: a file that appears nearer to the scene
    /// than the style of its name that is loaded takes over, and when it
    /// goes the next one out is loaded again. A description that no longer
    /// reads keeps the style as it was and shows the error
    /// ([`errors`](Self::errors)); one that does replaces the style at its
    /// index, so the renderer builds it again on the next frame once it is
    /// handed this registry. The WGSL itself is polled by the pass, so a
    /// shader edit needs nothing from the host.
    ///
    /// Costs one `stat` a loaded style and one directory listing a style
    /// folder: call it a few times a second, not every frame. On the web it
    /// does nothing.
    pub fn reload_changed_for(&mut self, scope: &AssetScope) -> Reload {
        // Filled natively; the web returns it empty
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut reload = Reload::default();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let current: Vec<Arc<Loaded>> = self.users().cloned().collect();
            for l in current {
                let now = modified(&l.source.toml_path);
                if now.is_none()
                    || now == *l.toml_modified.lock().unwrap_or_else(|e| e.into_inner())
                {
                    continue;
                }
                match read_source(&l.source.toml_path) {
                    Ok(source) => {
                        let name = source.name.clone();
                        self.install(source, &mut reload.messages, true);
                        tracing::info!("style '{name}': {} reloaded", shown(&l.source.toml_path));
                        reload.replaced.push(name);
                    }
                    Err(e) => {
                        // Keep drawing what was there; don't read it again
                        // until it changes once more
                        reload.messages.push(e.clone());
                        *l.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                        *l.toml_modified.lock().unwrap_or_else(|e| e.into_inner()) = now;
                    }
                }
            }
            self.reload_rechosen(&mut reload);
            let before: Vec<(String, PathBuf)> = self
                .users()
                .map(|l| (l.source.name.clone(), l.source.toml_path.clone()))
                .collect();
            // A file that didn't load and is gone takes its error with it (#425)
            self.failed.retain(|path, _| modified(path).is_some());
            // New files; one that failed is tried again only once it changes,
            // so a broken new file reports once, not four times a second (the
            // load that first refused it, `load_for`'s included, stamped it)
            // The folders in the search's order, the nearest file of a name the
            // one that counts: `Project::list`'s rule, read through
            // `file_watch` so the poll never waits on the disk (#633)
            let mut seen = std::collections::HashSet::new();
            // The folders are the scope's paths alone, existing or not, so
            // they are worked out once a scope rather than every poll: the
            // working out asks the working directory and the disk
            if self.poll_dirs.as_ref().is_none_or(|(s, _)| s != scope) {
                self.poll_dirs = Some((scope.clone(), style_dirs(scope)));
            }
            let dirs = self
                .poll_dirs
                .as_ref()
                .map(|(_, d)| d.clone())
                .unwrap_or_default();
            for dir in dirs {
                let Some(mut entries) = flint_core::file_watch::list(&dir) else {
                    continue;
                };
                entries.sort();
                for (path, _) in entries {
                    let Some(name) = AssetKind::Styles.info().name_of(&path) else {
                        continue;
                    };
                    if !seen.insert(name) || self.loaded_from(&path).is_some() {
                        continue;
                    }
                    let now = modified(&path);
                    let refused_at = self.failed.get(&path).map(|f| f.modified);
                    if refused_at == Some(now) {
                        continue;
                    }
                    if let Err(e) = self.load_file(&path, &mut reload.messages) {
                        reload.messages.push(e);
                    }
                }
            }
            for l in self.users() {
                let name = &l.source.name;
                match before.iter().find(|(n, _)| n == name) {
                    None => reload.added.push(name.clone()),
                    // Another file took the name over: its parameters may differ
                    Some((_, path)) if *path != l.source.toml_path => {
                        if !reload.replaced.contains(name) {
                            reload.replaced.push(name.clone());
                        }
                    }
                    Some(_) => {}
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = scope;
        reload
    }

    /// The image options' folders (#558): a style with an image option that
    /// found its choices in a folder is installed again, the description
    /// read again, when the poll lists other images there than it last did,
    /// so a PNG dropped into `luts/` is offered (F4, `--style`, a scene's
    /// table) and one deleted is not. Its parameters can't have changed,
    /// so it goes in [`Reload::rechosen`], not `replaced`: the host keeps
    /// its settings. A style the `.shader.toml` poll just read is passed
    /// over: its install listed the folder.
    #[cfg(not(target_arch = "wasm32"))]
    fn reload_rechosen(&mut self, reload: &mut Reload) {
        let current: Vec<Arc<Loaded>> = self.users().cloned().collect();
        for l in current {
            if reload.replaced.contains(&l.source.name) {
                continue;
            }
            let listed = {
                let mut folders = l.folders.lock().unwrap_or_else(|e| e.into_inner());
                let mut changed = false;
                for (option, last) in folders.iter_mut() {
                    let Some((_, o)) = l.source.options.iter().find(|(n, _)| n == option) else {
                        continue;
                    };
                    let Some(now) = listed_choices(l.source.dir(), &o.default) else {
                        continue;
                    };
                    if now != *last {
                        *last = now;
                        changed = true;
                    }
                }
                if !changed {
                    continue;
                }
                folders.clone()
            };
            match read_source(&l.source.toml_path) {
                Ok(source) => {
                    let name = source.name.clone();
                    self.install(source, &mut reload.messages, true);
                    // What the poll saw, not what the load's read listed: the
                    // next poll compares with the same answer it gave
                    if let Some(new) = self.user(&name) {
                        *new.folders.lock().unwrap_or_else(|e| e.into_inner()) = listed;
                    }
                    tracing::info!("style '{name}': an image option's folder changed");
                    reload.rechosen.push(name);
                }
                // The description itself doesn't read: the `.shader.toml`
                // poll says so when it is saved again
                Err(e) => reload.messages.push(e),
            }
        }
    }

    /// [`reload_changed_for`](Self::reload_changed_for) for a host that kept
    /// only the scene's directory. The project is found from it each call:
    /// a host that polls holds an [`AssetScope`].
    pub fn reload_changed(&mut self, scene_dir: &Path) -> Reload {
        #[cfg(not(target_arch = "wasm32"))]
        return self.reload_changed_for(&AssetScope::for_dir(scene_dir));
        #[cfg(target_arch = "wasm32")]
        {
            let _ = scene_dir;
            Reload::default()
        }
    }

    /// Every user style with its current error, for a host to show (a
    /// console line, an overlay): `(style, message)`. First each
    /// `.shader.toml` that didn't load (#425: it registered nothing, so the
    /// scene's table for it names an unknown style; the style is the one
    /// its file names, and the message starts with the file's path), then
    /// each loaded style whose shader, images or description fail. Empty
    /// when all of them load and compile.
    pub fn errors(&self) -> Vec<(String, String)> {
        let mut all = self.load_errors();
        all.extend(self.compile_errors());
        all
    }

    /// The `.shader.toml` files that didn't load, as [`errors`](Self::errors)
    /// lists them: a file's entry stays until it loads or is deleted (the
    /// reload poll notices both).
    pub fn load_errors(&self) -> Vec<(String, String)> {
        self.failed
            .values()
            .map(|f| (f.name.clone(), f.message.clone()))
            .collect()
    }

    /// The loaded styles' errors, as [`errors`](Self::errors) lists them.
    pub(super) fn compile_errors(&self) -> Vec<(String, String)> {
        self.users()
            .filter_map(|l| {
                let shader = l.error.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let images = l
                    .image_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let error = match (shader, images) {
                    (Some(s), Some(i)) => format!("{s}\n{i}"),
                    (s, i) => s.or(i)?,
                };
                Some((l.source.name.clone(), error))
            })
            .collect()
    }

    /// Record that user style `name`'s pass panicked while it was built
    /// (the renderer caught it): it stays off, and its error says so.
    pub fn report_build_panic(&self, name: &str) {
        if let Some(l) = self.user(name) {
            let e = format!(
                "{}: building the style panicked; it stays off until its files change",
                shown(&l.source.toml_path)
            );
            tracing::warn!("style '{name}': {e}");
            *l.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
        }
    }

    /// The source user style `name` was read from.
    pub fn source(&self, name: &str) -> Option<StyleSource> {
        self.user(name).map(|l| l.source.clone())
    }
}

/// What [`StyleRegistry::reload_changed_for`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reload {
    /// Styles whose `.shader.toml` was read again and replaced (new
    /// parameters, a new stage, another shader). A host re-reads its
    /// scene's `[post_process.styles.<name>]` for each, since the tables
    /// were checked against the old parameters.
    pub replaced: Vec<String>,
    /// Styles found in a style folder for the first time.
    pub added: Vec<String>,
    /// Styles installed again because an image option's folder holds other
    /// images now (#558): the same parameters with other choices, so a host
    /// hands the renderer the registry but keeps the settings it has.
    pub rechosen: Vec<String>,
    /// Load errors and check warnings, one line each.
    pub messages: Vec<String>,
}

impl Reload {
    /// Whether the registry changed.
    pub fn changed(&self) -> bool {
        !self.replaced.is_empty() || !self.added.is_empty() || !self.rechosen.is_empty()
    }
}

/// An image option's choices as the reload poll sees its folder now
/// ([`image_choices`] over [`flint_core::file_watch::list`], so the frame
/// never waits on the disk, #633); `None` when the folder can't be listed
/// (or hot reload is off), which changes nothing.
#[cfg(not(target_arch = "wasm32"))]
fn listed_choices(dir: &Path, default: &str) -> Option<Vec<String>> {
    let entries = flint_core::file_watch::list(&dir.join(choice_folder(default)))?;
    let names = entries
        .into_iter()
        .filter(|(_, is_dir)| !is_dir)
        .filter_map(|(path, _)| Some(path.file_name()?.to_str()?.to_string()));
    Some(image_choices(default, names))
}

/// Re-read the settings of each style `reload` touched from `authored`
/// (the scene's `[post_process]`, parsed again now that the registry
/// knows their new parameters) into `config`, leaving every other style as
/// the host has it (#275).
pub fn refresh_settings(
    config: &mut crate::postprocess::PostProcessConfig,
    authored: &crate::postprocess::PostProcessConfig,
    reload: &Reload,
) {
    for name in reload.replaced.iter().chain(&reload.added) {
        config.styles.restore(name, &authored.styles);
    }
}

/// Read and check the description at `path` (not its shader).
pub fn read_source(path: &Path) -> Result<StyleSource, String> {
    let text =
        flint_core::vfs::read_to_string(path).map_err(|e| format!("{}: {e}", shown(path)))?;
    parse_source(&text, path)
}

/// Check `text` as the description at `path` (which needn't exist yet).
pub fn parse_source(text: &str, path: &Path) -> Result<StyleSource, String> {
    let def: ShaderStyleDef = toml::from_str(text).map_err(|e| format!("{}: {e}", shown(path)))?;
    def.check(path)
}
/// Check the style at `path` the way a host loads it, its description and
/// then its shader, without a GPU and without registering it (#416:
/// `flint style check`). `Err` is the message a host would show.
#[cfg(not(target_arch = "wasm32"))]
///
/// Every pass is checked (#412): the errors of all that fail, one after
/// another, or one report adding up the passes' loops, reads and warnings
/// (the description's notes among them). `guarded` is `None`: there's no
/// one module to build.
pub fn check_file(path: &Path) -> Result<Report, String> {
    let source = read_source(path)?;
    let mut total = Report {
        warnings: source.notes.clone(),
        ..Report::default()
    };
    let mut errors = Vec::new();
    for (i, pass) in source.passes.iter().enumerate() {
        let user = match flint_core::vfs::read_to_string(&pass.wgsl_path) {
            Ok(user) => user,
            Err(e) => {
                errors.push(format!("{}: {e}", shown(&pass.wgsl_path)));
                continue;
            }
        };
        match check_pass(&source, i, &user) {
            Ok(report) => {
                total.loops += report.loops;
                total.bounded += report.bounded;
                total.texture_reads += report.texture_reads;
                total.warnings.extend(report.warnings);
            }
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        if source.passes.len() == 1 {
            // One pass: its own report, guarded text and all
            let user = flint_core::vfs::read_to_string(&source.wgsl_path)
                .map_err(|e| format!("{}: {e}", shown(&source.wgsl_path)))?;
            let mut report = check(&source, &user)?;
            report.warnings.splice(0..0, source.notes.iter().cloned());
            return Ok(report);
        }
        Ok(total)
    } else {
        Err(errors.join("\n"))
    }
}

/// The modified time of `path` on disk, for the reload polls (native
/// only: hot reload reads the disk, never the vfs, and a bundle never
/// changes). Through `flint_core::file_watch`, which in the player never
/// waits on the disk (#633): the stamp taken at a load and the one a poll
/// compares it with must come from the same place.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn modified(path: &Path) -> Option<std::time::SystemTime> {
    flint_core::file_watch::modified(path)
}

/// Whether user WGSL calls `flint_style_blend` outside a line comment.
pub(super) fn calls_style_blend(user: &str) -> bool {
    user.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("");
        code.contains("flint_style_blend")
    })
}

/// Make `source`'s descriptor and what goes with it (#742: owned, freed
/// with the last registry and pass that hold it; its names interned).
pub(super) fn build_loaded(source: StyleSource, messages: &mut Vec<String>) -> Loaded {
    // A shader that fails registers anyway: it draws a passthrough and its
    // error waits for the host, so a typo never takes a style's settings away
    messages.extend(source.notes.iter().cloned());
    let wgsl: Vec<_> = source
        .passes
        .iter()
        .map(|p| flint_core::vfs::read_to_string(&p.wgsl_path))
        .collect();
    // A shader that reads its own blend draws it (#396): the framework's
    // cross-fade would apply it twice. Decided when the description is
    // read, so a shader that starts or stops calling it takes effect at the
    // next load or `.shader.toml` edit. Any pass of several may (#412)
    let blends_itself = wgsl
        .iter()
        .any(|w| w.as_ref().is_ok_and(|user| calls_style_blend(user)));
    let pass_errors: Vec<Option<String>> = wgsl
        .into_iter()
        .enumerate()
        .map(|(i, w)| match w {
            Err(e) => Some(format!("{}: {e}", shown(&source.passes[i].wgsl_path))),
            Ok(user) => match check_pass(&source, i, &user) {
                Ok(report) => {
                    messages.extend(report.warnings);
                    None
                }
                Err(e) => Some(e),
            },
        })
        .collect();
    let error = join_errors(&pass_errors);
    messages.extend(pass_errors.into_iter().flatten());
    // Every image it may bind is there to read (a bundle that didn't pack
    // one is refused, #413); the pass decodes them when it first draws
    let mut missing_images = Vec::new();
    for image in &source.images {
        let files: Vec<&str> = match &image.file {
            Some(file) => vec![file.as_str()],
            None => source
                .options
                .iter()
                .find(|(n, _)| *n == image.name)
                .map(|(_, o)| o.choices.iter().map(String::as_str).collect())
                .unwrap_or_default(),
        };
        for file in files {
            let path = source.dir().join(file);
            if !flint_core::vfs::is_file(&path) {
                missing_images.push(format!(
                    "{}: image not found (style \"{}\", image {})",
                    shown(&path),
                    source.name,
                    image.name
                ));
            }
        }
    }
    messages.extend(missing_images.iter().cloned());
    let image_error = (!missing_images.is_empty()).then(|| missing_images.join("\n"));
    let leak = |s: String| -> &'static str { intern(&s) };
    // Each setting's label, help and `integer` ride on it (#510); a
    // colour's on its `_r` channel, as a built-in style's do
    let params: Vec<StyleParam> = source
        .params
        .iter()
        .map(|(name, p)| {
            let mut param = StyleParam::new(leak(name.clone()), p.default, p.min..=p.max);
            if let Some(label) = &p.label {
                param = param.label(leak(label.clone()));
            }
            if let Some(help) = &p.help {
                param = param.help(leak(help.clone()));
            }
            if p.integer {
                param = param.integer();
            }
            param
        })
        .collect();
    let options: Vec<StyleOption> = source
        .options
        .iter()
        .map(|(name, o)| {
            let choices: Vec<&'static str> = o.choices.iter().map(|c| leak(c.clone())).collect();
            // A fixed list (an image option's found once, here): closed, so
            // a scene naming another value is warned at load (#318)
            let mut option = StyleOption::new(
                leak(name.clone()),
                leak(o.default.clone()),
                intern_list(&choices),
            );
            if let Some(label) = &o.label {
                option = option.label(leak(label.clone()));
            }
            if let Some(help) = &o.help {
                option = option.help(leak(help.clone()));
            }
            option
        })
        .collect();
    let name = leak(source.name.clone());
    let mut built = StyleDescriptor::new(name, leak(source.label.clone()), source.stage, create);
    if let Some(component) = &source.component {
        built = built.component(leak(component.clone()));
    }
    built.params = List::from(params);
    built.options = List::from(options);
    let built = built
        .uses_mask(if source.inputs.mask {
            MaskUse::Always
        } else {
            MaskUse::Never
        })
        .needs(if source.inputs.edges {
            &[StyleNeed::Edges]
        } else {
            &[]
        })
        .feeds_bloom(source.feeds_bloom);
    let mut descriptor = if blends_itself {
        built.blends_itself()
    } else {
        built
    };
    // Each install is a generation of its own, which the chain's bind cache
    // keys mix in (`StyleFrame::bind`): a replaced style (#275) builds new
    // buffers and layouts, and a group kept for the old install would bind
    // the old pass's buffers (frozen at the old values, or the wrong size
    // for a new param count). A counter, from 1: 0 is a native style's
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    descriptor.generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let several = source.passes.len() > 1;
    let names = BindNames {
        input: leak(format!("{name} style input BG")),
        extra: leak(format!("{name} style depth/mask BG")),
        passes: source
            .passes
            .iter()
            .map(|p| PassNames {
                frame: leak(if several {
                    format!("{name} style {} frame BG", p.name)
                } else {
                    format!("{name} style frame BG")
                }),
                targets: leak(format!("{name} style {} targets BG", p.name)),
                pass: leak(if several {
                    format!("{name} style {}", p.name)
                } else {
                    format!("{name} style pass")
                }),
            })
            .collect(),
        // A target is keyed by name and format, and one asked for again at
        // the same size is the same texture (#412)
        targets: source
            .targets
            .iter()
            .map(|t| {
                let base = format!("{name}.{}", t.name);
                if t.history {
                    [leak(format!("{base}.a")), leak(format!("{base}.b"))]
                } else {
                    let one = leak(base);
                    [one, one]
                }
            })
            .collect(),
    };
    Loaded {
        descriptor,
        #[cfg(not(target_arch = "wasm32"))]
        toml_modified: Mutex::new(modified(&source.toml_path)),
        #[cfg(not(target_arch = "wasm32"))]
        folders: Mutex::new(
            source
                .options
                .iter()
                .filter(|(_, o)| o.discovered)
                .map(|(name, o)| (name.clone(), o.choices.clone()))
                .collect(),
        ),
        source,
        names,
        error: Mutex::new(error),
        image_error: Mutex::new(image_error),
    }
}
