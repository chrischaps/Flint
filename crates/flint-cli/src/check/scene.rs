//! What `flint check` asks of one scene: that it loads, that its entities
//! fit the schemas, that every asset it names is where the engine will look,
//! that its styles, look, `[wooze]` and `[living]` tables are right, and
//! that it keeps the project's constraints. Everything is read through
//! `flint_core::vfs`, so a scene inside a packed bundle is checked as one on
//! the disk is.

use super::{Checks, SceneSchemas};
use flint_asset::HeldAssets;
use flint_constraint::{ConstraintEvaluator, BAD_CONSTRAINT};
use flint_core::diagnostic::{self, codes, Diagnostic, Severity};
use flint_core::project::{shown, AssetKind, Project};
use flint_core::vfs;
use flint_ecs::FlintWorld;
use flint_scene::{load_scene_diagnosed, SceneFile};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The component fields that name an asset, and the kind the engine looks
/// for: what the loaders ask the project's one resolver (ADR 0114).
///
/// `flint_scene::diagnose` follows the fields a schema types `asset` (#684);
/// this pass asks more of these, as the player does (the asset catalog and a
/// procgen spec first, `model.runtime`, one diagnostic a name), so the
/// scene pass's `missing-asset` for one of them is left to it.
pub(super) const ASSET_FIELDS: &[(&str, &str, AssetKind)] = &[
    ("model", "asset", AssetKind::Models),
    ("material", "texture", AssetKind::Textures),
    ("sprite", "texture", AssetKind::Textures),
    ("audio_source", "file", AssetKind::Audio),
    ("script", "source", AssetKind::Scripts),
    ("terrain", "heightmap", AssetKind::Files),
    ("terrain", "splat_map", AssetKind::Files),
    ("grass_ground", "mask", AssetKind::Files),
    ("spline", "file", AssetKind::Files),
    ("particle_emitter", "texture", AssetKind::ParticleTextures),
];

/// The scene tables' own keys that name a file found the same way.
const SCENE_FILES: &[(&str, &str)] = &[("environment", "skybox"), ("scene", "input_config")];

/// Whether the key path `at` of a scene document is one this pass asks the
/// resolver about, so the bare-string pass leaves it alone.
pub(super) fn is_asset_field(at: &[String]) -> bool {
    match at {
        [first, _entity, component, field] if first == "entities" => ASSET_FIELDS
            .iter()
            .any(|(c, f, _)| c == component && f == field),
        [table, key] => SCENE_FILES.iter().any(|(t, k)| t == table && k == key),
        _ => false,
    }
}

/// Whether a diagnostic is about one of [`ASSET_FIELDS`].
fn is_entity_asset_field(d: &Diagnostic) -> bool {
    let (Some(component), Some(field)) = (d.component.as_deref(), d.field.as_deref()) else {
        return false;
    };
    ASSET_FIELDS
        .iter()
        .any(|(c, f, _)| *c == component && *f == field)
}

/// The codes of `flint_scene::diagnose` that mean a scene does not load, or
/// loads without something a bundle must hold: what `flint bundle` has
/// always refused to pack.
fn stops_a_load(d: &Diagnostic) -> bool {
    [
        codes::TOML_PARSE,
        codes::FILE_UNREADABLE,
        codes::BAD_PREFAB,
        codes::UNKNOWN_ARCHETYPE,
    ]
    .contains(&d.code)
        || dangling_parent(d)
}

/// A `parent` the loader refuses: one naming no entity, or one closing a
/// loop (#680). Either is named with its entity, so the load's own failure
/// says nothing more.
fn dangling_parent(d: &Diagnostic) -> bool {
    (d.code == codes::UNKNOWN_ENTITY && d.component.is_none()) || d.code == codes::PARENT_CYCLE
}

/// Whether the scene can't be read at all, so there is nothing more to ask.
fn cannot_load(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|d| [codes::TOML_PARSE, codes::FILE_UNREADABLE, codes::BAD_PREFAB].contains(&d.code))
}

/// What one run remembers between scenes, so a folder of scenes pays once
/// for what they share.
#[derive(Default)]
pub struct Memo {
    /// Each scene directory's styles (#742: its own registry, the
    /// built-ins and the user styles it sees), loaded once, and what the
    /// load said until the styles check reports it (once a directory).
    style_dirs: BTreeMap<PathBuf, (flint_render::stylize::StyleRegistry, Option<Vec<String>>)>,
    /// The asset catalog of each project root: asked before the files for a
    /// model or a texture, as the player asks, and as `diagnose` does.
    catalogs: BTreeMap<PathBuf, Option<HeldAssets>>,
    /// Procgen specs whose params have been checked: each is reported
    /// once a run, however many scenes name it.
    specs_checked: BTreeSet<PathBuf>,
}

