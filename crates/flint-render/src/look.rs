//! Looks (#279, #356): named post-processing presets laid over a scene.
//!
//! A look is a `styles/<name>.look.toml`, beside the `*.styleguide.toml`
//! generation guides: a `[look]` table that says what it is and what to tune,
//! and a `[post_process]` block in exactly the scene format. Blocks merge key
//! by key (tables merge, everything else replaces), so a look changes only
//! what it names.
//!
//! Two ways in, which differ only in who wins a key both name:
//!
//! - a scene's `[post_process] look = "graphite"` lays the look *under* the
//!   scene's own keys ([`resolve`]), so a scene can take a look and adjust it;
//! - `flint render --look graphite` lays it *over* the scene's block
//!   ([`Look::apply`]), so a look can be tried on any scene as it is.
//!
//! The player and viewer resolve a scene's look on load; F4's Look picker
//! and Rhai's `set_look` re-resolve the same authored keys with another.
//! Every read goes through `flint_core::vfs`, so a game bundle's looks work
//! on the web; the engine's own looks are compiled in.

use flint_core::project::{shown, AssetKind, AssetScope, OrderChange};
use flint_core::vfs;
use flint_scene::PostProcessDef;
use std::path::{Path, PathBuf};

/// The engine's looks, compiled in so a look works from a game repo, an
/// installed binary or the web as well as from the engine checkout.
const BUILTIN: &[(&str, &str)] = &[
    (
        "ascii_noir",
        include_str!("../../../styles/ascii_noir.look.toml"),
    ),
    ("matrix", include_str!("../../../styles/matrix.look.toml")),
    (
        "ink_wash",
        include_str!("../../../styles/ink_wash.look.toml"),
    ),
    (
        "graphite",
        include_str!("../../../styles/graphite.look.toml"),
    ),
    (
        "watercolor",
        include_str!("../../../styles/watercolor.look.toml"),
    ),
    (
        "coloured_pencil",
        include_str!("../../../styles/coloured_pencil.look.toml"),
    ),
    (
        "blueprint",
        include_str!("../../../styles/blueprint.look.toml"),
    ),
    (
        "charcoal",
        include_str!("../../../styles/charcoal.look.toml"),
    ),
];

/// The names of the looks compiled in.
pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTIN.iter().map(|(name, _)| *name)
}

/// A look as written.
#[derive(Debug, Clone)]
pub struct Look {
    pub name: String,
    pub description: String,
    /// Sweeps worth trying first, in `flint render --sweep` syntax.
    pub tune: Vec<String>,
    /// Advice for whoever tunes it (an agent, mostly).
    pub notes: String,
    /// Where it came from: a path, or `built-in`.
    pub source: String,
    /// The `[post_process]` block to merge.
    pub post_process: toml::Table,
}

