//! What `flint check` asks of one scene: that it loads, that its entities
//! fit the schemas, that every asset it names is where the engine will look,
//! that its styles, look, `[wooze]` and `[living]` tables are right, and
//! that it keeps the project's constraints. Everything is read through
//! `flint_core::vfs`, so a scene inside a packed bundle is checked as one on
//! the disk is.

use super::{Checks, SceneSchemas};
use flint_asset::{AssetCatalog, ContentStore, ProjectStore};
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

fn dangling_parent(d: &Diagnostic) -> bool {
    d.code == codes::UNKNOWN_ENTITY && d.component.is_none()
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
    /// Scene directories whose user styles have been loaded and reported.
    style_dirs: BTreeSet<PathBuf>,
    /// The asset catalog of each project root.
    catalogs: BTreeMap<PathBuf, Option<Catalog>>,
}

/// A project's asset catalog and content store: asked before the files for
/// a model or a texture, as the player asks.
struct Catalog {
    catalog: AssetCatalog,
    store: ContentStore,
}

impl Catalog {
    fn of(project: &Project) -> Option<Self> {
        let catalog = AssetCatalog::load_from_directory(project.catalog_dir()).ok()?;
        Some(Self {
            catalog,
            store: ContentStore::new(project.store_dir()),
        })
    }

    fn holds(&self, name: &str) -> bool {
        self.catalog
            .get(name)
            .and_then(|meta| flint_core::ContentHash::from_prefixed_hex(&meta.hash))
            .and_then(|hash| self.store.get(&hash))
            .is_some()
    }
}

/// What a scene's check found, and the files it reached: the scripts,
/// prefabs and effects a single-scene check goes on to read.
#[derive(Default)]
pub struct SceneFound {
    pub diagnostics: Vec<Diagnostic>,
    pub reached: Vec<PathBuf>,
}

/// Everything `checks` asks of the scene at `scene`. Never stops at the
/// first problem; a scene that can't be read at all is the one case with
/// nothing more to ask.
pub fn check(
    scene: &Path,
    schemas: &SceneSchemas,
    checks: Checks,
    strict: bool,
    memo: &mut Memo,
) -> SceneFound {
    let mut out = SceneFound::default();
    let project = Project::for_scene(scene);
    let scene_dir = scene
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut found = flint_scene::diagnose(scene, &schemas.registry, &project);
    if strict {
        diagnostic::promote_strict(&mut found);
    }
    if !checks.schema {
        found.retain(stops_a_load);
    }
    // This pass's own asset check asks the catalog and the specs first
    if checks.files {
        found.retain(|d| !(d.code == codes::MISSING_ASSET && is_entity_asset_field(d)));
    }
    let unreadable = cannot_load(&found);
    let no_parent = found.iter().any(dangling_parent);
    out.diagnostics.extend(found);
    if unreadable {
        return out;
    }
    // `diagnose` read and parsed it a moment ago
    let Ok(text) = vfs::read_to_string(scene) else {
        return out;
    };
    let Ok(file) = toml::from_str::<SceneFile>(&text) else {
        return out;
    };

    let world = match load_scene_diagnosed(scene, &schemas.registry) {
        Ok((world, _, _)) => Some(world),
        // A parent that isn't there has been named, with its entity
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
    // check, by the table's name
    if let Err(problem) = flint_procgen::LivingTables::of(&file) {
        // `[living]: unknown field ...`: the table, then what serde said
        let (table, said) = problem
            .strip_prefix('[')
            .and_then(|rest| rest.split_once("]: "))
            .map_or((None, problem.as_str()), |(table, said)| {
                (Some(table), said)
            });
        let d = Diagnostic::error(codes::TOML_PARSE, said)
            .file(scene)
            .hint("the scene does not load until the table fits");
        out.diagnostics.push(match table {
            Some(table) => d.table(table),
            None => d,
        });
    }
    if checks.files {
        assets(scene, scene_dir, &file, &project, memo, &mut out);
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
    }
    if checks.constraints {
        out.diagnostics.extend(schemas.problems.iter().cloned());
        if let Some(world) = &world {
            out.diagnostics.extend(constraints(scene, world, schemas));
        }
    }
    out
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
        .or_insert_with(|| Catalog::of(project));
    let in_catalog = |name: &str| catalog.as_ref().is_some_and(|c| c.holds(name));
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
            if name.is_empty() {
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
                Some(path) => out.reached.push(path),
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
    // The styles are registered either way: the tables below are read
    // against them. They are reported once a directory.
    let messages = flint_render::stylize::custom::load_dir(scene_dir);
    if memo.style_dirs.insert(scene_dir.to_path_buf()) {
        found.extend(messages.iter().map(|m| user_style_diagnostic(m, scene_dir)));
    }
    let Some(mut post) = file.post_process.clone() else {
        return;
    };
    for warning in flint_render::stylize::StyleSettings::from_tables(&post.style_tables()).1 {
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
        if let Err(e) = flint_render::look::resolve(&post, Some(&name), scene) {
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

/// One message of `stylize::custom::load_dir` as a diagnostic. A message
/// leads with the file it is about (and, from the shader compiler, a line
/// and a column); `: warning: ` marks one that is for the author and does
/// not stop the style loading.
fn user_style_diagnostic(message: &str, scene_dir: &Path) -> Diagnostic {
    let warning = message.contains(": warning: ");
    let (from, what) = match message.split_once(": ") {
        Some((from, what)) => (from.to_string(), what.trim_end().to_string()),
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
    let what = what.strip_prefix("warning: ").unwrap_or(&what).to_string();
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
                comps.get(flint_core::components::LIVING_PROP)?;
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
        for message in flint_player::species_problems(&living.species[name]) {
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