impl Memo {
    /// The styles a scene in `scene_dir` is drawn with, loaded the first
    /// time a directory is asked for; what the load said waits for the
    /// styles check.
    fn styles_for(&mut self, scene_dir: &Path) -> &flint_render::stylize::StyleRegistry {
        &self
            .style_dirs
            .entry(scene_dir.to_path_buf())
            .or_insert_with(|| {
                let scope = flint_core::project::AssetScope::for_dir(scene_dir);
                let (styles, messages) =
                    flint_render::stylize::StyleRegistry::new().for_scene(&scope);
                (styles, Some(messages))
            })
            .0
    }

    /// The styles of every folder that can take a look kept at `home`
    /// (#1272): `home`'s own first, then each scene folder under it the run
    /// has loaded. A scene finds a look by walking up, so these are the
    /// registries of the scenes that can name it.
    pub(super) fn styles_able_to_take(
        &mut self,
        home: &Path,
    ) -> Vec<&flint_render::stylize::StyleRegistry> {
        self.styles_for(home);
        let own = &self.style_dirs[home].0;
        std::iter::once(own)
            .chain(
                self.style_dirs
                    .iter()
                    .filter(|(dir, _)| dir.as_path() != home && dir.starts_with(home))
                    .map(|(_, (styles, _))| styles),
            )
            .collect()
    }
}

/// What a scene's check found, and the files it reached: the scripts,
/// prefabs and effects a single-scene check goes on to read.
#[derive(Default)]
pub struct SceneFound {
    pub diagnostics: Vec<Diagnostic>,
    pub reached: Vec<PathBuf>,
    /// How many the project's `[project.diagnostics] allow` took out (#795).
    pub allowed: usize,
}

/// Everything `checks` asks of the scene at `scene`. Never stops at the
/// first problem; a scene that can't be read at all is the one case with
/// nothing more to ask. What the scene's project allows is left out of the
/// list, and counted.
pub fn check(
    scene: &Path,
    schemas: &SceneSchemas,
    checks: Checks,
    strict: bool,
    memo: &mut Memo,
) -> SceneFound {
    let project = Project::for_scene(scene);
    let mut out = check_all(scene, &project, schemas, checks, strict, memo);
    out.allowed = diagnostic::drop_allowed(&mut out.diagnostics, &project.allow);
    out
}

/// [`check`] with nothing allowed.
fn check_all(
    scene: &Path,
    project: &Project,
    schemas: &SceneSchemas,
    checks: Checks,
    strict: bool,
    memo: &mut Memo,
) -> SceneFound {
    let mut out = SceneFound::default();
    let scene_dir = scene
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // A user style's `<name>_style` has no schema file: the schema its
    // `.shader.toml` makes stands in for none or an open one (#556), so
    // the scene's entities are held to it as to a built-in's
    let with_styles = (checks.schema || checks.styles)
        .then(|| with_style_schemas(&schemas.registry, memo.styles_for(scene_dir)))
        .flatten();
    let registry = with_styles.as_ref().unwrap_or(&schemas.registry);
    let mut found = flint_scene::diagnose_all(scene, registry, project);
    // A component or table of a package `[packages]` leaves out (#1334)
    if let Some(in_use) = &schemas.dirs.packages {
        in_use.name_unlisted(&mut found);
    }
    if with_styles.is_some() {
        say_user_styles_clamp(&mut found, memo.styles_for(scene_dir));
    }
    if checks.schema || checks.styles {
        say_unmasked_user_styles(&mut found, memo.styles_for(scene_dir));
    }
    if strict {
        diagnostic::promote_strict(&mut found);
    }
    // `diagnose` read and parsed it a moment ago. A typed table that
    // doesn't fit its type (#1061) refuses the load: the pass named it by
    // field, and the rest of the scene is checked without it
    let parsed = vfs::read_to_string(scene).ok().and_then(|text| {
        let (file, refused) = match SceneFile::parse(&text) {
            Ok(file) => (file, Vec::new()),
            Err(_) => SceneFile::parse_refusing(&text)?,
        };
        let refused: Vec<&str> = refused.into_iter().map(|(table, _)| table).collect();
        Some((text, file, refused))
    });
    let refused: &[&str] = parsed.as_ref().map_or(&[], |(_, _, refused)| refused);
    if !checks.schema {
        found.retain(|d| stops_a_load(d) || refuses_a_table(d, refused));
    }
    // This pass's own asset check asks the catalog and the specs first
    if checks.files {
        found.retain(|d| !(d.code == codes::MISSING_ASSET && is_entity_asset_field(d)));
    }
    let unreadable = cannot_load(&found);
    let no_parent = found.iter().any(dangling_parent);
    let refused = !refused.is_empty();
    out.diagnostics.extend(found);
    if unreadable {
        return out;
    }
    let Some((text, file, _)) = parsed else {
        return out;
    };

    let world = match load_scene_diagnosed(scene, registry) {
        // Said by field already
        _ if refused => None,
        Ok((world, _, _)) => Some(world),
        // A parent that isn't there, or loops, has been named with its entity
        Err(_) if no_parent => None,
        Err(e) => {
            out.diagnostics.push(
                Diagnostic::error(codes::SCENE_LOAD, e.to_string())
                    .file(scene)
                    .hint("the scene parses, and the engine still can't build it"),
            );
            None
        }
    };

    // `[living]` and `[wooze]` are living props' tables, kept raw by the
    // scene file (#686): one that doesn't fit its type (a misspelt key)
    // refuses the scene when it is played (`flint_procgen::settle_scene`),
    // as it did when it failed the file's parse: an error in every kind of
    // check, by the table's name. One mistake is said once (#807): the
    // scene pass has held the table to its `[table.<name>]` schema, which
    // the type mirrors, and named the field; that diagnostic becomes the
    // error, and serde's message stands only when the pass said nothing
    for (table, said) in flint_procgen::LivingTables::problems(&file) {
        refuses_the_load(&mut out.diagnostics, scene, table, &said);
    }
    if checks.schema {
        if let Some(world) = &world {
            particle_emitters(scene, world, &mut out.diagnostics);
        }
        spec_params(scene, scene_dir, &file, project, memo, &mut out.diagnostics);
    }
    if checks.files {
        assets(scene, scene_dir, &file, project, memo, &mut out);
    }
    if checks.styles {
        styles(scene, scene_dir, &file, &text, memo, &mut out.diagnostics);
    }
    if checks.tables {
        let wooze = wooze(scene, &file, schemas);
        // The scene pass has checked `[wooze]` against its `[table.wooze]`
        // schema like any table (#686), and this check holds it to the same
        // schema and to the field itself, one issue a zone, in the field's
        // words and by the zone's name. One mistake is said once: these
        // stay, and the pass's warnings (a key the schema doesn't declare)
        // with them.
        if !wooze.is_empty() {
            out.diagnostics
                .retain(|d| !(d.is_error() && d.table.as_deref() == Some("wooze")));
        }
        out.diagnostics.extend(wooze);
        out.diagnostics
            .extend(species(scene, &file, world.as_ref()));
        out.diagnostics.extend(fields(scene, &file));
    }
    if checks.constraints {
        out.diagnostics.extend(schemas.problems.iter().cloned());
        if let Some(world) = &world {
            out.diagnostics.extend(constraints(scene, world, schemas));
        }
    }
    flint_scene::place_table_diagnostics(&text, &mut out.diagnostics);
    out
}