impl Look {
    /// Parse a look's text; `name` is used when `[look] name` is absent.
    pub fn parse(name: &str, text: &str, source: &str) -> Result<Self, String> {
        let at = format!("look '{name}' ({source})");
        let mut root: toml::Table = toml::from_str(text).map_err(|e| format!("{at}: {e}"))?;
        let meta = match root.remove("look") {
            Some(toml::Value::Table(t)) => t,
            Some(_) => return Err(format!("{at}: [look] must be a table")),
            None => toml::Table::new(),
        };
        let post_process = match root.remove("post_process") {
            Some(toml::Value::Table(t)) => t,
            Some(_) => return Err(format!("{at}: [post_process] must be a table")),
            None => return Err(format!("{at} has no [post_process] block")),
        };
        if let Some(key) = root.keys().next() {
            return Err(format!(
                "{at}: unknown top-level table [{key}] (a look has [look] and [post_process])"
            ));
        }
        if post_process.contains_key("look") {
            return Err(format!("{at}: a look can't name another look"));
        }
        let text_of = |key: &str| {
            meta.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let tune = meta
            .get("tune")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let look = Self {
            name: meta
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(name)
                .to_string(),
            description: text_of("description"),
            notes: text_of("notes"),
            tune,
            source: source.to_string(),
            post_process,
        };
        // Catch a misspelt field or style now, not as a silent default later.
        look.apply(None)
            .map_err(|e| format!("look '{}' ({source}): {e}", look.name))?;
        Ok(look)
    }

    /// The scene's `[post_process]` with this look merged over it (what
    /// `flint render --look` does).
    pub fn apply(&self, scene: Option<&PostProcessDef>) -> Result<PostProcessDef, String> {
        overlay(scene, &self.post_process)
    }

    /// Whether the look is one of the engine's compiled-in ones.
    pub fn is_builtin(&self) -> bool {
        self.source == "built-in"
    }
}

/// `base` with `over` merged into it, checked by deserializing the result.
/// Keys `PostProcessDef` doesn't have are refused, so a typo can't vanish.
pub fn overlay(
    base: Option<&PostProcessDef>,
    over: &toml::Table,
) -> Result<PostProcessDef, String> {
    let mut table = match base {
        Some(def) => toml::Table::try_from(def)
            .map_err(|e| format!("serializing the scene's [post_process]: {e}"))?,
        None => toml::Table::new(),
    };
    // Both sides in the style-table spelling first (#298), so a look's
    // `[styles.kuwahara] radius` overrides a scene's `kuwahara_radius`
    // and the other way round
    flint_scene::fold_legacy_style_keys(&mut table);
    let mut over = over.clone();
    flint_scene::fold_legacy_style_keys(&mut over);
    if let Some((key, near)) = PostProcessDef::unknown_fields(&over).into_iter().next() {
        return Err(format!(
            "[post_process] has no field '{key}'{}",
            did_you_mean(near)
        ));
    }
    merge(&mut table, &over);
    let def: PostProcessDef = toml::Value::Table(table)
        .try_into()
        .map_err(|e| format!("[post_process]: {e}"))?;
    // The style tables are checked against the registry, as a scene's are.
    let (_, warnings) = crate::stylize::StyleSettings::from_tables(&def.styles);
    if let Some(first) = warnings.into_iter().next() {
        return Err(first);
    }
    Ok(def)
}

/// ", did you mean 'x'?" when a known name is close
/// ([`PostProcessDef::closest_field`], shared with the scene loader's
/// warning, #354).
pub fn did_you_mean(near: Option<String>) -> String {
    near.map(|k| format!(", did you mean '{k}'?"))
        .unwrap_or_default()
}

/// Merge `over` into `base`: tables recurse, everything else replaces.
pub fn merge(base: &mut toml::Table, over: &toml::Table) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// A scene's `[post_process]` with `look` laid under its own keys (#356).
///
/// `def` is the block as loaded (its [`PostProcessDef::authored`] keys are
/// what wins), or one an earlier `resolve` returned: the result keeps the
/// authored keys, so resolving it again with another look starts from the
/// scene, not from the last look. `look = None` (or `""`) is the scene's
/// keys alone. A block built in code has no authored record, and then the
/// look goes over it, as `--look` does.
///
/// Pass `def.look.as_deref()` for the look the scene names.
pub fn resolve(
    def: &PostProcessDef,
    look: Option<&str>,
    scene_path: impl AsRef<Path>,
) -> Result<PostProcessDef, String> {
    // No look to find, no project to find either
    if look.is_none_or(|name| name.trim().is_empty()) {
        return resolve_with(def, None, &mut |_| unreachable!("no look is named"));
    }
    resolve_in(def, look, &AssetScope::for_scene(scene_path.as_ref()))
}

/// [`resolve`] for a host that holds the scene's project: the look is
/// searched for from `scope`.
pub fn resolve_in(
    def: &PostProcessDef,
    look: Option<&str>,
    scope: &AssetScope,
) -> Result<PostProcessDef, String> {
    resolve_with(def, look, &mut |name| find_in(name, scope))
}

fn resolve_with(
    def: &PostProcessDef,
    look: Option<&str>,
    find: &mut dyn FnMut(&str) -> Result<Look, String>,
) -> Result<PostProcessDef, String> {
    let authored = def.authored.as_ref().map(|t| {
        let mut t = t.clone();
        t.remove("look");
        t
    });
    let look = look.filter(|name| !name.trim().is_empty());
    let mut out = match (look, &authored) {
        (None, Some(keys)) => overlay(None, keys)?,
        (None, None) => def.clone(),
        (Some(name), Some(keys)) => {
            let found = find(name)?;
            overlay(Some(&found.apply(None)?), keys)?
        }
        (Some(name), None) => {
            let found = find(name)?;
            let mut base = def.clone();
            base.look = None;
            found.apply(Some(&base))?
        }
    };
    out.look = look.map(str::to_string);
    out.authored = def.authored.clone();
    Ok(out)
}

/// [`resolve`] with the look the scene names, or the block as it is (and a
/// warning) when that look can't be found or doesn't fit: a scene still
/// plays without its look.
pub fn resolve_scene(def: &PostProcessDef, scene_path: impl AsRef<Path>) -> PostProcessDef {
    if def.look.is_none() {
        return def.clone();
    }
    let scene_path = scene_path.as_ref();
    resolve_scene_in(def, scene_path, &AssetScope::for_scene(scene_path))
}

/// [`resolve_scene`] for a host that holds the scene's project.
pub fn resolve_scene_in(
    def: &PostProcessDef,
    scene_path: &Path,
    scope: &AssetScope,
) -> PostProcessDef {
    let Some(name) = def.look.as_deref() else {
        return def.clone();
    };
    match resolve_in(def, Some(name), scope) {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!(
                "{}: [post_process] look = \"{name}\": {e} (drawn without it)",
                scene_path.display()
            );
            let mut out = resolve_in(def, None, scope).unwrap_or_else(|_| def.clone());
            out.look = None;
            out
        }
    }
}

