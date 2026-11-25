use super::*;
use crate::project::{schema_dirs, Project, SchemaDirsError};
use crate::vfs::{self, MemorySource};
use std::sync::Arc;

fn manifest(name: &str, tier: &str, extra: &str) -> String {
    let range = if tier == "kernel" {
        "api = \"0.1.0\""
    } else {
        "engine = \"^0.1\""
    };
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntier = \"{tier}\"\n\
         summary = \"{name}\"\n{range}\n{extra}"
    )
}

/// An engine checkout's packages, as this tree has them in Phase 1.
fn engine() -> MemorySource {
    let mut files = MemorySource::new();
    add_engine(&mut files, "");
    files
}

/// [`engine`]'s files under `prefix`.
fn add_engine(files: &mut MemorySource, prefix: &str) {
    let mut add = |name: &str, tier: &str, extra: &str| {
        files.insert(
            format!("{prefix}packages/{name}/package.toml"),
            manifest(name, tier, extra),
        );
    };
    add(
        "engine",
        "kernel",
        "[provides]\ncomponents = [\"transform\"]\n",
    );
    add(
        "procgen",
        "standard",
        "[build]\nfeatures = [\"procgen\"]\n[provides]\ncrates = [\"flint-procgen\"]\n",
    );
    add(
        "living",
        "feature",
        "[requires]\nprocgen = \"*\"\n[build]\nfeatures = [\"living\"]\n\
         [provides]\ncomponents = [\"living_prop\"]\nscene_tables = [\"living\"]\n\
         script_modules = [\"living\"]\n",
    );
    add(
        "music",
        "feature",
        "[build]\nfeatures = [\"music\"]\n[provides]\ncomponents = [\"music_session\"]\n",
    );
    files.insert(format!("{prefix}schemas/components/transform.toml"), "");
    files.insert(
        format!("{prefix}packages/living/schemas/components/living_prop.toml"),
        "",
    );
    files.insert(
        format!("{prefix}packages/music/schemas/components/music_session.toml"),
        "",
    );
}

fn list(text: &str) -> Result<PackageList, ListError> {
    let doc: toml::Table = text.parse().unwrap();
    PackageList::read(&doc["packages"], Path::new("flint.toml"))
}

fn in_use(text: &str) -> Result<InUse, Vec<Diagnostic>> {
    vfs::scoped(Arc::new(engine()), || {
        InUse::resolve(&list(text).unwrap(), &Packages::read(Path::new("packages")))
    })
}

fn problems(text: &str) -> Vec<String> {
    in_use(text)
        .unwrap_err()
        .into_iter()
        .map(|d| {
            assert_eq!(d.code, codes::BAD_MANIFEST);
            format!(
                "{}: {}{}",
                d.field.unwrap_or_default(),
                d.message,
                d.hint.map(|h| format!(" ({h})")).unwrap_or_default()
            )
        })
        .collect()
}

#[test]
fn an_empty_list_is_the_kernel_and_the_standard_packages() {
    let u = in_use("[packages]\n").unwrap();
    assert_eq!(
        u.on.iter().map(String::as_str).collect::<Vec<_>>(),
        ["engine", "procgen"]
    );
    let out: Vec<&str> = u.left_out.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(out, ["living", "music"]);
}

#[test]
fn a_listed_feature_brings_what_it_requires() {
    let u = in_use("[packages]\nliving = \"*\"\n").unwrap();
    assert!(u.is_on("living") && u.is_on("procgen") && u.is_on("engine"));
    assert!(!u.is_on("music"));
}

#[test]
fn a_standard_package_set_to_false_is_left_out() {
    let u = in_use("[packages]\nprocgen = false\nmusic = \"^0.1\"\n").unwrap();
    assert!(!u.is_on("procgen"));
    assert!(u.is_on("music"));
    assert_eq!(
        u.hint_for("procgen"),
        "remove `procgen = false` from [packages] in flint.toml"
    );
    assert_eq!(
        u.hint_for("living"),
        "add `living = \"*\"` under [packages] in flint.toml"
    );
}

#[test]
fn each_mistake_of_the_list_is_named_by_its_line() {
    let found = problems(
        "[packages]\nlivng = \"*\"\nengine = false\nmusic = false\nprocgen = false\nliving = \"*\"\n",
    );
    assert_eq!(
        found,
        [
            "packages.engine: `engine` is the kernel: every game has it",
            "packages.livng: no package `livng` (the packages are engine, procgen, living, music) \
             (did you mean `living`?)",
            "packages.music: `music` is a feature package: it is out unless listed, so `false` \
             says nothing (remove the line `music = false`)",
            "packages.procgen: `living` requires `procgen`, which `procgen = false` leaves out \
             (remove `procgen = false`, or leave `living` out as well)",
        ]
    );
}