/// One of the pass's errors in a typed table that refuses the load (#1061):
/// what stops the load, so it is said in every kind of check.
fn refuses_a_table(d: &Diagnostic, refused: &[&str]) -> bool {
    d.is_error() && d.entity.is_none() && d.table.as_deref().is_some_and(|t| refused.contains(&t))
}

/// The params of every procgen spec a model or texture of the scene is made
/// from, against its generator's schema (#718): the one validator `flint
/// gen` runs before generating (`flint_procgen::validate_params`), so a
/// spec is judged the same wherever it is checked. A value the generator
/// can't use is an error, a key it ignores a warning, each `spec-param` on
/// the spec's file with the parameter's path as its field. A spec found in
/// the asset catalog first, one that doesn't parse (the files pass's
/// `bad-spec`) or one whose generator is a game's own is passed over.
fn spec_params(
    scene: &Path,
    scene_dir: &Path,
    file: &SceneFile,
    project: &Project,
    memo: &mut Memo,
    out: &mut Vec<Diagnostic>,
) {
    let specs: BTreeMap<String, PathBuf> = project
        .list(AssetKind::Specs, scene_dir)
        .into_iter()
        .map(|listed| (listed.name, listed.path))
        .collect();
    if specs.is_empty() {
        return;
    }
    let catalog = memo
        .catalogs
        .entry(project.root.clone())
        .or_insert_with(|| HeldAssets::of(project));
    let in_catalog = |name: &str| catalog.as_ref().is_some_and(|c| c.holds(name));
    let to_be_made = flint_scene::declared_assets(file);
    let registry = flint_procgen::GeneratorRegistry::with_built_ins();

    let mut names: Vec<&String> = file.entities.keys().collect();
    names.sort_unstable();
    for entity in names {
        let def = &file.entities[entity];
        for (component, field, kind) in ASSET_FIELDS {
            if !matches!(kind, AssetKind::Models | AssetKind::Textures) {
                continue;
            }
            let Some(name) = def
                .components
                .get(*component)
                .and_then(|data| data.get(field))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            if to_be_made.contains(name) || in_catalog(name) {
                continue;
            }
            let Some(path) = specs.get(name) else {
                continue;
            };
            if !memo.specs_checked.insert(path.clone()) {
                continue;
            }
            let Ok(spec) = flint_procgen::ProcGenSpec::from_file(path) else {
                continue;
            };
            let Some(generator) = registry.get(&spec.generator) else {
                continue;
            };
            for issue in flint_procgen::validate_params(&generator.param_schema(), &spec.params) {
                let message = format!("procgen spec `{name}`: {issue}");
                let d = if issue.is_error() {
                    Diagnostic::error(codes::SPEC_PARAM, message)
                } else {
                    Diagnostic::warning(codes::SPEC_PARAM, message)
                };
                out.push(d.file(path).field(issue.key()).hint(format!(
                    "{} names it (entity '{entity}', {component}.{field}); \
                     `flint gen --describe {}` lists every parameter",
                    shown(scene),
                    spec.generator
                )));
            }
        }
    }
}

