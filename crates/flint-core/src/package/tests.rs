use super::*;
use crate::vfs::{self, MemorySource};
use std::sync::Arc;

const LIVING: &str = "[package]\nname = \"living\"\nversion = \"0.1.0\"\ntier = \"feature\"\n\
summary = \"Props that hold and glide\"\nengine = \"^0.1\"\n\
[requires]\nprocgen = \"*\"\n[build]\nfeatures = [\"living\"]\n\
[provides]\ncrates = [\"flint-living\"]\ncomponents = [\"living_prop\"]\n";

const KERNEL: &str = "[package]\nname = \"engine\"\nversion = \"0.1.0\"\ntier = \"kernel\"\n\
summary = \"The kernel\"\napi = \"0.1.0\"\n[provides]\ncrates = [\"flint-core\"]\n";

const PROCGEN: &str = "[package]\nname = \"procgen\"\nversion = \"0.1.0\"\ntier = \"standard\"\n\
summary = \"Specs\"\nengine = \"^0.1\"\n[provides]\ncrates = [\"flint-procgen\"]\n";

fn file() -> &'static Path {
    Path::new("packages/living/package.toml")
}

/// Every mistake of `text`, each as `field: message`.
fn mistakes(text: &str) -> Vec<String> {
    parse(text, file())
        .unwrap_err()
        .into_iter()
        .map(|d| {
            assert_eq!(d.code, codes::BAD_PACKAGE);
            assert_eq!(d.file.as_deref(), Some(file()));
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
fn a_manifest_reads_whole() {
    let p = parse(LIVING, file()).unwrap();
    assert_eq!(p.name, "living");
    assert_eq!(p.version, "0.1.0");
    assert_eq!(p.tier, Tier::Feature);
    assert_eq!(p.engine.as_deref(), Some("^0.1"));
    assert_eq!(p.api, None);
    assert_eq!(p.requires.get("procgen").map(String::as_str), Some("*"));
    assert_eq!(p.features, ["living"]);
    assert_eq!(p.provides.of(Kind::Crate), ["flint-living"]);
    assert_eq!(p.provides.of(Kind::Component), ["living_prop"]);
    assert!(p.provides.of(Kind::Command).is_empty());
    assert_eq!(p.dir(), Path::new("packages/living"));
}

#[test]
fn a_misspelled_key_or_table_is_named_with_the_nearest() {
    let text = LIVING
        .replace("summary", "sumary")
        .replace("[build]", "[biuld]")
        .replace("components =", "componets =");
    let found = mistakes(&text);
    assert!(
        found.contains(&"package.sumary: unknown key `sumary` in [package] (the keys are name, version, tier, summary, engine, api) (did you mean `summary`?)".to_string()),
        "{found:#?}"
    );
    assert!(found.contains(&"package.summary: missing: one line on what it is".to_string()));
    assert!(
        found
            .iter()
            .any(|f| f.starts_with("biuld: unknown table `[biuld]`")
                && f.ends_with("(did you mean `build`?)")),
        "{found:#?}"
    );
    assert!(
        found
            .iter()
            .any(|f| f.starts_with("provides.componets: unknown key")
                && f.ends_with("(did you mean `components`?)")),
        "{found:#?}"
    );
}

#[test]
fn every_mistake_is_reported_at_once() {
    let text =
        "[package]\nname = \"Living\"\nversion = \"1\"\ntier = \"feture\"\nsummary = \"a\\nb\"\n\
        engine = \"not a range\"\n[requires]\nmusic = 3\n[provides]\ncrates = [\"a\", \"a\", 4]\n\
        commands = [\"music --chart\"]\n";
    let found = mistakes(text);
    for want in [
        "package.name: `Living` is no package name",
        "package.version: `1` is no semver version",
        "package.tier: `feture` is no tier (the tiers are kernel, standard, feature, game) (did you mean `feature`?)",
        "package.summary: one line, not empty",
        "package.engine: `not a range` is no version requirement",
        "requires.music: a version requirement string",
        "provides.crates[1]: `a` is listed twice",
        "provides.crates[2]: a string, not an integer",
        "provides.commands[0]: `music --chart` is no command path",
    ] {
        assert!(
            found.iter().any(|f| f.starts_with(want)),
            "missing {want:?} in {found:#?}"
        );
    }
}

#[test]
fn the_kernel_states_the_api_and_the_others_a_range_of_it() {
    assert!(parse(KERNEL, Path::new("packages/engine/package.toml")).is_ok());
    let found = mistakes(&KERNEL.replace("api = \"0.1.0\"", "engine = \"^0.1\""));
    assert!(
        found
            .iter()
            .any(|f| f.starts_with("package.engine: the kernel is the engine API")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|f| f.starts_with("package.api: missing")),
        "{found:#?}"
    );

    let found = mistakes(&LIVING.replace("engine = \"^0.1\"", "api = \"0.1.0\""));
    assert!(
        found
            .iter()
            .any(|f| f.starts_with("package.engine: missing")),
        "{found:#?}"
    );
    assert!(
        found
            .iter()
            .any(|f| f.starts_with("package.api: only the kernel")),
        "{found:#?}"
    );
}

#[test]
fn a_package_does_not_require_itself() {
    let found = mistakes(&LIVING.replace("procgen = \"*\"", "living = \"*\""));
    assert_eq!(
        found,
        ["requires.living: a package does not require itself"]
    );
}

#[test]
fn not_toml_and_no_package_table_are_one_mistake_each() {
    assert!(mistakes("[package")[0].starts_with(": not TOML"));
    assert_eq!(
        mistakes("[provides]\ncrates = []\n"),
        ["package: no [package] table: a manifest names its package, version, tier and summary"]
    );
}

fn folder(files: &[(&str, &str)]) -> Packages {
    let mut source = MemorySource::new();
    for (path, text) in files {
        source.insert(*path, *text);
    }
    vfs::scoped(Arc::new(source), || Packages::read(Path::new("packages")))
}

#[test]
fn a_folder_reads_through_the_vfs_by_tier_then_name() {
    let read = folder(&[
        ("packages/living/package.toml", LIVING),
        ("packages/engine/package.toml", KERNEL),
        ("packages/procgen/package.toml", PROCGEN),
        ("packages/notes/README.md", "no manifest here"),
    ]);
    assert!(read.problems.is_empty(), "{:#?}", read.problems);
    let names: Vec<&str> = read.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["engine", "procgen", "living"]);
    assert_eq!(
        read.owner(Kind::Crate, "flint-procgen").unwrap().name,
        "procgen"
    );
    assert!(read.owner(Kind::Crate, "flint-nothing").is_none());
    assert_eq!(read.get("living").unwrap().tier, Tier::Feature);
}

#[test]
fn an_item_two_packages_provide_names_both() {
    let read = folder(&[
        ("packages/living/package.toml", LIVING),
        ("packages/engine/package.toml", KERNEL),
        (
            "packages/procgen/package.toml",
            &PROCGEN.replace("\"flint-procgen\"", "\"flint-procgen\", \"flint-living\""),
        ),
    ]);
    let lines: Vec<String> = read.problems.iter().map(Diagnostic::one_line).collect();
    assert_eq!(
        lines,
        ["packages/living/package.toml: error[bad-package] field 'provides.crates': the crate `flint-living` is also provided by `procgen` (packages/procgen/package.toml): an item has one owner"]
    );
    assert!(read.failed());
}

#[test]
fn the_checks_across_manifests() {
    let read = folder(&[
        (
            "packages/living/package.toml",
            &LIVING.replace("procgen = \"*\"", "procgn = \"*\""),
        ),
        ("packages/engine/package.toml", KERNEL),
        (
            "packages/core/package.toml",
            &KERNEL.replace("flint-core", "flint-ecs"),
        ),
        ("packages/gen/package.toml", PROCGEN),
    ]);
    let lines: Vec<String> = read.problems.iter().map(Diagnostic::one_line).collect();
    for want in [
        "packages/living/package.toml: error[bad-package] field 'requires.procgn': requires `procgn`, and no package is named that (did you mean `procgen`?)",
        "packages/core/package.toml: error[bad-package] field 'package.name': the package `engine` is in the folder `core`: a package's folder is its name",
        "packages/engine/package.toml: error[bad-package] field 'package.name': the package `engine` is also named in packages/core/package.toml: a name is one package",
        "packages/gen/package.toml: error[bad-package] field 'package.name': the package `procgen` is in the folder `gen`: a package's folder is its name",
        "error[bad-package] field 'package.tier': a second kernel (the first is `engine`): there is one kernel",
    ] {
        assert!(lines.iter().any(|l| l.ends_with(want) || l == want), "missing {want:?} in {lines:#?}");
    }
}

#[test]
fn a_folder_that_isnt_there_is_one_mistake() {
    let read = folder(&[]);
    assert!(read.packages.is_empty());
    assert_eq!(read.problems.len(), 1);
    assert!(read.problems[0]
        .message
        .starts_with("no package folder here"));
}

#[test]
fn the_engines_packages_are_the_subtrees_then_the_checkouts() {
    let mut game = MemorySource::new();
    game.insert("game/engine/packages/living/package.toml", LIVING);
    game.insert("game/engine/crates/flint-core/Cargo.toml", "");
    vfs::scoped(Arc::new(game), || {
        assert_eq!(
            engine_packages_dir(Path::new("game")),
            Some(PathBuf::from("game/engine/packages"))
        );
        assert_eq!(
            engine_packages_dir(Path::new("game/engine")),
            Some(PathBuf::from("game/engine/packages"))
        );
    });
    let mut stray = MemorySource::new();
    stray.insert("notes/packages/x/package.toml", LIVING);
    vfs::scoped(Arc::new(stray), || {
        // A `packages/` folder is the engine's only in an engine checkout
        assert_eq!(engine_packages_dir(Path::new("notes")), None);
    });
}

/// The repo's own manifests read with no mistake. flint-cli's completeness
/// test holds what they list to the tree.
#[test]
fn the_engines_manifests_read() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages");
    let read = Packages::read(&dir);
    let lines: Vec<String> = read.problems.iter().map(Diagnostic::one_line).collect();
    assert!(lines.is_empty(), "{lines:#?}");
    assert_eq!(read.packages.len(), 7);
    assert_eq!(read.packages[0].name, "engine");
    assert_eq!(read.packages[0].tier, Tier::Kernel);
}
