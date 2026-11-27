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
//! [rust]                     # where its Rust joins the launchers (ADR 0137, #1554, #1555)
//! player = "flint-living"    # the crate whose `player::PACKAGE` flint-player lists; one of `crates`
//! cli = "flint-living"       # the crate whose `cli::extensions()` flint lists; one of `crates`
//!
//! [provides]                 # what it owns; every item has exactly one owner
//! crates = ["flint-living", "flint-wooze"]
//! components = ["living_prop", "shallow_wooze"]
//! archetypes = []
//! scene_tables = ["living", "wooze"]
//! script_modules = ["living", "wooze", "debug"]
//! commands = []              # paths below `flint`: "music chart", "asset generate"
//! flags = []                 # its flags on a command it doesn't own: "render --ai-refine",
//!                            #   or one value of one: "asset resolve --strategy ai_generate"
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
const TABLES: [&str; 7] = [
    "package", "requires", "build", "rust", "with", "provides", "without",
];

/// The keys `[package]` may hold.
const PACKAGE_KEYS: [&str; 6] = ["name", "version", "tier", "summary", "engine", "api"];

/// The keys `[build]` may hold.
const BUILD_KEYS: [&str; 1] = ["features"];

/// The keys `[rust]` may hold.
const RUST_KEYS: [&str; 2] = ["player", "cli"];

/// The keys a `[with.<package>]` table may hold.
const WITH_KEYS: [&str; 1] = ["features"];

/// The keys `[without]` may hold.
const WITHOUT_KEYS: [&str; 2] = ["does", "policies"];