/// The assets the scene names, each asked of the project's one resolver
/// from the scene's directory, as its loader asks: a miss is a
/// `missing-asset` with every path tried.
fn assets(
    scene: &Path,
    scene_dir: &Path,
    file: &SceneFile,
    project: &Project,
    memo: &mut Memo,
    out: &mut SceneFound,
) {
    let catalog = memo
        .catalogs
        .entry(project.root.clone())
        .or_insert_with(|| HeldAssets::of(project));
    let in_catalog = |name: &str| catalog.as_ref().is_some_and(|c| c.holds(name));
    // An asset an `asset_def` of the scene declares is one `flint asset
    // resolve` makes: no file need be there yet (`diagnose` passes it too)
    let to_be_made = flint_scene::declared_assets(file);
    // A model or texture name may be a procgen spec's: the player builds it
    let specs: BTreeMap<String, PathBuf> = project
        .list(AssetKind::Specs, scene_dir)
        .into_iter()
        .map(|listed| (listed.name, listed.path))
        .collect();
    let missing = |kind: AssetKind, name: &str, tried: Vec<PathBuf>| {
        let info = kind.info();
        Diagnostic::error(
            codes::MISSING_ASSET,
            format!("{} `{name}` is not found", info.noun),
        )
        .file(scene)
        .tried(tried)
        .hint(format!(
            "`flint asset where {} {name} --scene {}` shows the search",
            info.label,
            shown(scene)
        ))
    };

    let mut names: Vec<&String> = file.entities.keys().collect();
    names.sort_unstable();
    let mut bad_specs = BTreeSet::new();
    let mut bad_splines = BTreeSet::new();
    // One asset missing is one problem, however many entities name it: the
    // first is the diagnostic's, the rest are counted in its message
    let mut misses: Vec<((AssetKind, String), Diagnostic, Vec<String>)> = Vec::new();
    for entity in names {
        let def = &file.entities[entity];
        for (component, field, kind) in ASSET_FIELDS {
            let Some(data) = def.components.get(*component) else {
                continue;
            };
            let Some(name) = data.get(field).and_then(|v| v.as_str()) else {
                continue;
            };
            if name.is_empty() || to_be_made.contains(name) {
                continue;
            }
            let made = matches!(kind, AssetKind::Models | AssetKind::Textures);
            // A mesh code uploads at run time has no file to find
            if *kind == AssetKind::Models && flint_render::model_loader::is_runtime_model(data) {
                continue;
            }
            if made && in_catalog(name) {
                continue;
            }
            if let Some(spec) = made.then(|| specs.get(name)).flatten() {
                // A spec that doesn't parse is skipped by the player, and
                // the name is then nobody's
                if let Err(e) = flint_procgen::ProcGenSpec::from_file(spec) {
                    if bad_specs.insert(spec.clone()) {
                        out.diagnostics.push(
                            Diagnostic::error(
                                codes::BAD_SPEC,
                                format!("the procgen spec `{name}` does not parse: {e}"),
                            )
                            .file(spec)
                            .hint(format!(
                                "{} names it (entity '{entity}', {component}.{field}); \
                                 `flint gen {} --dry-run` checks a spec",
                                shown(scene),
                                shown(spec)
                            )),
                        );
                    }
                }
                continue;
            }
            let answer = project.explain(*kind, name, scene_dir);
            match answer.found {
                Some(path) => {
                    // A spline file the loader would refuse, placed (#750)
                    if *component == "spline" && bad_splines.insert(path.clone()) {
                        if let Err(e) = flint_geometry::spline_file::SplineFile::read(&path) {
                            let mut d = Diagnostic::error(codes::BAD_SPLINE, e.message.clone())
                                .file(&path)
                                .hint(format!(
                                    "{} names it (entity '{entity}', spline.file); the \
                                         spline loads no path and its meshes are not made",
                                    shown(scene)
                                ));
                            if let (Some(line), Some(column)) = (e.line, e.column) {
                                d = d.at(line, column);
                            }
                            out.diagnostics.push(d);
                        }
                    }
                    out.reached.push(path)
                }
                None => {
                    let key = (*kind, name.to_string());
                    if let Some((_, _, others)) = misses.iter_mut().find(|(k, _, _)| *k == key) {
                        others.push(entity.clone());
                        continue;
                    }
                    let mut d = missing(*kind, name, answer.tried)
                        .entity(entity.clone())
                        .component(*component)
                        .field(*field);
                    if made {
                        d.hint = d.hint.map(|hint| {
                            format!(
                                "{hint}; the asset catalog and a procgen spec named `{name}` \
                                 were asked first"
                            )
                        });
                    }
                    misses.push((key, d, Vec::new()));
                }
            }
        }
    }
    for (_, mut d, others) in misses {
        if !others.is_empty() {
            d.message = format!("{} (also named by {})", d.message, list_of(&others));
        }
        out.diagnostics.push(d);
    }

    // The prefabs it is made from (one that is missing is a `bad-prefab`)
    let mut templates: Vec<&str> = file.prefabs.values().map(|p| p.template.as_str()).collect();
    templates.sort_unstable();
    templates.dedup();
    for template in templates {
        out.reached.extend(
            project
                .resolve(AssetKind::Prefabs, template, scene_dir)
                .ok(),
        );
    }

    // The scene tables' own files
    let skybox = file.environment.as_ref().and_then(|e| e.skybox.clone());
    if let Some(name) = skybox.filter(|name| !name.is_empty()) {
        let answer = project.explain(AssetKind::Files, &name, scene_dir);
        if answer.found.is_none() {
            out.diagnostics
                .push(missing(AssetKind::Files, &name, answer.tried).field("environment.skybox"));
        }
    }
    input_config(
        scene,
        scene_dir,
        file,
        project,
        &missing,
        &mut out.diagnostics,
    );
}

