//! `[packages]` in `flint.toml`: which packages a game uses (ADR 0136
//! decision 9, #1334).
//!
//! ```toml
//! [packages]
//! living = "*"      # a feature package, any version
//! debug-hud = false # drop a standard package
//! ```
//!
//! [`PackageList`] is the table as written, checked for its shape when the
//! manifest is read (a name, a version requirement's syntax, `false`).
//! [`InUse`] is that list held to the engine's manifests: the kernel, every
//! standard package not set to `false`, every feature package listed, and
//! what those require. A name no package has, a `false` that can't be, and a
//! requirement left out are its errors, each a `bad-manifest` naming the
//! line. With no `[packages]` key there is no `InUse` and every package is
//! in, as before the key existed.
//!
//! What it decides: which packages' schemas load
//! ([`crate::project::schema_dirs`]), which script modules `flint script
//! api` lists and `flint script check` accepts, and how `flint check` words
//! a component, scene table or script function of a package left out:
//! `unlisted-package`, with the line to add ([`InUse::name_unlisted`]).

use super::{Kind, Package, Packages, Tier};
use crate::diagnostic::{codes, did_you_mean, Diagnostic};
use crate::project::shown;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The table `flint.toml` lists packages in.
pub const TABLE: &str = "packages";

/// What one line of `[packages]` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// A version requirement (`"*"`, `"^0.1"`), its syntax checked. Phase 1
    /// stores it; nothing is held to it until packages version apart.
    Version(String),
    /// `false`: a standard package left out.
    Off,
}

/// One line of `[packages]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub wanted: Wanted,
}

/// `[packages]` as `flint.toml` writes it, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageList {
    /// The manifest it is in.
    pub file: PathBuf,
    pub entries: Vec<Listed>,
}

/// Why a `[packages]` line can't be read: its dotted key and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListError {
    pub key: String,
    pub message: String,
}

impl PackageList {
    /// Read the `[packages]` value of `file`: a table of names to a version
    /// requirement or `false`. A table value (`{ version, path }`) is
    /// refused with the phase that brings it.
    pub fn read(value: &toml::Value, file: &Path) -> Result<PackageList, ListError> {
        let Some(table) = value.as_table() else {
            return Err(ListError {
                key: TABLE.into(),
                message: format!(
                    "must be a table of package names (`[packages]`, then `living = \"*\"`), not {}",
                    super::a(value.type_str())
                ),
            });
        };
        let mut entries = Vec::new();
        for (name, value) in table {
            let key = format!("{TABLE}.{name}");
            if !super::is_name(name) {
                return Err(ListError {
                    key,
                    message: format!(
                        "`{name}` is no package name (lowercase letters, digits and `-`)"
                    ),
                });
            }
            let wanted = match value {
                toml::Value::String(req) => match semver::VersionReq::parse(req) {
                    Ok(_) => Wanted::Version(req.clone()),
                    Err(e) => {
                        return Err(ListError {
                            key,
                            message: format!(
                                "`{req}` is no version requirement ({e}); write `\"*\"` for any"
                            ),
                        })
                    }
                },
                toml::Value::Boolean(false) => Wanted::Off,
                toml::Value::Boolean(true) => {
                    return Err(ListError {
                        key,
                        message: format!(
                            "a package is listed with a version requirement: `{name} = \"*\"`, \
                             not `true`"
                        ),
                    })
                }
                toml::Value::Table(_) => {
                    return Err(ListError {
                        key,
                        message: "a table (`{ version, path }`) is for a game's own packages \
                                  (Phase 2) and package sources (Phase 5); Phase 1 takes a \
                                  version requirement (`\"*\"`) or `false`"
                            .into(),
                    })
                }
                other => {
                    return Err(ListError {
                        key,
                        message: format!(
                            "a version requirement (`\"*\"`) or `false`, not {}",
                            super::a(other.type_str())
                        ),
                    })
                }
            };
            entries.push(Listed {
                name: name.clone(),
                wanted,
            });
        }
        Ok(PackageList {
            file: file.to_path_buf(),
            entries,
        })
    }