/// What a player built without the package still knows of it (`[without]`,
/// ADR 0137 decision 8, #1585), so the kernel names no package: the line a
/// scene that uses it hears is written from `[provides]`' components and
/// scene tables and [`does`](Without::does), and each of
/// [`policies`](Without::policies) is a name a game state may give in every
/// build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Without {
    /// What the scene's parts do in a build without the package, after
    /// "the scene's <parts>": `stand still`. `None`: the kernel's words.
    pub does: Option<String>,
    /// The names a game state's policy may pause this package's subsystem
    /// by (#727): `living`. A state naming one registers in a build
    /// without the package too, and does nothing there.
    pub policies: Vec<String>,
}

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
    /// A flag on a command another package (or the kernel) owns (#1572).
    Flag,
    /// A kind of spec file the package makes assets from, as a pattern of
    /// its name (`*.procgen.toml`, #1595): a scene naming one uses it.
    Spec,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Crate,
        Kind::Component,
        Kind::Archetype,
        Kind::SceneTable,
        Kind::ScriptModule,
        Kind::Command,
        Kind::Flag,
        Kind::Spec,
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
            Kind::Flag => "flags",
            Kind::Spec => "specs",
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
            Kind::Flag => "flag",
            Kind::Spec => "spec",
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
    /// Its flags on a command it doesn't own, as a line spells them: the
    /// command's path, the flag and at most one value of it (`render
    /// --ai-refine`, `asset resolve --strategy ai_generate`), #1572.
    pub flags: Vec<String>,
    /// The spec files it makes assets from, each a pattern of a file's
    /// name, `*` and then its ending (`*.procgen.toml`), #1595.
    pub specs: Vec<String>,
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
            Kind::Flag => &self.flags,
            Kind::Spec => &self.specs,
        }
    }

    /// Whether `file` (a path, or a file's name) is a spec file one of
    /// [`Provides::specs`]' patterns names.
    pub fn makes_from(&self, file: &str) -> bool {
        self.specs
            .iter()
            .filter_map(|p| spec_ending(p))
            .any(|ending| file.len() > ending.len() && file.ends_with(ending))
    }

    fn of_mut(&mut self, kind: Kind) -> &mut Vec<String> {
        match kind {
            Kind::Crate => &mut self.crates,
            Kind::Component => &mut self.components,
            Kind::Archetype => &mut self.archetypes,
            Kind::SceneTable => &mut self.scene_tables,
            Kind::ScriptModule => &mut self.script_modules,
            Kind::Command => &mut self.commands,
            Kind::Flag => &mut self.flags,
            Kind::Spec => &mut self.specs,
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
    /// The crate of its own (one of `provides.crates`) whose
    /// `player::PACKAGE` the player launcher lists when the build has the
    /// package (`[rust] player`, #1555); `None`: its player part, if any,
    /// is still in flint-player.
    pub player: Option<String>,
    /// The crate of its own (one of `provides.crates`) whose
    /// `cli::extensions()` the CLI launcher lists when the build has the
    /// package (`[rust] cli`, #1554); `None` adds no command.
    pub cli: Option<String>,
    /// `[with.<package>] features` (ADR 0137 decision 9, #1558): a part of
    /// this package that needs another one listed too, neither requiring
    /// the other, and the Cargo features (`crate/feature`) that turn it on
    /// when the game lists both. Living's `debug` module and panel are its
    /// `[with.debug-hud]`.
    pub with: BTreeMap<String, Vec<String>>,
    pub provides: Provides,
    /// What a build without it still knows of it (`[without]`, #1585).
    pub without: Without,
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
    let (player, cli) = p.rust(&table, &provides);
    let with = p.with(&table, package.as_ref().map(|(name, _)| name.as_str()));
    let without = p.without(&table);
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
            player,
            cli,
            with,
            provides,
            without,
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
        Packages::read_with_game(dir, None)
    }

    /// [`read`](Self::read) of the engine's folder, then of a game's own
    /// `packages/` (#1562), whose every package is `tier = "game"`, checked
    /// against each other as one set.
    pub fn read_with_game(dir: &Path, game: Option<&Path>) -> Packages {
        let mut out = Packages {
            dir: dir.to_path_buf(),
            ..Packages::default()
        };
        out.read_folder(dir, false);
        if let Some(game) = game {
            out.read_folder(game, true);
        }
        out.packages
            .sort_by(|a, b| (a.tier, &a.name).cmp(&(b.tier, &b.name)));
        out.problems.extend(across(dir, &out.packages));
        out
    }

    fn read_folder(&mut self, dir: &Path, game: bool) {
        let mut folders = match vfs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                self.problems.push(
                    Diagnostic::error(codes::BAD_PACKAGE, format!("no package folder here: {e}"))
                        .file(dir),
                );
                return;
            }
        };
        folders.sort();
        for folder in folders {
            let file = folder.join(MANIFEST_FILE);
            if !vfs::is_file(&file) {
                continue;
            }
            match read(&file) {
                Ok(p) if game && p.tier != Tier::Game => self.problems.push(
                    Diagnostic::error(
                        codes::BAD_PACKAGE,
                        format!("a game's own package is `tier = \"game\"`, not {}", p.tier),
                    )
                    .file(&file)
                    .field("package.tier"),
                ),
                Ok(package) => self.packages.push(package),
                Err(problems) => self.problems.extend(problems),
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// The package whose `[provides] specs` names `file` (a path, or a
    /// file's name) as one of its spec files, if one does (#1595).
    pub fn spec_owner(&self, file: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.provides.makes_from(file))
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
        for other in p.with.keys() {
            if !names.contains(&other.as_str()) {
                problems.push(
                    Diagnostic::error(
                        codes::BAD_PACKAGE,
                        format!("has a part with `{other}`, and no package is named that"),
                    )
                    .file(&p.file)
                    .field(format!("with.{other}"))
                    .hint_opt(did_you_mean(other, names.iter().copied())),
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
                        "unknown table `[{key}]` (the tables are [package], [requires], [build], [rust], [with], [provides], [without])"
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

    /// `[rust] player` and `cli`: each a crate the package provides, or a
    /// mistake.
    fn rust(
        &mut self,
        table: &toml::Table,
        provides: &Provides,
    ) -> (Option<String>, Option<String>) {
        let Some(rust) = self.sub(table, "rust") else {
            return (None, None);
        };
        self.unknown_keys(rust, "rust", &RUST_KEYS);
        let mut crate_of = |key: &str| {
            let krate = self.string(rust, "rust", key)?;
            if !provides.crates.contains(&krate) {
                self.error(
                    format!("rust.{key}"),
                    format!("`{krate}` is no crate of this package: name one of [provides] crates"),
                );
            }
            Some(krate)
        };
        (crate_of("player"), crate_of("cli"))
    }

    /// `[with.<package>]`: each a table of `features`, naming a package
    /// other than this one (whether it is one is the cross-file check's).
    fn with(&mut self, table: &toml::Table, own: Option<&str>) -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        let Some(with) = self.sub(table, "with") else {
            return out;
        };
        for (name, part) in with {
            let field = format!("with.{name}");
            if Some(name.as_str()) == own {
                self.error(
                    field,
                    "a package's part with itself is the package: put it in [build]",
                );
                continue;
            }
            let Some(part) = part.as_table() else {
                self.error(field, format!("a table, not {}", a(part.type_str())));
                continue;
            };
            self.unknown_keys(part, &field, &WITH_KEYS);
            out.insert(name.clone(), self.strings(part, &field, "features"));
        }
        out
    }

    /// `[without]`: `does`, a phrase, and `policies`, names.
    fn without(&mut self, table: &toml::Table) -> Without {
        let Some(without) = self.sub(table, "without") else {
            return Without::default();
        };
        self.unknown_keys(without, "without", &WITHOUT_KEYS);
        let does = self.string(without, "without", "does");
        if does.as_deref().is_some_and(|d| d.trim().is_empty()) {
            self.error(
                "without.does",
                "empty: say what the parts do, or leave the key out",
            );
        }
        Without {
            does,
            policies: self.strings(without, "without", "policies"),
        }
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
        for (i, flag) in out.flags.clone().iter().enumerate() {
            if FlagLine::parse(flag).is_none() {
                self.error(
                    format!("provides.flags[{i}]"),
                    format!(
                        "`{flag}` is no flag line: a command's words below `flint`, then `--flag`, \
                         then at most one value, one space apart"
                    ),
                );
            }
        }
        for (i, spec) in out.specs.clone().iter().enumerate() {
            if spec_ending(spec).is_none() {
                self.error(
                    format!("provides.specs[{i}]"),
                    format!(
                        "`{spec}` is no spec pattern: `*` and then the files' ending, as \
                         `*.procgen.toml`"
                    ),
                );
            }
        }
        out
    }
}

/// A `[provides] specs` pattern's ending: `.procgen.toml` of
/// `*.procgen.toml`. `None` for anything else: the one `*` comes first, then
/// a `.`, and no folder.
pub fn spec_ending(pattern: &str) -> Option<&str> {
    pattern
        .strip_prefix('*')
        .filter(|e| e.len() > 1 && e.starts_with('.') && !e.contains(['*', '/', '\\', ' ']))
}

/// A `[provides] flags` line read: `asset resolve --strategy ai_generate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagLine<'a> {
    /// The command's words below `flint`: `["asset", "resolve"]`.
    pub command: Vec<&'a str>,
    /// The flag as written, dashes and all: `--strategy`.
    pub flag: &'a str,
    /// One value of it, when the line names one: `ai_generate`.
    pub value: Option<&'a str>,
}

impl<'a> FlagLine<'a> {
    /// The line, or `None` when it isn't one.
    pub fn parse(line: &'a str) -> Option<Self> {
        let words: Vec<&str> = line.split(' ').collect();
        let at = words.iter().position(|w| w.starts_with("--"))?;
        let (command, rest) = words.split_at(at);
        let named = |w: &&str| !w.is_empty() && !w.starts_with('-');
        let flag = rest[0];
        let ok = !command.is_empty()
            && command.iter().all(named)
            && flag.len() > 2
            && rest.len() <= 2
            && rest[1..].iter().all(named);
        ok.then(|| Self {
            command: command.to_vec(),
            flag,
            value: rest.get(1).copied(),
        })
    }

    /// Whether the line's value names `given`: the same word, or for a
    /// value ending `.*` (`render --sweep wooze.*`, #1595) any word that
    /// starts with what comes before the `*`, more after it.
    pub fn names_value(&self, given: &str) -> bool {
        match self.value {
            Some(value) => match value.strip_suffix('*').filter(|p| p.ends_with('.')) {
                Some(prefix) => given.len() > prefix.len() && given.starts_with(prefix),
                None => value == given,
            },
            None => false,
        }
    }
}

#[cfg(test)]
mod tests;
