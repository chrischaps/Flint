//! A package's manifest, `package.toml` (ADR 0136 decision 8, #1331).
//!
//! A package is a name a game lists and everything that name brings: its
//! crates, components, archetypes, scene tables, script modules and CLI
//! commands. Its manifest says what it owns, so tools learn it without
//! compiling anything (`flint package list`, and from #1334 `flint check`).
//! Every item of the engine has exactly one owner; flint-cli's completeness
//! test holds that against the tree.
//!
//! ```toml
//! [package]
//! name = "living"            # required: [a-z][a-z0-9-]*, unique; the key a game lists
//! version = "0.1.0"          # required: semver; the workspace version until Phase 5
//! tier = "feature"           # required: kernel | standard | feature | game
//! summary = "Props that hold and glide, and the wooze field"   # required, one line
//! engine = "^0.1"            # required except on the kernel: the engine API range
//! # api = "0.1.0"            # the kernel's manifest only: the engine API version
//!
//! [requires]                 # other packages it needs: name = version requirement
//! procgen = "*"
//!
//! [build]                    # how flint build turns it on (Phase 1)
//! features = ["living"]      # features of flint-player and flint-cli, same names
//!
//! [provides]                 # what it owns; every item has exactly one owner
//! crates = ["flint-living", "flint-wooze"]
//! components = ["living_prop", "shallow_wooze"]
//! archetypes = []
//! scene_tables = ["living", "wooze"]
//! script_modules = ["living", "wooze", "debug"]
//! commands = []              # paths below `flint`: "music chart", "asset generate"
//! ```
//!
//! Every read goes through the [`vfs`]. A mistake is a [`Diagnostic`]
//! (`bad-package`) naming the file and the key, as `flint.toml`'s are, and
//! every mistake of a file is reported at once, not only the first.
//!
//! ```
//! use flint_core::package::{self, Tier};
//! use std::path::Path;
//!
//! let living = package::parse(
//!     "[package]\nname = \"living\"\nversion = \"0.1.0\"\ntier = \"feature\"\n\
//!      summary = \"Props that hold and glide\"\nengine = \"^0.1\"\n\
//!      [requires]\nprocgen = \"*\"\n[provides]\ncrates = [\"flint-living\"]\n",
//!     Path::new("packages/living/package.toml"),
//! )
//! .unwrap();
//! assert_eq!(living.tier, Tier::Feature);
//! assert_eq!(living.provides.crates, ["flint-living"]);
//!
//! let typo = package::parse(
//!     "[package]\nname = \"living\"\nversoin = \"0.1.0\"\n",
//!     Path::new("packages/living/package.toml"),
//! )
//! .unwrap_err();
//! assert!(typo[0].one_line().contains("did you mean `version`?"));
//! ```

use crate::diagnostic::{codes, did_you_mean, Diagnostic};
use crate::vfs;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod in_use;
pub use in_use::{InUse, ListError, Listed, PackageList, Wanted};

/// A package's manifest file name, in the package's own folder.
pub const MANIFEST_FILE: &str = "package.toml";

/// The folder of an engine checkout that holds a folder a package (ADR 0136
/// decision 4): `packages/<name>/package.toml`.
pub const PACKAGES_DIR: &str = "packages";

/// The tables a manifest may hold.
const TABLES: [&str; 4] = ["package", "requires", "build", "provides"];

/// The keys `[package]` may hold.
const PACKAGE_KEYS: [&str; 6] = ["name", "version", "tier", "summary", "engine", "api"];

/// The keys `[build]` may hold.
const BUILD_KEYS: [&str; 1] = ["features"];