/// `a, b and 3 more`: entity names for a message, the first few.
fn list_of(names: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut shown: Vec<String> = names.iter().take(SHOWN).map(|n| format!("'{n}'")).collect();
    if names.len() > SHOWN {
        shown.push(format!("{} more", names.len() - SHOWN));
    }
    match shown.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => shown.join(""),
    }
}

/// The game layer of the input config, found as the player finds it
/// (`flint-player`'s `resolve_input_paths`): the scene's `input_config`,
/// else the manifest's `[project] input`, else `config/input.toml` if there
/// is one. It must be there when named, and it must parse.
fn input_config(
    scene: &Path,
    scene_dir: &Path,
    file: &SceneFile,
    project: &Project,
    missing: &dyn Fn(AssetKind, &str, Vec<PathBuf>) -> Diagnostic,
    found: &mut Vec<Diagnostic>,
) {
    let named = file.scene.input_config.as_deref().filter(|n| !n.is_empty());
    let path = match named {
        Some(name) => {
            let answer = project.explain(AssetKind::Files, name, scene_dir);
            if answer.found.is_none() {
                found.push(
                    missing(AssetKind::Files, name, answer.tried).field("scene.input_config"),
                );
            }
            answer.found
        }
        None => match project.input_path() {
            Some(path) if !vfs::is_file(&path) => {
                found.push(
                    Diagnostic::error(
                        codes::MISSING_ASSET,
                        format!("the input config `{}` is not there", shown(&path)),
                    )
                    .file(project.manifest_path())
                    .field("project.input")
                    .tried([path])
                    .hint("the manifest names one file, from the project root, with no search"),
                );
                None
            }
            Some(path) => Some(path),
            None => {
                project
                    .explain(AssetKind::Files, "config/input.toml", scene_dir)
                    .found
            }
        },
    };
    let Some(path) = path else {
        return;
    };
    let parsed = vfs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| {
            flint_runtime::InputConfig::from_toml_str(&text).map_err(|e| e.to_string())
        });
    if let Err(e) = parsed {
        found.push(
            Diagnostic::error(codes::BAD_INPUT_CONFIG, e)
                .file(&path)
                .hint(format!("{} plays with it", shown(scene))),
        );
    }
}

/// The user styles the scene can see, its `[post_process.styles]` tables
/// and its look: what a load would register, warn about and lay under.
fn styles(
    scene: &Path,
    scene_dir: &Path,
    file: &SceneFile,
    text: &str,
    memo: &mut Memo,
    found: &mut Vec<Diagnostic>,
) {
    // The styles are loaded either way: the tables below are read against
    // them. They are reported once a directory. Each directory's are its
    // own (#742), so a style an earlier scene of the run loaded from a
    // folder this one can't see is as unknown to it as when it is checked
    // alone (#426)
    let styles = memo.styles_for(scene_dir).clone();
    if let Some(messages) = memo
        .style_dirs
        .get_mut(scene_dir)
        .and_then(|(_, m)| m.take())
    {
        found.extend(messages.iter().map(|m| user_style_diagnostic(m, scene_dir)));
    }
    let Some(mut post) = file.post_process.clone() else {
        return;
    };
    let unseen = flint_render::stylize::StyleSettings::from_tables(&post.style_tables(), &styles).1;
    for warning in unseen {
        found.push(
            Diagnostic::warning(codes::BAD_STYLE_SETTING, warning)
                .file(scene)
                .field("post_process.styles")
                .hint("`flint render --list-styles` lists every style, key and value"),
        );
    }
    // The scene's look (#356) must be found and fit under its keys
    if let Some(name) = post.look.clone().filter(|name| !name.trim().is_empty()) {
        post.authored = flint_scene::PostProcessDef::authored_table(text);
        let resolved = flint_render::look::resolve(&post, Some(&name), scene);
        // Its tables for styles that aren't built in, against the styles
        // this scene is drawn with (#1272): the load would warn of each
        if resolved.is_ok() {
            if let Ok(look) = flint_render::look::find(&name, scene) {
                found.extend(super::looks::style_diagnostics(&look, &[&styles]));
            }
        }
        if let Err(e) = resolved {
            // A look that isn't found lists where it was looked for
            let (what, tried) = match e.split_once(" Tried:\n") {
                Some((what, tried)) => (what, tried.lines().map(|l| l.trim().into()).collect()),
                None => (e.as_str(), Vec::new()),
            };
            found.push(
                Diagnostic::error(codes::BAD_LOOK, format!("look = \"{name}\": {what}"))
                    .file(scene)
                    .field("post_process.look")
                    .tried(tried)
                    .hint("the scene is drawn without its look; `flint look list` names them"),
            );
        }
    }
}