    pub fn get(&self, name: &str) -> Option<&Listed> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Whether `[packages]` sets `name = false`.
    pub fn is_off(&self, name: &str) -> bool {
        self.get(name).is_some_and(|e| e.wanted == Wanted::Off)
    }
}

/// The packages a game is built and checked with: a [`PackageList`] held
/// to the engine's manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InUse {
    /// The `flint.toml` whose `[packages]` decided it.
    pub manifest: PathBuf,
    /// Every package in, by name: the kernel, the standard packages not
    /// set to `false`, the feature packages listed and what they require.
    pub on: BTreeSet<String>,
    /// The manifests of every package left out, by tier then name.
    pub left_out: Vec<Package>,
    /// The names set to `false`.
    pub off: BTreeSet<String>,
}

impl InUse {
    /// Hold `list` to the manifests of `packages`. Every mistake at once,
    /// each a `bad-manifest` on `list.file` with the line's key as field.
    pub fn resolve(list: &PackageList, packages: &Packages) -> Result<InUse, Vec<Diagnostic>> {
        let mut problems = Vec::new();
        let problem = |key: &str, message: String| {
            Diagnostic::error(codes::BAD_MANIFEST, message)
                .file(&list.file)
                .field(format!("{TABLE}.{key}"))
        };
        let names = || packages.packages.iter().map(|p| p.name.as_str());
        let mut on: BTreeSet<String> = BTreeSet::new();
        for p in &packages.packages {
            if matches!(p.tier, Tier::Kernel | Tier::Standard) {
                on.insert(p.name.clone());
            }
        }
        for entry in &list.entries {
            let Some(package) = packages.get(&entry.name) else {
                let all: Vec<&str> = names().collect();
                problems.push(
                    problem(
                        &entry.name,
                        format!(
                            "no package `{}` (the packages are {})",
                            entry.name,
                            all.join(", ")
                        ),
                    )
                    .hint_opt(did_you_mean(&entry.name, names())),
                );
                continue;
            };
            match (&entry.wanted, package.tier) {
                (Wanted::Version(_), _) => {
                    on.insert(package.name.clone());
                }
                (Wanted::Off, Tier::Standard) if !package.features.is_empty() => {
                    on.remove(&package.name);
                }
                (Wanted::Off, Tier::Standard) => problems.push(problem(
                    &entry.name,
                    format!(
                        "`{}` can't be left out yet: it has no build switch until Phase 3",
                        entry.name
                    ),
                )),
                (Wanted::Off, Tier::Kernel) => problems.push(problem(
                    &entry.name,
                    format!("`{}` is the kernel: every game has it", entry.name),
                )),
                (Wanted::Off, tier) => problems.push(
                    problem(
                        &entry.name,
                        format!(
                            "`{}` is a {tier} package: it is out unless listed, so `false` \
                             says nothing",
                            entry.name
                        ),
                    )
                    .hint(format!("remove the line `{} = false`", entry.name)),
                ),
            }
        }
        // What the listed packages need comes with them, unless it was
        // left out by name
        let mut queue: Vec<String> = on.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            let Some(package) = packages.get(&name) else {
                continue;
            };
            for needed in package.requires.keys() {
                if list.is_off(needed) {
                    problems.push(
                        problem(
                            needed,
                            format!(
                                "`{name}` requires `{needed}`, which `{needed} = false` leaves out"
                            ),
                        )
                        .hint(format!(
                            "remove `{needed} = false`, or leave `{name}` out as well"
                        )),
                    );
                } else if on.insert(needed.clone()) {
                    queue.push(needed.clone());
                }
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        let left_out = packages
            .packages
            .iter()
            .filter(|p| !on.contains(&p.name))
            .cloned()
            .collect();
        let off = list
            .entries
            .iter()
            .filter(|e| e.wanted == Wanted::Off)
            .map(|e| e.name.clone())
            .collect();
        Ok(InUse {
            manifest: list.file.clone(),
            on,
            left_out,
            off,
        })
    }

    pub fn is_on(&self, package: &str) -> bool {
        self.on.contains(package)
    }

    /// The left-out package that provides `item`, if one does.
    pub fn left_out_owner(&self, kind: Kind, item: &str) -> Option<&Package> {
        self.left_out
            .iter()
            .find(|p| p.provides.of(kind).iter().any(|i| i == item))
    }

    /// What to write to bring `package` in: `add `living = "*"` under
    /// [packages] in flint.toml`, or for a package set to `false`, to take
    /// that line out.
    pub fn hint_for(&self, package: &str) -> String {
        // Spelled from the working directory where it lies under it
        let manifest = std::env::current_dir()
            .ok()
            .and_then(|cwd| self.manifest.strip_prefix(cwd).ok().map(shown))
            .unwrap_or_else(|| shown(&self.manifest));
        if self.off.contains(package) {
            format!("remove `{package} = false` from [packages] in {manifest}")
        } else {
            format!("add `{package} = \"*\"` under [packages] in {manifest}")
        }
    }

    /// The error for `item` of `kind`, which `package` provides and the
    /// list leaves out: one wording for every place that says it.
    pub fn unlisted(&self, kind: Kind, item: &str, package: &str) -> Diagnostic {
        Diagnostic::error(
            codes::UNLISTED_PACKAGE,
            format!(
                "{} `{item}` is the `{package}` package's, and [packages] doesn't list it",
                article_noun(kind)
            ),
        )
        .hint(self.hint_for(package))
    }

    /// Turn each `unknown-component`, `unknown-table` and
    /// `unknown-archetype` whose name a left-out package provides into the
    /// `unlisted-package` error that names the package and the line to add,
    /// keeping its file, entity, component and place. One error a file and
    /// item: the first entity's, counting the others.
    pub fn name_unlisted(&self, found: &mut Vec<Diagnostic>) {
        for d in found.iter_mut() {
            let (kind, item) = match d.code {
                codes::UNKNOWN_COMPONENT => (Kind::Component, d.component.clone()),
                codes::UNKNOWN_TABLE => (Kind::SceneTable, d.table.clone()),
                codes::UNKNOWN_ARCHETYPE => (Kind::Archetype, quoted(&d.message)),
                _ => continue,
            };
            let Some(item) = item else {
                continue;
            };
            let Some(package) = self.left_out_owner(kind, &item) else {
                continue;
            };
            let named = self.unlisted(kind, &item, &package.name);
            d.severity = named.severity;
            d.code = named.code;
            d.message = named.message;
            d.hint = named.hint;
        }
        // The same item in many entities is one line to add: say it once
        let key = |d: &Diagnostic| {
            (d.code == codes::UNLISTED_PACKAGE).then(|| {
                (
                    d.file.clone(),
                    d.component.clone(),
                    d.table.clone(),
                    d.message.clone(),
                )
            })
        };
        let mut kept: Vec<Diagnostic> = Vec::with_capacity(found.len());
        let mut others: Vec<usize> = Vec::new();
        for d in found.drain(..) {
            let k = key(&d);
            match k
                .as_ref()
                .and_then(|k| kept.iter().position(|e| key(e).as_ref() == Some(k)))
            {
                Some(at) => others[at] += 1,
                None => {
                    kept.push(d);
                    others.push(0);
                }
            }
        }
        for (d, n) in kept.iter_mut().zip(others) {
            if n > 0 {
                let entities = if n == 1 { "entity" } else { "entities" };
                d.message.push_str(&format!(" ({n} other {entities} too)"));
            }
        }
        *found = kept;
    }
}

/// `the component`, `the scene table`, `the script module`.
fn article_noun(kind: Kind) -> String {
    format!("the {}", kind.noun())
}

/// The first `` `name` `` a message quotes: how `unknown-archetype` names
/// its archetype.
fn quoted(message: &str) -> Option<String> {
    let start = message.find('`')? + 1;
    let len = message[start..].find('`')?;
    Some(message[start..start + len].to_string())
}

#[cfg(test)]
#[path = "in_use_tests.rs"]
mod tests;