#[test]
fn a_line_that_cannot_be_one_is_refused_when_read() {
    let error = |text: &str| {
        let e = list(text).unwrap_err();
        format!("{}: {}", e.key, e.message)
    };
    assert_eq!(
        error("[packages]\nliving = \"one\"\n"),
        "packages.living: `one` is no version requirement (unexpected character 'o' while \
         parsing major version number); write `\"*\"` for any"
    );
    assert!(error("[packages]\nliving = true\n").contains("`living = \"*\"`, not `true`"));
    assert!(error("[packages]\nliving = { path = \"x\" }\n").contains("(Phase 2)"));
    assert!(error("[packages]\nLiving = \"*\"\n").contains("no package name"));
    assert!(error("packages = 3\n").starts_with("packages: must be a table"));
}

#[test]
fn an_unknown_component_of_a_left_out_package_names_the_line_to_add() {
    let u = in_use("[packages]\n").unwrap();
    let mut found = vec![
        Diagnostic::error(codes::UNKNOWN_COMPONENT, "no schema")
            .file("scenes/grove.scene.toml")
            .entity("oak")
            .component("living_prop"),
        Diagnostic::error(codes::UNKNOWN_COMPONENT, "no schema")
            .file("scenes/grove.scene.toml")
            .entity("birch")
            .component("living_prop"),
        Diagnostic::warning(codes::UNKNOWN_TABLE, "no schema").table("living"),
        Diagnostic::error(codes::UNKNOWN_COMPONENT, "no schema").component("nope"),
    ];
    u.name_unlisted(&mut found);
    // The second prop is counted on the first's line
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].code, codes::UNLISTED_PACKAGE);
    assert_eq!(found[0].entity.as_deref(), Some("oak"));
    assert_eq!(
        found[0].message,
        "the component `living_prop` is the `living` package's, and [packages] doesn't list it \
         (1 other entity too)"
    );
    assert_eq!(
        found[0].hint.as_deref(),
        Some("add `living = \"*\"` under [packages] in flint.toml")
    );
    assert_eq!(found[1].code, codes::UNLISTED_PACKAGE);
    assert_eq!(found[1].severity, crate::diagnostic::Severity::Error);
    assert_eq!(found[2].code, codes::UNKNOWN_COMPONENT);
}

/// A game with the engine at `engine/` and the given `flint.toml`.
fn game(manifest: &str) -> MemorySource {
    let mut files = MemorySource::new();
    add_engine(&mut files, "engine/");
    files.insert("engine/crates/flint-core/Cargo.toml", "");
    files.insert("flint.toml", manifest);
    files.insert("schemas/components/.gitkeep", "");
    files.insert("scenes/a.scene.toml", "");
    files
}

fn dirs_of(manifest: &str) -> Result<Vec<String>, SchemaDirsError> {
    vfs::scoped(Arc::new(game(manifest)), || {
        let found = schema_dirs(Some(Path::new("scenes/a.scene.toml")), &[] as &[&Path])?;
        Ok(found.dirs.iter().map(|d| vfs::normalize(d)).collect())
    })
}

const SCHEMAS: &str = "[project]\nschemas = [\"engine/schemas\", \"schemas\"]\n";

#[test]
fn with_no_packages_key_every_package_schema_loads() {
    assert_eq!(
        dirs_of(SCHEMAS).unwrap(),
        [
            "engine/schemas",
            "engine/packages/living/schemas",
            "engine/packages/music/schemas",
            "schemas"
        ]
    );
}

#[test]
fn a_list_loads_only_the_schemas_of_the_packages_it_lets_in() {
    assert_eq!(
        dirs_of(&format!("{SCHEMAS}[packages]\nmusic = \"*\"\n")).unwrap(),
        ["engine/schemas", "engine/packages/music/schemas", "schemas"]
    );
    assert_eq!(
        dirs_of(&format!("{SCHEMAS}[packages]\n")).unwrap(),
        ["engine/schemas", "schemas"]
    );
    // A package directory the manifest spells out goes with its package
    let spelled = "[project]\nschemas = [\"engine/schemas\", \"engine/packages/living/schemas\", \
                   \"schemas\"]\n[packages]\n";
    assert_eq!(dirs_of(spelled).unwrap(), ["engine/schemas", "schemas"]);
}

#[test]
fn a_mistake_in_the_list_stops_the_schemas_with_its_line() {
    let e = dirs_of(&format!("{SCHEMAS}[packages]\nlivng = \"*\"\n")).unwrap_err();
    let SchemaDirsError::Packages { problems, .. } = &e else {
        panic!("{e}")
    };
    assert_eq!(problems[0].field.as_deref(), Some("packages.livng"));
    assert!(e.to_string().contains("did you mean `living`?"), "{e}");
}

#[test]
fn the_project_keeps_the_list_as_written() {
    vfs::scoped(Arc::new(game("[packages]\nliving = \"^0.1\"\n")), || {
        let project = Project::discover(Path::new("scenes/a.scene.toml")).unwrap();
        let list = project.packages.unwrap();
        assert_eq!(
            list.entries,
            [Listed {
                name: "living".into(),
                wanted: Wanted::Version("^0.1".into())
            }]
        );
    });
    vfs::scoped(Arc::new(game("[project]\nid = \"x\"\n")), || {
        let project = Project::discover(Path::new("scenes/a.scene.toml")).unwrap();
        assert_eq!(project.packages, None);
    });
}