/// A user style's settings row clamps each param (#492), so a value past
/// its range on its component draws as the range's end: the hint says so,
/// after the range the schema gives.
fn say_user_styles_clamp(found: &mut [Diagnostic], styles: &flint_render::stylize::StyleRegistry) {
    for d in found.iter_mut().filter(|d| d.code == codes::OUT_OF_RANGE) {
        let Some(style) = styles.iter().find(|s| {
            s.component.is_some()
                && s.component == d.component.as_deref()
                && styles.is_user_style(s.name)
        }) else {
            continue;
        };
        let clamped = format!("the user style '{}' draws it clamped into it", style.name);
        d.hint = Some(match d.hint.take() {
            Some(range) => format!("{range}; {clamped}"),
            None => clamped,
        });
    }
}

/// A `<name>_style` on an entity, where the user style `name` reads no
/// mask, is the warning a drawing host gives it (#1320), not an
/// `unknown-component` error asking for a schema file: the style has no
/// component, so the table does nothing.
fn say_unmasked_user_styles(
    found: &mut [Diagnostic],
    styles: &flint_render::stylize::StyleRegistry,
) {
    for d in found
        .iter_mut()
        .filter(|d| d.code == codes::UNKNOWN_COMPONENT)
    {
        let Some(component) = d.component.clone() else {
            continue;
        };
        let said = styles
            .iter()
            .filter(|s| styles.is_user_style(s.name))
            .find_map(|s| {
                flint_render::stylize::component_schema::unmasked_style_component(s, &component)
            });
        if let Some(said) = said {
            let (file, entity) = (d.file.take(), d.entity.take());
            *d = said;
            d.file = file;
            d.entity = entity;
        }
    }
}

/// `registry` with the component schema of each style `styles` holds
/// whose component it declares open or not at all (#556:
/// `flint_render::stylize::add_component_schemas`), or `None` when there
/// is none, so the registry the run loaded is used as it is.
fn with_style_schemas(
    registry: &flint_schema::SchemaRegistry,
    styles: &flint_render::stylize::StyleRegistry,
) -> Option<flint_schema::SchemaRegistry> {
    // With no schema at all the scene pass says so once (`no-schemas`).
    // The built-in styles' components would make the registry look loaded
    // and turn every engine table into an `unknown-component` error: an
    // `--engine none` project failed `flint check` that way (#1228)
    if registry.component_names().is_empty() && registry.archetype_names().is_empty() {
        return None;
    }
    let wanted = styles.iter().filter_map(|d| d.component).any(|component| {
        registry
            .get_component(component)
            .is_none_or(|schema| schema.is_open())
    });
    wanted.then(|| {
        let mut registry = registry.clone();
        flint_render::stylize::add_component_schemas(styles, &mut registry);
        registry
    })
}

/// One message of `stylize::custom::load_dir` as a diagnostic. A message
/// leads with the file it is about (and, from the shader compiler, a line
/// and a column); `: warning: ` marks one that is for the author and does
/// not stop the style loading.
fn user_style_diagnostic(message: &str, scene_dir: &Path) -> Diagnostic {
    let warning = message.contains(": warning: ");
    let (from, what) = match message.split_once(": ") {
        // The mark goes before the place is put in front (#573: a warning
        // with a line and column read "line 12, column 45: warning: ...")
        Some((from, what)) => (
            from.to_string(),
            what.strip_prefix("warning: ")
                .unwrap_or(what)
                .trim_end()
                .to_string(),
        ),
        None => (
            shown(&scene_dir.join("styles")),
            message.trim_end().to_string(),
        ),
    };
    // `styles/broken.wgsl:2:17` -> the file, and the place in the message
    let mut parts = from.rsplitn(3, ':');
    let (column, line, file) = (parts.next(), parts.next(), parts.next());
    let numbered = |s: Option<&str>| s.is_some_and(|s| s.parse::<u32>().is_ok());
    let (file, what) = match file {
        Some(file) if numbered(line) && numbered(column) => (
            file.to_string(),
            format!(
                "line {}, column {}: {what}",
                line.unwrap_or_default(),
                column.unwrap_or_default()
            ),
        ),
        _ => (from, what),
    };
    let d = if warning {
        Diagnostic::warning(codes::BAD_USER_STYLE, what)
    } else {
        Diagnostic::error(
            codes::BAD_USER_STYLE,
            format!("user style doesn't load: {what}"),
        )
        .hint("a style that doesn't load is drawn as a passthrough")
    };
    d.file(file)
}