/// Find a look by name or path for the scene at `scene_path`, in its
/// project. See [`find_in`].
pub fn find(name_or_path: &str, scene_path: impl AsRef<Path>) -> Result<Look, String> {
    find_in(name_or_path, &AssetScope::for_scene(scene_path.as_ref()))
}

/// Find a look by name or path, by the project's one asset search from
/// `scope` (ADR 0114; [`AssetKind::Looks`]): `styles/<name>.look.toml`, or
/// the name as a path, in the scene's directory, each directory above it up
/// to the project root, then the mounted engine. A file the search finds
/// wins over a built-in look of the same name; the built-ins come after.
/// A path ending `.toml` that is not a `.look.toml` is a file named by a
/// path of its own ([`AssetKind::Files`]), searched the same way.
pub fn find_in(name_or_path: &str, scope: &AssetScope) -> Result<Look, String> {
    let by_path = name_or_path.ends_with(".toml") && !name_or_path.ends_with(LOOK_SUFFIX);
    let answer = if by_path {
        let mut answer = scope.explain(AssetKind::Files, name_or_path);
        // TEMPORARY, with the resolver's order-change warning (#765): such
        // a path was read from the working directory and nowhere else
        let as_written = PathBuf::from(name_or_path);
        answer.order_change = (vfs::is_file(&as_written)
            && answer.found.as_deref() != Some(as_written.as_path()))
        .then(|| OrderChange {
            kind: AssetKind::Looks,
            name: name_or_path.to_string(),
            old: as_written,
            new: answer.found.clone(),
        });
        answer
    } else {
        scope.explain(AssetKind::Looks, name_or_path)
    };
    if let Some(change) = &answer.order_change {
        change.log();
    }
    if let Some(path) = &answer.found {
        let text = vfs::read_to_string(path)
            .map_err(|e| format!("reading look {}: {e}", path.display()))?;
        return Look::parse(&file_stem(path), &text, &shown(path));
    }
    if let Some((name, text)) = BUILTIN.iter().find(|(n, _)| *n == name_or_path) {
        return Look::parse(name, text, "built-in");
    }
    let names: Vec<String> = list_in(scope).into_iter().map(|l| l.name).collect();
    let mut message = format!(
        "unknown look '{name_or_path}'; known: {} (`flint render --look help` describes \
         them). Not built in, and no file. Tried:",
        names.join(", ")
    );
    for path in &answer.tried {
        message.push_str("\n  ");
        message.push_str(&shown(path));
    }
    Err(message)
}

/// How a look file's name ends.
const LOOK_SUFFIX: &str = ".look.toml";

fn file_stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.strip_suffix(".look.toml")
        .or_else(|| name.strip_suffix(".toml"))
        .unwrap_or(&name)
        .to_string()
}

/// Every look reachable from `scene_path`: the files in its folders, then
/// the built-ins they don't shadow, sorted by name; and a message for each
/// file that doesn't parse (and is skipped).
pub fn list_checked(scene_path: impl AsRef<Path>) -> (Vec<Look>, Vec<String>) {
    list_checked_in(&AssetScope::for_scene(scene_path.as_ref()))
}