/// A package's tier (ADR 0136 decision 3): who has it without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Every game links it. Never listed; always there.
    Kernel,
    /// On unless a game drops it (`name = false`).
    Standard,
    /// Off unless a game lists it (`name = "*"`).
    Feature,
    /// Lives in the game's repo, in the same form (Phase 2).
    Game,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Kernel, Tier::Standard, Tier::Feature, Tier::Game];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Kernel => "kernel",
            Tier::Standard => "standard",
            Tier::Feature => "feature",
            Tier::Game => "game",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One kind of thing a package owns: a list in `[provides]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Crate,
    Component,
    Archetype,
    SceneTable,
    ScriptModule,
    Command,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Crate,
        Kind::Component,
        Kind::Archetype,
        Kind::SceneTable,
        Kind::ScriptModule,
        Kind::Command,
    ];

    /// Its key in `[provides]`: `scene_tables`.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Crate => "crates",
            Kind::Component => "components",
            Kind::Archetype => "archetypes",
            Kind::SceneTable => "scene_tables",
            Kind::ScriptModule => "script_modules",
            Kind::Command => "commands",
        }
    }

    /// One of it, in a sentence: `scene table`.
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Crate => "crate",
            Kind::Component => "component",
            Kind::Archetype => "archetype",
            Kind::SceneTable => "scene table",
            Kind::ScriptModule => "script module",
            Kind::Command => "command",
        }
    }
}

/// What a package owns (`[provides]`), each list in the manifest's order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Provides {
    pub crates: Vec<String>,
    pub components: Vec<String>,
    pub archetypes: Vec<String>,
    pub scene_tables: Vec<String>,
    pub script_modules: Vec<String>,
    /// Paths below `flint`, words split by one space: `music chart`.
    pub commands: Vec<String>,
}

impl Provides {
    pub fn of(&self, kind: Kind) -> &[String] {
        match kind {
            Kind::Crate => &self.crates,
            Kind::Component => &self.components,
            Kind::Archetype => &self.archetypes,
            Kind::SceneTable => &self.scene_tables,
            Kind::ScriptModule => &self.script_modules,
            Kind::Command => &self.commands,
        }
    }

    fn of_mut(&mut self, kind: Kind) -> &mut Vec<String> {
        match kind {
            Kind::Crate => &mut self.crates,
            Kind::Component => &mut self.components,
            Kind::Archetype => &mut self.archetypes,
            Kind::SceneTable => &mut self.scene_tables,
            Kind::ScriptModule => &mut self.script_modules,
            Kind::Command => &mut self.commands,
        }
    }
}

/// One package's manifest, read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub tier: Tier,
    pub summary: String,
    /// The engine API range it builds against; `None` only on the kernel.
    pub engine: Option<String>,
    /// The engine API version; the kernel's manifest only.
    pub api: Option<String>,
    /// The packages it needs, each with its version requirement.
    pub requires: BTreeMap<String, String>,
    /// The Cargo features of flint-player and flint-cli that turn it on.
    pub features: Vec<String>,
    pub provides: Provides,
    /// The manifest it was read from.
    pub file: PathBuf,
}

impl Package {
    /// The folder the manifest is in: where, from #1332, its schemas are.
    pub fn dir(&self) -> &Path {
        self.file.parent().unwrap_or(Path::new(""))
    }
}

/// Read a manifest through the vfs.
pub fn read(file: &Path) -> Result<Package, Vec<Diagnostic>> {
    match vfs::read_to_string(file) {
        Ok(text) => parse(&text, file),
        Err(e) => Err(vec![Diagnostic::error(
            codes::BAD_PACKAGE,
            format!("could not be read: {e}"),
        )
        .file(file)]),
    }
}

/// Parse a manifest's text; `file` is named in each diagnostic.
pub fn parse(text: &str, file: &Path) -> Result<Package, Vec<Diagnostic>> {
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            return Err(vec![Diagnostic::error(
                codes::BAD_PACKAGE,
                format!("not TOML: {}", e.message()),
            )
            .file(file)])
        }
    };
    let mut p = Parse {
        file,
        problems: Vec::new(),
    };
    let package = p.package(&table);
    p.unknown_tables(&table);
    let requires = p.requires(&table, package.as_ref().map(|(name, _)| name.as_str()));
    let features = p.features(&table);
    let provides = p.provides(&table);
    match package {
        Some((name, head)) if p.problems.is_empty() => Ok(Package {
            name,
            version: head.version,
            tier: head.tier,
            summary: head.summary,
            engine: head.engine,
            api: head.api,
            requires,
            features,
            provides,
            file: file.to_path_buf(),
        }),
        _ => Err(p.problems),
    }
}

/// Whether `name` is a package name: `[a-z][a-z0-9-]*`.
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The engine's package folder for a project at `root`: the `engine/`
/// subtree's (a game), else the root's own when the root is an engine
/// checkout. `None` when neither is there.
pub fn engine_packages_dir(root: &Path) -> Option<PathBuf> {
    let subtree = root.join("engine").join(PACKAGES_DIR);
    if vfs::is_dir(&subtree) {
        return Some(subtree);
    }
    let own = root.join(PACKAGES_DIR);
    (crate::project::is_engine_checkout(root) && vfs::is_dir(&own)).then_some(own)
}

/// Every manifest of a package folder, read and checked against each other.
#[derive(Debug, Clone, Default)]
pub struct Packages {
    /// The folder read: `<dir>/<name>/package.toml` for each.
    pub dir: PathBuf,
    /// The manifests that read, by tier and then name.
    pub packages: Vec<Package>,
    /// Every mistake, each file's and those between files.
    pub problems: Vec<Diagnostic>,
}

impl Packages {
    /// Read `dir/*/package.toml`. A folder without a manifest is skipped;
    /// a manifest that doesn't read is left out of `packages` and its
    /// mistakes are in `problems`. Then, across the manifests: a name used
    /// twice, a name other than its folder's, a requirement naming no
    /// package, more than one kernel, and an item two packages provide.
    pub fn read(dir: &Path) -> Packages {
        let mut out = Packages {
            dir: dir.to_path_buf(),
            ..Packages::default()
        };
        let mut folders = match vfs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                out.problems.push(
                    Diagnostic::error(codes::BAD_PACKAGE, format!("no package folder here: {e}"))
                        .file(dir),
                );
                return out;
            }
        };
        folders.sort();
        for folder in folders {
            let file = folder.join(MANIFEST_FILE);
            if !vfs::is_file(&file) {
                continue;
            }
            match read(&file) {
                Ok(package) => out.packages.push(package),
                Err(problems) => out.problems.extend(problems),
            }
        }
        out.packages
            .sort_by(|a, b| (a.tier, &a.name).cmp(&(b.tier, &b.name)));
        out.problems.extend(across(dir, &out.packages));
        out
    }

    pub fn get(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// The package that provides `item` of `kind`, if one does.
    pub fn owner(&self, kind: Kind, item: &str) -> Option<&Package> {
        self.packages
            .iter()
            .find(|p| p.provides.of(kind).iter().any(|i| i == item))
    }

    /// Whether any problem is an error.
    pub fn failed(&self) -> bool {
        crate::diagnostic::has_errors(&self.problems)
    }
}

/// The checks between manifests (see [`Packages::read`]).
fn across(dir: &Path, packages: &[Package]) -> Vec<Diagnostic> {
    let mut problems = Vec::new();
    let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let mut seen: BTreeMap<&str, &Path> = BTreeMap::new();
    for p in packages {
        if let Some(first) = seen.insert(&p.name, &p.file) {
            problems.push(
                Diagnostic::error(
                    codes::BAD_PACKAGE,
                    format!(
                        "the package `{}` is also named in {}: a name is one package",
                        p.name,
                        near(dir, first)
                    ),
                )
                .file(&p.file)
                .field("package.name"),
            );
        }
        let folder = p.dir().file_name().and_then(|f| f.to_str());
        if folder.is_some_and(|f| f != p.name) {
            problems.push(
                Diagnostic::error(
                    codes::BAD_PACKAGE,
                    format!(
                        "the package `{}` is in the folder `{}`: a package's folder is its name",
                        p.name,
                        folder.unwrap_or_default()
                    ),
                )
                .file(&p.file)
                .field("package.name"),
            );
        }
        for wanted in p.requires.keys() {
            if !names.contains(&wanted.as_str()) {
                problems.push(
                    Diagnostic::error(
                        codes::BAD_PACKAGE,
                        format!("requires `{wanted}`, and no package is named that"),
                    )
                    .file(&p.file)
                    .field(format!("requires.{wanted}"))
                    .hint_opt(did_you_mean(wanted, names.iter().copied())),
                );
            }
        }
    }
    let kernels: Vec<&Package> = packages.iter().filter(|p| p.tier == Tier::Kernel).collect();
    for extra in kernels.iter().skip(1) {
        problems.push(
            Diagnostic::error(
                codes::BAD_PACKAGE,
                format!(
                    "a second kernel (the first is `{}`): there is one kernel",
                    kernels[0].name
                ),
            )
            .file(&extra.file)
            .field("package.tier"),
        );
    }
    for kind in Kind::ALL {
        let mut owners: BTreeMap<&str, &Package> = BTreeMap::new();
        for p in packages {
            for item in p.provides.of(kind) {
                if let Some(first) = owners.insert(item, p) {
                    if first.name != p.name {
                        problems.push(
                            Diagnostic::error(
                                codes::BAD_PACKAGE,
                                format!(
                                    "the {} `{item}` is also provided by `{}` ({}): an item has one owner",
                                    kind.noun(),
                                    first.name,
                                    near(dir, &first.file)
                                ),
                            )
                            .file(&p.file)
                            .field(format!("provides.{}", kind.key())),
                        );
                    }
                }
            }
        }
    }
    problems
}

/// `an integer`, `a string`: a TOML type with its article.
fn a(type_str: &str) -> String {
    let article = if type_str.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    };
    format!("{article} {type_str}")
}

/// Another manifest, as a message names it: from the package folder's
/// own name down (`packages/procgen/package.toml`), wherever it is.
fn near(dir: &Path, file: &Path) -> String {
    let below = file.strip_prefix(dir).unwrap_or(file);
    let named = match dir.file_name() {
        Some(name) if below != file => Path::new(name).join(below),
        _ => below.to_path_buf(),
    };
    crate::project::shown(&named)
}

/// `[package]`'s values, before the package is whole.
struct Head {
    version: String,
    tier: Tier,
    summary: String,
    engine: Option<String>,
    api: Option<String>,
}

struct Parse<'a> {
    file: &'a Path,
    problems: Vec<Diagnostic>,
}

impl Parse<'_> {
    fn error(&mut self, field: impl Into<String>, message: impl Into<String>) -> &mut Diagnostic {
        self.problems.push(
            Diagnostic::error(codes::BAD_PACKAGE, message)
                .file(self.file)
                .field(field),
        );
        self.problems.last_mut().unwrap()
    }

    fn unknown_tables(&mut self, table: &toml::Table) {
        for key in table.keys() {
            if !TABLES.contains(&key.as_str()) {
                let hint = did_you_mean(key, TABLES);
                self.error(
                    key.clone(),
                    format!(
                        "unknown table `[{key}]` (the tables are [package], [requires], [build], [provides])"
                    ),
                )
                .hint = hint;
            }
        }
    }

    /// A sub-table, or `None` with a mistake when it is something else.
    fn sub<'t>(&mut self, table: &'t toml::Table, key: &str) -> Option<&'t toml::Table> {
        match table.get(key)? {
            toml::Value::Table(t) => Some(t),
            other => {
                self.error(
                    key,
                    format!("`{key}` is {}, not a table", a(other.type_str())),
                );
                None
            }
        }
    }

    fn unknown_keys(&mut self, table: &toml::Table, at: &str, known: &[&str]) {
        for key in table.keys() {
            if !known.contains(&key.as_str()) {
                let hint = did_you_mean(key, known.iter().copied());
                let list = known.join(", ");
                self.error(
                    format!("{at}.{key}"),
                    format!("unknown key `{key}` in [{at}] (the keys are {list})"),
                )
                .hint = hint;
            }
        }
    }

    fn string(&mut self, table: &toml::Table, at: &str, key: &str) -> Option<String> {
        match table.get(key)? {
            toml::Value::String(s) => Some(s.clone()),
            other => {
                self.error(
                    format!("{at}.{key}"),
                    format!("a string, not {}", a(other.type_str())),
                );
                None
            }
        }
    }

    fn required(&mut self, table: &toml::Table, at: &str, key: &str, what: &str) -> Option<String> {
        if !table.contains_key(key) {
            self.error(format!("{at}.{key}"), format!("missing: {what}"));
            return None;
        }
        self.string(table, at, key)
    }

    fn package(&mut self, table: &toml::Table) -> Option<(String, Head)> {
        let Some(head) = self.sub(table, "package") else {
            if !table.contains_key("package") {
                self.error(
                    "package",
                    "no [package] table: a manifest names its package, version, tier and summary",
                );
            }
            return None;
        };
        self.unknown_keys(head, "package", &PACKAGE_KEYS);
        let name = self.required(
            head,
            "package",
            "name",
            "the package's name, the key a game lists",
        );
        let version = self.required(head, "package", "version", "its semver, `0.1.0`");
        let tier = self.required(head, "package", "tier", "kernel, standard, feature or game");
        let summary = self.required(head, "package", "summary", "one line on what it is");
        let engine = self.string(head, "package", "engine");
        let api = self.string(head, "package", "api");

        if let Some(name) = &name {
            if !is_name(name) {
                self.error(
                    "package.name",
                    format!("`{name}` is no package name: a lowercase letter, then lowercase letters, digits and `-`"),
                );
            }
        }
        if let Some(v) = &version {
            if let Err(e) = semver::Version::parse(v) {
                self.error(
                    "package.version",
                    format!("`{v}` is no semver version: {e}"),
                );
            }
        }
        let tier = tier.and_then(|t| match Tier::parse(&t) {
            Some(tier) => Some(tier),
            None => {
                let hint = did_you_mean(&t, Tier::ALL.map(Tier::as_str));
                self.error(
                    "package.tier",
                    format!("`{t}` is no tier (the tiers are kernel, standard, feature, game)"),
                )
                .hint = hint;
                None
            }
        });
        if let Some(s) = &summary {
            if s.trim().is_empty() || s.contains('\n') {
                self.error("package.summary", "one line, not empty");
            }
        }
        if let Some(e) = &engine {
            if let Err(err) = semver::VersionReq::parse(e) {
                self.error(
                    "package.engine",
                    format!("`{e}` is no version requirement: {err}"),
                );
            }
        }
        if let Some(a) = &api {
            if let Err(err) = semver::Version::parse(a) {
                self.error("package.api", format!("`{a}` is no semver version: {err}"));
            }
        }
        match tier {
            Some(Tier::Kernel) => {
                if engine.is_some() {
                    self.error(
                        "package.engine",
                        "the kernel is the engine API: it states `api`, not a range of it",
                    );
                }
                if !head.contains_key("api") {
                    self.error(
                        "package.api",
                        "missing: the kernel states the engine API version, `api = \"0.1.0\"`",
                    );
                }
            }
            Some(_) => {
                if !head.contains_key("engine") {
                    self.error(
                        "package.engine",
                        "missing: the engine API range it builds against, `engine = \"^0.1\"`",
                    );
                }
                if api.is_some() {
                    self.error(
                        "package.api",
                        "only the kernel states the engine API version; a package states its range in `engine`",
                    );
                }
            }
            None => {}
        }
        Some((
            name?,
            Head {
                version: version?,
                tier: tier?,
                summary: summary?,
                engine,
                api,
            },
        ))
    }

    fn requires(&mut self, table: &toml::Table, own: Option<&str>) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let Some(requires) = self.sub(table, "requires") else {
            return out;
        };
        for (name, value) in requires {
            let field = format!("requires.{name}");
            if !is_name(name) {
                self.error(field.clone(), format!("`{name}` is no package name"));
            }
            if Some(name.as_str()) == own {
                self.error(field.clone(), "a package does not require itself");
            }
            match value {
                toml::Value::String(req) => match semver::VersionReq::parse(req) {
                    Ok(_) => {
                        out.insert(name.clone(), req.clone());
                    }
                    Err(e) => {
                        self.error(field, format!("`{req}` is no version requirement: {e}"));
                    }
                },
                other => {
                    self.error(
                        field,
                        format!(
                            "a version requirement string (`\"*\"`, `\"^0.1\"`), not {}",
                            a(other.type_str())
                        ),
                    );
                }
            }
        }
        out
    }

    fn strings(&mut self, table: &toml::Table, at: &str, key: &str) -> Vec<String> {
        let field = format!("{at}.{key}");
        let Some(value) = table.get(key) else {
            return Vec::new();
        };
        let Some(items) = value.as_array() else {
            self.error(
                field,
                format!("a list of strings, not {}", a(value.type_str())),
            );
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for (i, item) in items.iter().enumerate() {
            match item.as_str() {
                Some(s) if out.iter().any(|o| o == s) => {
                    self.error(format!("{field}[{i}]"), format!("`{s}` is listed twice"));
                }
                Some(s) if s.trim().is_empty() || s.trim() != s => {
                    self.error(
                        format!("{field}[{i}]"),
                        format!("`{s}` is no name: empty, or with spaces at an end"),
                    );
                }
                Some(s) => out.push(s.to_string()),
                None => {
                    self.error(
                        format!("{field}[{i}]"),
                        format!("a string, not {}", a(item.type_str())),
                    );
                }
            }
        }
        out
    }

    fn features(&mut self, table: &toml::Table) -> Vec<String> {
        let Some(build) = self.sub(table, "build") else {
            return Vec::new();
        };
        self.unknown_keys(build, "build", &BUILD_KEYS);
        self.strings(build, "build", "features")
    }

    fn provides(&mut self, table: &toml::Table) -> Provides {
        let mut out = Provides::default();
        let Some(provides) = self.sub(table, "provides") else {
            return out;
        };
        let keys = Kind::ALL.map(Kind::key);
        self.unknown_keys(provides, "provides", &keys);
        for kind in Kind::ALL {
            *out.of_mut(kind) = self.strings(provides, "provides", kind.key());
        }
        for (i, command) in out.commands.clone().iter().enumerate() {
            let words: Vec<&str> = command.split(' ').collect();
            if words.iter().any(|w| w.is_empty() || w.starts_with('-')) {
                self.error(
                    format!("provides.commands[{i}]"),
                    format!("`{command}` is no command path: words below `flint`, one space apart, no flags"),
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