/// What the player says of a `[living]` or `[wooze]` table that doesn't fit
/// its type: it refuses the scene. When the scene pass has already named
/// the mistake by field (an undeclared key is its `unknown-field` warning, a
/// wrong type its error), those diagnostics are made the errors, with the
/// load's hint; serde's `said` is added only when the pass found nothing,
/// so one mistake is one diagnostic.
fn refuses_the_load(found: &mut Vec<Diagnostic>, scene: &Path, table: &str, said: &str) {
    const HINT: &str = flint_scene::REFUSES_THE_LOAD;
    let mut named = false;
    for d in found
        .iter_mut()
        .filter(|d| d.table.as_deref() == Some(table) && d.entity.is_none())
    {
        named = true;
        d.severity = Severity::Error;
        d.hint = Some(match d.hint.take() {
            Some(hint) => format!("{hint}; {HINT}"),
            None => HINT.to_string(),
        });
    }
    if !named {
        found.push(
            Diagnostic::error(codes::TOML_PARSE, said)
                .file(scene)
                .table(table)
                .hint(HINT),
        );
    }
}

/// Every inline `particle_emitter` held to the emitter's own reader, as the
/// player reads it (#690): a key it doesn't read and a value it can't use
/// are `bad-emitter` errors at their key. The schema declares every key the
/// emitter reads (#1051), its many-form ones (a curve, a `[min, max]`
/// range, a shape table) as `any`, which only the reader can judge, so what
/// the schema pass says of this component's keys and forms (`unknown-field`,
/// `bad-type`, `bad-enum-value`) is left to the reader, and so is anything at a key the
/// reader found wrong: one mistake is said once, in the emitter's words.
/// The schema's ranges and the texture's asset check stay.
fn particle_emitters(scene: &Path, world: &FlintWorld, found: &mut Vec<Diagnostic>) {
    use flint_core::components::PARTICLE_EMITTER;
    use flint_particles::EmitterDef;

    let top = |field: &str| field.split(['.', '[']).next().unwrap_or(field).to_string();
    let mut ours = Vec::new();
    let mut said: BTreeSet<(String, String)> = BTreeSet::new();
    for &id in world.entities_with_component(PARTICLE_EMITTER) {
        let Some(table) = world
            .get_components(id)
            .and_then(|c| c.get(PARTICLE_EMITTER))
            .and_then(|v| v.as_table())
        else {
            continue;
        };
        let entity = world.get_name(id).unwrap_or("").to_string();
        for problem in EmitterDef::problems(table) {
            let what = if problem.unknown {
                format!(
                    "`{}` is not a key particle_emitter reads; the emitter plays without it",
                    problem.key.as_deref().unwrap_or("")
                )
            } else {
                format!(
                    "{}; the emitter would not play (at run time it keeps its last good settings)",
                    problem.message
                )
            };
            let mut d = Diagnostic::error(codes::BAD_EMITTER, what)
                .file(scene)
                .entity(entity.clone())
                .component(PARTICLE_EMITTER)
                .hint_opt(problem.hint.clone());
            if let Some(key) = &problem.key {
                said.insert((entity.clone(), top(key)));
                d = d.field(key.clone());
            }
            ours.push(d);
        }
    }
    found.retain(|d| {
        let on_emitter = d.component.as_deref() == Some(PARTICLE_EMITTER);
        let at_ours = || {
            let entity = d.entity.clone().unwrap_or_default();
            let field = top(d.field.as_deref().unwrap_or(""));
            said.contains(&(entity, field))
        };
        let of_form = [codes::UNKNOWN_FIELD, codes::BAD_TYPE, codes::BAD_ENUM_VALUE];
        !(on_emitter && (of_form.contains(&d.code) || at_ours()))
    });
    found.extend(ours);
}

/// What's wrong with the scene's `[wooze]` table and the preset it names.
/// The zone, when it is about one, is the diagnostic's field.
fn wooze(scene: &Path, file: &SceneFile, schemas: &SceneSchemas) -> Vec<Diagnostic> {
    let mut file = file.clone();
    let preset_issues = flint_procgen::apply_preset_file(&mut file, scene);
    let Some(table) = flint_procgen::LivingTables::of(&file)
        .ok()
        .and_then(|tables| tables.wooze)
    else {
        return Vec::new();
    };
    preset_issues
        .into_iter()
        .chain(table.check(&schemas.registry))
        .map(|issue| {
            let d = Diagnostic::error(codes::BAD_WOOZE, issue.to_string())
                .file(scene)
                .component("wooze");
            match &issue.zone {
                Some(zone) => d.field(zone.clone()),
                None => d,
            }
        })
        .collect()
}