/// [`list_checked`] from `scope`: every look file the project's search
/// sees ([`Project::list`](flint_core::project::Project::list): a nearer
/// file hides a farther one of its name), then the built-ins no file
/// shadows.
pub fn list_checked_in(scope: &AssetScope) -> (Vec<Look>, Vec<String>) {
    let mut looks: Vec<Look> = Vec::new();
    let mut skipped = Vec::new();
    for listed in scope.list(AssetKind::Looks) {
        let path = listed.path;
        match vfs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| Look::parse(&listed.name, &t, &shown(&path)))
        {
            Ok(look) => looks.push(look),
            Err(e) => skipped.push(format!("skipping {}: {e}", shown(&path))),
        }
    }
    for (name, text) in BUILTIN {
        if !looks.iter().any(|l| l.name == *name) {
            if let Ok(look) = Look::parse(name, text, "built-in") {
                looks.push(look);
            }
        }
    }
    looks.sort_by(|a, b| a.name.cmp(&b.name));
    (looks, skipped)
}

/// [`list_checked`], logging the skipped files.
pub fn list(scene_path: impl AsRef<Path>) -> Vec<Look> {
    list_in(&AssetScope::for_scene(scene_path.as_ref()))
}

/// [`list_checked_in`], logging the skipped files.
pub fn list_in(scope: &AssetScope) -> Vec<Look> {
    let (looks, skipped) = list_checked_in(scope);
    for message in skipped {
        tracing::warn!("{message}");
    }
    looks
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::vfs::MemorySource;
    use std::sync::Arc;

    #[test]
    fn every_builtin_look_parses_and_checks() {
        for (name, text) in BUILTIN {
            let look =
                Look::parse(name, text, "built-in").unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(look.name, *name, "[look] name matches the file");
            assert!(!look.description.is_empty(), "{name} says what it is");
            assert!(!look.tune.is_empty(), "{name} suggests a sweep");
        }
    }

    #[test]
    fn the_shipped_files_are_the_builtins() {
        // A new styles/*.look.toml is compiled in, or a game repo (or the
        // web) can't find it.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../styles");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".look.toml"))
            .map(|p| file_stem(&p))
            .collect();
        on_disk.sort();
        let mut builtin: Vec<String> = builtin_names().map(str::to_string).collect();
        builtin.sort();
        assert_eq!(on_disk, builtin);
    }

    #[test]
    fn a_look_changes_only_what_it_names() {
        let scene: PostProcessDef =
            toml::from_str("exposure = 2.5\nbloom_intensity = 0.3").unwrap();
        let look = Look::parse(
            "t",
            "[post_process]\nbloom_intensity = 0.1\n[post_process.styles.invert]\nenabled = true",
            "test",
        )
        .unwrap();
        let merged = look.apply(Some(&scene)).unwrap();
        assert_eq!(merged.exposure, 2.5, "the scene's exposure survives");
        assert_eq!(merged.bloom_intensity, 0.1, "the look's bloom wins");
        assert!(merged.styles.contains_key("invert"));
    }

    #[test]
    fn style_tables_merge_key_by_key() {
        let scene: PostProcessDef =
            toml::from_str("[styles.ascii]\nenabled = true\ncell = 12\nramp = \"blocks\"").unwrap();
        let over: toml::Table = toml::from_str("[styles.ascii]\ncolor = \"mono\"").unwrap();
        let merged = overlay(Some(&scene), &over).unwrap();
        let ascii = &merged.styles["ascii"];
        assert_eq!(ascii["cell"].as_integer(), Some(12));
        assert_eq!(ascii["color"].as_str(), Some("mono"));
    }

    #[test]
    fn a_misspelt_field_is_refused_with_a_hint() {
        let err = Look::parse("t", "[post_process]\nbloom_intensty = 0.1", "test").unwrap_err();
        assert!(err.contains("bloom_intensity"), "{err}");
    }

    #[test]
    fn an_unknown_style_is_refused() {
        let err = Look::parse(
            "t",
            "[post_process.styles.watercolour]\nenabled = true",
            "test",
        )
        .unwrap_err();
        assert!(err.contains("watercolour"), "{err}");
    }

    #[test]
    fn a_look_cannot_name_a_look() {
        let err = Look::parse("t", "[post_process]\nlook = \"matrix\"", "test").unwrap_err();
        assert!(err.contains("another look"), "{err}");
    }

    fn authored(text: &str) -> PostProcessDef {
        PostProcessDef::from_authored(toml::from_str(text).unwrap()).unwrap()
    }

    /// Done when #356: a scene naming a look draws what `--look` draws
    /// when the scene says nothing else.
    #[test]
    fn a_scene_look_alone_is_the_cli_look() {
        let scene = authored("look = \"graphite\"");
        let resolved = resolve(&scene, scene.look.as_deref(), "levels/a.scene.toml").unwrap();
        let cli = find("graphite", "levels/a.scene.toml")
            .unwrap()
            .apply(Some(&authored("")))
            .unwrap();
        assert_eq!(
            toml::Table::try_from(&resolved).unwrap().remove("styles"),
            toml::Table::try_from(&cli).unwrap().remove("styles")
        );
        assert_eq!(resolved.exposure, cli.exposure);
        assert_eq!(resolved.look.as_deref(), Some("graphite"));
    }

    #[test]
    fn a_scene_look_goes_under_the_scenes_keys() {
        // graphite sets the pencil style; the scene keeps its own spacing
        // and its own exposure
        let scene = authored("look = \"graphite\"\nexposure = 1.7\n[styles.pencil]\nspacing = 9");
        let out = resolve(&scene, scene.look.as_deref(), "x.scene.toml").unwrap();
        assert_eq!(out.exposure, 1.7);
        let pencil = &out.styles["pencil"];
        assert_eq!(
            pencil["spacing"].as_integer(),
            Some(9),
            "the scene's key wins"
        );
        let look = find("graphite", "x.scene.toml").unwrap();
        let look_pencil = look.post_process["styles"]["pencil"].as_table().unwrap();
        for key in look_pencil.keys().filter(|k| *k != "spacing") {
            assert_eq!(
                pencil.get(key),
                look_pencil.get(key),
                "the look's {key} stays"
            );
        }
    }

    #[test]
    fn resolving_again_starts_from_the_scene() {
        let scene = authored("look = \"matrix\"\nexposure = 1.2");
        let matrix = resolve(&scene, Some("matrix"), "x.scene.toml").unwrap();
        let ink = resolve(&matrix, Some("ink_wash"), "x.scene.toml").unwrap();
        let ink_direct = resolve(&scene, Some("ink_wash"), "x.scene.toml").unwrap();
        assert_eq!(
            toml::Table::try_from(&ink).unwrap(),
            toml::Table::try_from(&ink_direct).unwrap(),
            "matrix leaves nothing behind"
        );
        let none = resolve(&ink, None, "x.scene.toml").unwrap();
        assert!(none.styles.is_empty(), "no look: the scene's keys alone");
        assert_eq!(none.exposure, 1.2);
        assert_eq!(none.look, None);
    }

    #[test]
    fn an_unknown_scene_look_draws_without_it() {
        let scene = authored("look = \"nope\"\nexposure = 1.4");
        let out = resolve_scene(&scene, "x.scene.toml");
        assert_eq!(out.exposure, 1.4);
        assert_eq!(out.look, None);
        assert!(resolve(&scene, Some("nope"), "x.scene.toml")
            .unwrap_err()
            .contains("unknown look 'nope'"));
    }

    /// Looks are found through the vfs (the web's bundle), nearest
    /// folder first, and a file shadows a built-in of its name.
    #[test]
    fn looks_are_found_through_the_vfs() {
        let source: MemorySource = [
            (
                "game/styles/dusk.look.toml",
                b"[look]\ndescription = \"d\"\n[post_process]\nexposure = 0.6\n".to_vec(),
            ),
            (
                "game/levels/styles/matrix.look.toml",
                b"[post_process]\nexposure = 3.0\n".to_vec(),
            ),
            (
                "game/levels/a.scene.toml",
                b"[scene]\nname = \"a\"\n".to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        vfs::scoped(Arc::new(source), || {
            let scene = "game/levels/a.scene.toml";
            assert_eq!(
                find("dusk", scene).unwrap().post_process["exposure"].as_float(),
                Some(0.6)
            );
            let matrix = find("matrix", scene).unwrap();
            assert!(
                !matrix.is_builtin(),
                "the scene's file shadows the built-in"
            );
            let names: Vec<String> = list(scene).into_iter().map(|l| l.name).collect();
            assert!(names.contains(&"dusk".to_string()));
            assert!(names.contains(&"graphite".to_string()), "built-ins too");
            assert_eq!(names.iter().filter(|n| *n == "matrix").count(), 1);
        });
    }

    /// ADR 0114: a scene two folders down finds the project's look and the
    /// mounted engine's; the nearest of a name wins, a file before a
    /// built-in; and a miss lists the paths tried.
    #[test]
    fn a_nested_scene_finds_the_projects_looks_by_the_one_rule() {
        let look = |exposure: f32| format!("[post_process]\nexposure = {exposure}\n").into_bytes();
        let source: MemorySource = [
            ("game/flint.toml", Vec::new()),
            ("game/styles/dusk.look.toml", look(0.6)),
            ("game/styles/matrix.look.toml", look(3.0)),
            ("game/levels/act1/styles/dusk.look.toml", look(0.25)),
            ("game/levels/act1/cave/a.scene.toml", Vec::new()),
            ("game/levels/b.scene.toml", Vec::new()),
            ("game/engine/styles/stock.look.toml", look(1.5)),
            ("game/out/tuned.look.toml", look(2.25)),
            ("game/out/plain.toml", look(2.5)),
        ]
        .into_iter()
        .collect();
        vfs::scoped(Arc::new(source), || {
            let deep = "game/levels/act1/cave/a.scene.toml";
            let exposure = |name: &str, scene: &str| {
                find(name, scene).unwrap().post_process["exposure"].as_float()
            };
            // The nearest dusk from the cave; the project's from levels/
            assert_eq!(exposure("dusk", deep), Some(0.25));
            assert_eq!(exposure("dusk", "game/levels/b.scene.toml"), Some(0.6));
            // The project's file, three folders up, before the built-in
            let matrix = find("matrix", deep).unwrap();
            assert_eq!(matrix.source, "game/styles/matrix.look.toml");
            // The mounted engine's, last of the files
            assert_eq!(exposure("stock", deep), Some(1.5));
            // No file: the built-in
            assert!(find("graphite", deep).unwrap().is_builtin());
            // A path, from each directory of the walk
            assert_eq!(exposure("out/tuned.look.toml", deep), Some(2.25));
            assert_eq!(exposure("out/plain.toml", deep), Some(2.5));
            assert_eq!(find("out/tuned.look.toml", deep).unwrap().name, "tuned");

            let names: Vec<String> = list(deep).into_iter().map(|l| l.name).collect();
            for name in ["dusk", "matrix", "stock", "graphite"] {
                assert_eq!(names.iter().filter(|n| *n == name).count(), 1, "{name}");
            }

            let err = find("nope", deep).unwrap_err();
            assert!(err.starts_with("unknown look 'nope'; known: "), "{err}");
            let tried: Vec<&str> = err.lines().skip(1).map(str::trim).collect();
            assert_eq!(
                tried,
                [
                    "game/levels/act1/cave/styles/nope.look.toml",
                    "game/levels/act1/cave/nope.look.toml",
                    "game/levels/act1/styles/nope.look.toml",
                    "game/levels/act1/nope.look.toml",
                    "game/levels/styles/nope.look.toml",
                    "game/levels/nope.look.toml",
                    "game/styles/nope.look.toml",
                    "game/nope.look.toml",
                    "game/engine/styles/nope.look.toml",
                    "game/engine/nope.look.toml",
                ]
            );
        });
    }

    /// The working directory's `styles/` is no longer searched (ADR 0114):
    /// a look there, outside the scene's project, is not found.
    #[test]
    fn the_working_directorys_styles_are_not_searched() {
        let source: MemorySource = [
            ("game/flint.toml", Vec::new()),
            ("game/levels/a.scene.toml", Vec::new()),
            (
                "styles/dusk.look.toml",
                b"[post_process]\nexposure = 0.6\n".to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        vfs::scoped(Arc::new(source), || {
            let scope = AssetScope::for_scene(Path::new("game/levels/a.scene.toml"));
            let err = find_in("dusk", &scope).unwrap_err();
            assert!(!err.contains("\n  styles/dusk.look.toml"), "{err}");
            // The one-release warning names the file the old search read
            let change = scope
                .explain(AssetKind::Looks, "dusk")
                .order_change
                .expect("the old search found it");
            assert_eq!(change.old, Path::new("styles/dusk.look.toml"));
            assert_eq!(change.new, None);
        });
    }
}
