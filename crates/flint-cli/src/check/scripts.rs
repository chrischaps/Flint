//! The scripts pass (#694): every `.rhai` a scene runs, every one under a
//! project, and the modules they import, held to the script API by
//! `flint_script::check`. `flint script check` runs only this; `flint
//! check` and `flint validate` run it beside their other checks
//! ([`super::Checks::scripts`]).
//!
//! A game's binary registers functions the engine's API doesn't list (its
//! extensions). [`ScriptApiArgs`] lets them pass: `--allow <name>`, or
//! `--api <file>` with the game's own `flint script api --json` listing.

use flint_core::diagnostic::{codes, Diagnostic};
use flint_core::project::{shown, AssetKind, Project};
use flint_core::vfs;
use flint_script::check::{Api, Checker, ScriptFile};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The flags that name a game's own script functions.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct ScriptApiArgs {
    /// A script function the game binary registers (an extension's) that
    /// the engine's API doesn't list: the script check lets any call of it
    /// pass (repeatable)
    #[arg(long = "allow", value_name = "NAME", action = clap::ArgAction::Append)]
    pub allow: Vec<String>,

    /// A game's own `flint script api --json` listing, from its binary
    /// (extensions included): the script check knows its functions at
    /// their argument counts (repeatable)
    #[arg(long = "api", value_name = "FILE", action = clap::ArgAction::Append)]
    pub api: Vec<PathBuf>,
}

impl ScriptApiArgs {
    /// The engine's API with these names added. A listing that doesn't read
    /// is the error, by name.
    pub fn api(&self) -> anyhow::Result<Api> {
        let mut api = engine_api().clone();
        for path in &self.api {
            add_listing(&mut api, path)?;
        }
        for name in &self.allow {
            api.allow(name.clone());
        }
        Ok(api)
    }
}

/// The engine's own API and Rhai's packages, built once a process.
pub fn engine_api() -> &'static Api {
    static API: OnceLock<Api> = OnceLock::new();
    API.get_or_init(|| Api::of(&mut flint_script::ScriptSystem::new()))
}

/// Add every function of a `flint script api --json` listing.
fn add_listing(api: &mut Api, path: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("--api {}: can't read it", shown(path)))?;
    let doc: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .with_context(|| format!("--api {}: not JSON", shown(path)))?;
    let functions = doc["functions"].as_array().with_context(|| {
        format!(
            "--api {}: no `functions` list; it is what `flint script api --json` prints",
            shown(path)
        )
    })?;
    for f in functions {
        let (Some(name), Some(params)) = (f["name"].as_str(), f["params"].as_array()) else {
            anyhow::bail!(
                "--api {}: a function without a `name` and `params`",
                shown(path)
            );
        };
        api.add(name, params.len());
    }
    Ok(())
}

/// The scripts the entities of `scene` run, found as the player finds them
/// (the project's one search from the scene's folder), and a
/// `missing-asset` for each source that isn't there when `report_missing`.
pub(super) fn of_scene(
    scene: &Path,
    registry: &flint_schema::SchemaRegistry,
    report_missing: bool,
) -> (Vec<ScriptFile>, Vec<Diagnostic>) {
    let project = Project::for_scene(scene);
    let scene_dir = scene
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut sources = sources(scene, registry);
    sources.sort();
    sources.dedup();
    let (mut files, mut missing) = (Vec::new(), Vec::new());
    for (entity, source) in sources {
        let answer = project.explain(AssetKind::Scripts, &source, scene_dir);
        match answer.found {
            Some(path) => files.push(ScriptFile::in_scene(path, project.clone(), scene_dir)),
            None if report_missing => missing.push(
                Diagnostic::error(
                    codes::MISSING_ASSET,
                    format!("script `{source}` is not found"),
                )
                .file(scene)
                .entity(entity)
                .component("script")
                .field("source")
                .tried(answer.tried)
                .hint(format!(
                    "`flint asset where scripts {source} --scene {}` shows the search",
                    shown(scene)
                )),
            ),
            None => {}
        }
    }
    (files, missing)
}

/// Each `(entity, script.source)` of a scene: from the loaded world, so a
/// prefab's scripts are there, else from the file as written.
fn sources(scene: &Path, registry: &flint_schema::SchemaRegistry) -> Vec<(String, String)> {
    let enabled_source = |data: &toml::Value| -> Option<String> {
        let enabled = data
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let source = data.get("source")?.as_str()?;
        (enabled && !source.is_empty()).then(|| source.to_string())
    };
    if let Ok((world, _, _)) = flint_scene::load_scene_diagnosed(scene, registry) {
        let mut out = Vec::new();
        for &id in world.entities_with_component(flint_core::components::SCRIPT) {
            let data = world
                .get_components(id)
                .and_then(|c| c.get(flint_core::components::SCRIPT).cloned());
            if let Some(source) = data.as_ref().and_then(enabled_source) {
                let name = world.get_name(id).unwrap_or_default().to_string();
                out.push((name, source));
            }
        }
        return out;
    }
    let Ok(text) = vfs::read_to_string(scene) else {
        return Vec::new();
    };
    let Ok(file) = toml::from_str::<flint_scene::SceneFile>(&text) else {
        return Vec::new();
    };
    file.entities
        .iter()
        .filter_map(|(name, def)| {
            let source = enabled_source(def.components.get("script")?)?;
            Some((name.clone(), source))
        })
        .collect()
}

/// Check `files` (and what they import) against `api`. The paths are
/// spelled from the working directory with `/`, as the rest of a report.
pub fn check(api: &Api, files: Vec<ScriptFile>) -> (Vec<Diagnostic>, Vec<String>) {
    let checked = Checker::new(api.clone()).check(files);
    let spelled = |path: &Path| PathBuf::from(shown(&super::from_here(path)));
    let diagnostics = checked
        .diagnostics
        .into_iter()
        .map(|mut d| {
            d.file = d.file.map(|f| spelled(&f));
            d.tried = d.tried.iter().map(|p| spelled(p)).collect();
            d
        })
        .collect();
    let read = checked
        .files
        .iter()
        .map(|p| shown(&super::from_here(p)))
        .collect();
    (diagnostics, read)
}