/// What's wrong with the fields the scene declares, `[field.<name>]`
/// (#1014): each key and zone the player would leave out, in the field's
/// words (the zones' are the wooze field's), placed by field and key
/// (`weather.keys.wind`) or zone (`weather.zone.storm`). `[table.field]` is
/// open, so the scene pass says nothing of it and this is the one check.
fn fields(scene: &Path, file: &SceneFile) -> Vec<Diagnostic> {
    let Some((_, issues)) = flint_core::field::SceneFields::of_tables(&file.tables) else {
        return Vec::new();
    };
    issues
        .into_iter()
        .map(|issue| {
            // The place is the diagnostic's table and field, so the message
            // is the issue's alone
            let place = issue.place();
            let d = Diagnostic::error(codes::BAD_FIELD, issue.message)
                .file(scene)
                .table("field");
            if place.is_empty() {
                d
            } else {
                d.field(place)
            }
        })
        .collect()
}

/// The values of a species table the player would leave out.
#[cfg(feature = "living")]
fn species_problems(def: &flint_procgen::LivingSpeciesDef) -> Vec<String> {
    flint_player::species_problems(def)
}

/// Built without the living package (#1333), whose player grows no props:
/// only the species no prop grows from are said.
#[cfg(not(feature = "living"))]
fn species_problems(_: &flint_procgen::LivingSpeciesDef) -> Vec<String> {
    Vec::new()
}

/// What's wrong with the scene's `[living.species.<spec>]` tables (#246): a
/// value the player would leave out, and a species no living prop in the
/// scene grows from, which is most likely a misspelt spec name and would
/// change nothing. The species is the diagnostic's field.
fn species(scene: &Path, file: &SceneFile, world: Option<&FlintWorld>) -> Vec<Diagnostic> {
    let mut file = file.clone();
    // A preset the scene names can hold species too, laid under its own.
    flint_procgen::apply_preset_file(&mut file, scene);
    let living = flint_procgen::LivingTables::of(&file)
        .ok()
        .and_then(|tables| tables.living);
    let Some(living) = living.filter(|living| !living.species.is_empty()) else {
        return Vec::new();
    };
    let grown: Option<Vec<String>> = world.map(|world| {
        let mut grown: Vec<String> = world
            .all_entities()
            .iter()
            .filter_map(|entity| {
                let comps = world.get_components(entity.id)?;
                comps.get(super::LIVING_PROP)?;
                let asset = comps.get(flint_core::components::MODEL)?.get("asset")?;
                Some(asset.as_str()?.to_string())
            })
            .collect();
        grown.sort();
        grown.dedup();
        grown
    });

    let problem = |name: &str, message: String| {
        Diagnostic::error(codes::BAD_SPECIES, message)
            .file(scene)
            .component("living")
            .field(format!("species.{name}"))
    };
    let mut names: Vec<&String> = living.species.keys().collect();
    names.sort_unstable();
    let mut found = Vec::new();
    for name in names {
        for message in species_problems(&living.species[name]) {
            found.push(problem(name, message));
        }
        // Which specs grow is the loaded world's to say
        let Some(grown) = &grown else {
            continue;
        };
        if !grown.contains(name) {
            let species = if grown.is_empty() {
                "it has no living props".to_string()
            } else {
                format!("its living props grow from {}", grown.join(", "))
            };
            found.push(problem(
                name,
                format!(
                    "no living prop in this scene grows from '{name}', \
                     so it changes nothing; {species}"
                ),
            ));
        }
    }
    found
}

/// The constraints the scene breaks, each at the constraint's own severity
/// (`info` reads as a warning here). One that can't be evaluated is among
/// the schemas' own problems, named with its file.
fn constraints(scene: &Path, world: &FlintWorld, schemas: &SceneSchemas) -> Vec<Diagnostic> {
    ConstraintEvaluator::new(world, &schemas.registry, &schemas.constraints)
        .validate()
        .violations
        .into_iter()
        .filter(|v| v.entity_name != BAD_CONSTRAINT)
        .map(|v| {
            let severity = match v.severity {
                flint_constraint::Severity::Error => Severity::Error,
                _ => Severity::Warning,
            };
            let d = Diagnostic::new(
                severity,
                codes::CONSTRAINT,
                format!("{} (constraint `{}`)", v.message, v.constraint_name),
            )
            .file(scene)
            .entity(v.entity_name);
            if v.has_auto_fix {
                d.hint(format!(
                    "`flint validate {} --fix` can fix it",
                    shown(scene)
                ))
            } else {
                d
            }
        })
        .collect()
}
