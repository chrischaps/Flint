//! `flint script check` (#694, ADR 0116 step 4): what is wrong with a
//! script before the game runs it.
//!
//! Rhai resolves a call when the line runs, so a misspelled function, a call
//! with one argument too few or an `fn on_colision` is found by playing to
//! it, or never: a callback the engine doesn't know is simply not called.
//! This compiles each file with Flint's parser settings and walks its syntax
//! tree (Rhai's `internals`), holding every call to what can answer it:
//!
//! - the declared API ([`ApiListing`]: the registry, at each arity a
//!   function is declared at, and anything an extension registered raw);
//! - Rhai's own package functions, from the engine's metadata (never a
//!   hand list), and the few names Rhai resolves itself ([`RHAI_KEYWORDS`]);
//! - the file's own `fn`s, and those of the modules it imports, through the
//!   project's search for `scripts/lib/<name>.rhai` ([`ModuleBase`]);
//! - names a host allows ([`Api::allow`], [`Api::add`]): a game binary's
//!   extension functions, which the engine's registry has never heard of.
//!
//! A method call `x.f(a)` is `f(x, a)` to a native function and `f(a)` with
//! `this` to a script one, so it is matched at both arities. What it
//! reports, each a [`Diagnostic`] at a line and column:
//!
//! | Code | Severity | |
//! |---|---|---|
//! | `script-syntax` | error | the file doesn't compile |
//! | `unknown-function` | error | no such function; the hint is the nearest name |
//! | `wrong-arity` | error | the function is there at other argument counts; the hint lists them |
//! | `bad-argument` | error | a literal no declaration accepts (`"x"` for a num, `0.5` for an int) |
//! | `unknown-callback` | error | `fn on_colision`: one edit from a callback, never called |
//! | `callback-arity` | error | a callback with a parameter count the engine doesn't call |
//! | `missing-module` | error | `import "x"` finds no file; `tried` has the paths |
//! | `deprecated-function` | warning | a function the API marks deprecated |
//! | `mixed-numbers` | warning | a per-frame call mixing int and float literals in four or more `num` slots |
//!
//! The last is ADR 0116 decision 5: such a call misses both exact
//! signatures the builder registers (all floats, all ints) and takes Rhai's
//! search over 2^n `Dynamic` mixes, ~4 µs a call instead of ~0.4. Only
//! literals are judged, only in `on_update` and `on_draw_ui` and the
//! file's functions they call, so it stays cheap and quiet.

use crate::api::registry::{parse_signature, ApiEntry, ApiListing, CALLBACKS};
use crate::module_resolver::ModuleBase;
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::vfs;
use rhai::{ASTNode, Engine, Expr, FnCallExpr, OptimizationLevel, Stmt, AST};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Rhai syntax that looks like a call, and the functions Rhai resolves
/// itself rather than through a package (any number of arguments).
pub const RHAI_KEYWORDS: &[&str] = &[
    "if",
    "else",
    "while",
    "for",
    "in",
    "loop",
    "do",
    "until",
    "switch",
    "return",
    "fn",
    "let",
    "const",
    "throw",
    "try",
    "catch",
    "import",
    "export",
    "as",
    "private",
    "this",
    "global",
    "Fn",
    "call",
    "curry",
    "is_def_fn",
    "is_def_var",
    "is_shared",
    "type_of",
    "print",
    "debug",
    "eval",
];

/// The callbacks run every frame: what decision 5's warning looks inside.
const PER_FRAME: &[&str] = &[
    flint_core::callbacks::ON_UPDATE,
    flint_core::callbacks::ON_DRAW_UI,
];

/// How many `num` parameters make a function's mixed call slow: the
/// builder's all-int copy starts at four (ADR 0116 decision 5).
const MANY_NUMS: usize = 4;

/// The signatures Rhai's packages register: everything an engine lists with
/// packages (`gen_fn_signatures(true)`), less what it lists without them
/// (Flint's and a host's).
pub fn package_signatures(with_packages: Vec<String>, own: Vec<String>) -> Vec<String> {
    let own: BTreeSet<String> = own.into_iter().collect();
    with_packages
        .into_iter()
        .filter(|s| !own.contains(s))
        .collect()
}

/// What a script's call may name: see the module docs.
#[derive(Debug, Clone, Default)]
pub struct Api {
    /// The declared functions, every overload, by name.
    declared: BTreeMap<String, Vec<ApiEntry>>,
    /// Functions known by name and arity only: an extension's raw
    /// registration, or a game's listing ([`Api::add`]).
    other: BTreeMap<String, BTreeSet<usize>>,
    /// Rhai's package functions, by name and arity.
    builtins: BTreeMap<String, BTreeSet<usize>>,
    /// Names any call may use at any arity ([`Api::allow`]).
    allowed: BTreeSet<String>,
}

impl Api {
    /// From a listing (`ScriptSystem::api_listing`) and the signatures of
    /// Rhai's own packages ([`package_signatures`]).
    pub fn new(listing: &ApiListing, packages: &[String]) -> Self {
        let mut api = Api::default();
        for e in &listing.declared {
            api.declared
                .entry(e.name.to_string())
                .or_default()
                .push(e.clone());
        }
        for u in &listing.undeclared {
            api.add(&u.name, u.params.len());
        }
        for f in packages.iter().filter_map(|sig| parse_signature(sig)) {
            // `get$len`, `set$x`, `index$get$`: property and index access,
            // never written as a call
            if !f.name.contains('$') {
                api.builtins
                    .entry(f.name)
                    .or_default()
                    .insert(f.params.len());
            }
        }
        api
    }

    /// Everything `system` can call now: Flint's API, any extension's, and
    /// Rhai's packages.
    pub fn of(system: &mut crate::ScriptSystem) -> Self {
        let listing = system.api_listing();
        let engine = system.rhai_engine_mut();
        let packages = package_signatures(
            engine.gen_fn_signatures(true),
            engine.gen_fn_signatures(false),
        );
        Self::new(&listing, &packages)
    }

    /// Let any call of `name` pass, at any arity: a function a game binary
    /// registers that this API doesn't list.
    pub fn allow(&mut self, name: impl Into<String>) {
        self.allowed.insert(name.into());
    }

    /// Know `name` at `arity`: a function from a game's own `flint script
    /// api --json` listing.
    pub fn add(&mut self, name: &str, arity: usize) {
        self.other
            .entry(name.to_string())
            .or_default()
            .insert(arity);
    }

    /// How many functions it knows by name, Rhai's packages aside.
    pub fn len(&self) -> usize {
        let mut names: BTreeSet<&str> = self.declared.keys().map(String::as_str).collect();
        names.extend(self.other.keys().map(String::as_str));
        names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every name a call could have meant, for "did you mean".
    fn names(&self) -> impl Iterator<Item = &str> {
        self.declared
            .keys()
            .chain(self.other.keys())
            .chain(self.builtins.keys())
            .map(String::as_str)
    }

    /// The arities `name` is known at outside the declared API.
    fn other_arities(&self, name: &str) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        for map in [&self.other, &self.builtins] {
            if let Some(set) = map.get(name) {
                out.extend(set.iter().copied());
            }
        }
        out
    }
}

/// One script to check, and where its imports are looked for.
#[derive(Debug, Clone)]
pub struct ScriptFile {
    pub path: PathBuf,
    /// The search `import "name"` goes through, as the player sets it for
    /// the scene that runs the script.
    pub base: ModuleBase,
    /// A module (`scripts/lib/`, or imported): the engine never calls its
    /// callbacks, so its `fn on_...` are not judged as callbacks.
    pub module: bool,
}

impl ScriptFile {
    /// `path` as a scene in `scene_dir` of `project` would run it.
    pub fn in_scene(
        path: PathBuf,
        project: flint_core::project::Project,
        scene_dir: &Path,
    ) -> Self {
        let module = is_lib(&path);
        Self {
            path,
            base: ModuleBase::Project {
                project,
                scene_dir: scene_dir.to_path_buf(),
            },
            module,
        }
    }

    /// A script found on its own (every `.rhai` of a project), with the
    /// folder a scene running it would be in: the one holding `scripts/`.
    pub fn found(path: PathBuf) -> Self {
        let project = flint_core::project::Project::for_scene(&path);
        let scene_dir = scene_dir_of(&path);
        Self::in_scene(path, project, &scene_dir)
    }
}

/// Whether `path` is in a `lib` folder: a module, imported and not run.
fn is_lib(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "lib")
}

/// The folder a scene running `script` would be in: `levels/` for
/// `levels/scripts/a.rhai` and `levels/scripts/lib/m.rhai` alike, where the
/// project's search finds `scripts/` beside it.
fn scene_dir_of(script: &Path) -> PathBuf {
    let mut dir = script.parent().unwrap_or(Path::new("")).to_path_buf();
    if dir.file_name().is_some_and(|n| n == "lib") {
        dir.pop();
    }
    if dir.file_name().is_some_and(|n| n == "scripts") {
        dir.pop();
    }
    if dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        dir
    }
}

/// What a run found.
#[derive(Debug, Clone, Default)]
pub struct Checked {
    /// Every file read, scripts and the modules they import, in order.
    pub files: Vec<PathBuf>,
    /// Each file's problems in line order, the files in the order read.
    pub diagnostics: Vec<Diagnostic>,
}

/// Holds scripts to an [`Api`]. Compiles as the player does, with the
/// optimizer off so the tree is what was written.
pub struct Checker {
    api: Api,
    engine: Engine,
}

impl Checker {
    pub fn new(api: Api) -> Self {
        let mut engine = crate::engine::compiler();
        engine.set_optimization_level(OptimizationLevel::None);
        Self { api, engine }
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    /// Check `files` and every module they import, each file once.
    pub fn check(&self, files: Vec<ScriptFile>) -> Checked {
        let mut run = Run {
            checker: self,
            parsed: HashMap::new(),
            out: Checked::default(),
        };
        let mut queue: std::collections::VecDeque<ScriptFile> = files.into();
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        while let Some(file) = queue.pop_front() {
            if !seen.insert(normal(&file.path)) {
                continue;
            }
            queue.extend(run.file(&file));
        }
        run.out
    }

    /// Check one script's text, as if it were at `path`, with no imports
    /// followed: for tests and a host's own checks.
    pub fn check_source(&self, path: &Path, source: &str, module: bool) -> Vec<Diagnostic> {
        let mut found = Vec::new();
        match self.engine.compile(source) {
            Err(e) => found.push(syntax_error(path, &e)),
            Ok(ast) => {
                let file = ScriptFile {
                    path: path.to_path_buf(),
                    base: ModuleBase::Dir(path.parent().unwrap_or(Path::new("")).to_path_buf()),
                    module,
                };
                let mut run = Run {
                    checker: self,
                    parsed: HashMap::new(),
                    out: Checked::default(),
                };
                run.ast(&file, source, &ast, &mut found, false);
            }
        }
        found
    }
}

/// A path compared as written with `/`, so one file read twice is one.
fn normal(path: &Path) -> PathBuf {
    // A scene's search may spell it absolute, a project's walk relative
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    PathBuf::from(vfs::normalize(&path))
}

fn syntax_error(path: &Path, e: &rhai::ParseError) -> Diagnostic {
    let d = Diagnostic::error(codes::SCRIPT_SYNTAX, e.0.to_string())
        .file(path)
        .hint("the script does not load until it compiles");
    match (e.1.line(), e.1.position()) {
        (Some(line), Some(col)) => d.at(line, col),
        (Some(line), None) => d.at(line, 1),
        _ => d,
    }
}

/// A module's functions: name to the arities it defines.
type Fns = BTreeMap<String, BTreeSet<usize>>;

struct Run<'c> {
    checker: &'c Checker,
    /// Each module's functions, by file (`None`: it didn't read or compile).
    parsed: HashMap<PathBuf, Option<Fns>>,
    out: Checked,
}

impl Run<'_> {
    /// Read, compile and check `file`; the modules it imports, to check next.
    fn file(&mut self, file: &ScriptFile) -> Vec<ScriptFile> {
        self.out.files.push(file.path.clone());
        let source = match vfs::read_to_string(&file.path) {
            Ok(text) => text,
            Err(e) => {
                self.out.diagnostics.push(
                    Diagnostic::error(codes::FILE_UNREADABLE, e.to_string()).file(&file.path),
                );
                return Vec::new();
            }
        };
        let ast = match self.checker.engine.compile(strip_shebang(&source)) {
            Ok(ast) => ast,
            Err(e) => {
                self.out.diagnostics.push(syntax_error(&file.path, &e));
                return Vec::new();
            }
        };
        let mut found = Vec::new();
        let imports = self.ast(file, &source, &ast, &mut found, true);
        self.out.diagnostics.extend(found);
        imports
    }

    /// The functions of the module at `path`, compiled once.
    fn module_fns(&mut self, path: &Path) -> Option<Fns> {
        let key = normal(path);
        if let Some(fns) = self.parsed.get(&key) {
            return fns.clone();
        }
        let fns = vfs::read_to_string(path).ok().and_then(|text| {
            let ast = self.checker.engine.compile(strip_shebang(&text)).ok()?;
            Some(own_fns(&ast, true))
        });
        self.parsed.insert(key, fns.clone());
        fns
    }

    /// Everything about one compiled file. Returns the modules it imports.
    fn ast(
        &mut self,
        file: &ScriptFile,
        source: &str,
        ast: &AST,
        found: &mut Vec<Diagnostic>,
        follow: bool,
    ) -> Vec<ScriptFile> {
        let path = &file.path;
        let mut seen = Seen::default();
        collect(ast.statements(), None, &mut seen);
        for f in ast.iter_fn_def() {
            for p in f.params.iter() {
                seen.vars.insert(p.to_string());
            }
            collect(f.body.statements(), Some(f.name.as_str()), &mut seen);
        }
        let own = own_fns(ast, false);

        // Imports: each alias to the functions of its module
        let mut modules: BTreeMap<String, Option<Fns>> = BTreeMap::new();
        let mut next = Vec::new();
        for import in &seen.imports {
            let Some(name) = &import.path else {
                continue;
            };
            match file.base.file_path(name) {
                Ok(module_path) => {
                    let fns = self.module_fns(&module_path);
                    if fns.is_none() && !vfs::is_file(&module_path) {
                        found.push(
                            Diagnostic::error(
                                codes::MISSING_MODULE,
                                format!("import \"{name}\": there is no {}", shown(&module_path)),
                            )
                            .file(path)
                            .at(import.line, import.col),
                        );
                    }
                    if !import.alias.is_empty() {
                        modules.insert(import.alias.clone(), fns);
                    }
                    if follow && vfs::is_file(&module_path) {
                        next.push(ScriptFile {
                            path: module_path,
                            base: file.base.clone(),
                            module: true,
                        });
                    }
                }
                Err(not_found) => {
                    found.push(
                        Diagnostic::error(
                            codes::MISSING_MODULE,
                            format!("import \"{name}\": no `lib/{name}.rhai` in the scripts search"),
                        )
                        .file(path)
                        .at(import.line, import.col)
                        .tried(not_found.tried)
                        .hint("a module is `scripts/lib/<name>.rhai` beside the scene or in a folder above it"),
                    );
                    if !import.alias.is_empty() {
                        modules.insert(import.alias.clone(), None);
                    }
                }
            }
        }

        let per_frame = per_frame_fns(&seen);
        for call in &seen.calls {
            let fast = call
                .within
                .as_deref()
                .is_some_and(|f| per_frame.contains(f));
            if let Some(d) = self.judge(call, &own, &modules, &seen, fast) {
                found.extend(d.into_iter().map(|d| d.file(path).at(call.line, call.col)));
            }
        }
        if !file.module {
            found.extend(callbacks(path, source, ast, &seen));
        }
        found.sort_by_key(|d| (d.line, d.column));
        next
    }

    /// What is wrong with one call, if anything.
    fn judge(
        &self,
        call: &CallSite,
        own: &Fns,
        modules: &BTreeMap<String, Option<Fns>>,
        seen: &Seen,
        per_frame: bool,
    ) -> Option<Vec<Diagnostic>> {
        let api = &self.checker.api;
        let name = call.name.as_str();
        let n = call.args.len();
        // `x.f(a)`: `f(x, a)` natively, `f(a)` with `this` in a script
        let native = if call.method { n + 1 } else { n };

        if let Some(alias) = &call.namespace {
            // Only a module this file imports can be judged
            let fns = modules.get(alias)?.as_ref()?;
            return match fns.get(name) {
                Some(arities) if arities.contains(&n) => None,
                Some(arities) => Some(vec![Diagnostic::error(
                    codes::WRONG_ARITY,
                    format!(
                        "`{alias}::{name}` takes {}; this call passes {n}",
                        arg_counts(arities)
                    ),
                )]),
                None => Some(vec![Diagnostic::error(
                    codes::UNKNOWN_FUNCTION,
                    format!("module `{alias}` has no function `{name}`"),
                )
                .hint_opt(diagnostic::did_you_mean(
                    name,
                    fns.keys().map(String::as_str),
                ))]),
            };
        }
        if RHAI_KEYWORDS.contains(&name) || api.allowed.contains(name) {
            return None;
        }
        if let Some(arities) = own.get(name) {
            if arities.contains(&n) || (call.method && arities.contains(&native)) {
                return None;
            }
        }
        let declared = api.declared.get(name);
        if let Some(entries) = declared {
            let at: Vec<&ApiEntry> = entries
                .iter()
                .filter(|e| e.params.len() == native)
                .collect();
            if !at.is_empty() {
                let skip = native - n;
                return self.judge_declared(call, &at, skip, per_frame);
            }
        }
        let other = api.other_arities(name);
        if other.contains(&native) || (call.method && other.contains(&n)) {
            return None;
        }
        // A variable holding a function pointer, called by its name
        if !call.method && seen.vars.contains(name) {
            return None;
        }
        // `obj.f()` on a map holding a closure under `f`
        if call.method && seen.map_keys.contains(name) {
            return None;
        }

        // Known at other arities: say which
        let mut arities: BTreeSet<usize> = other;
        let mut signatures: Vec<String> = Vec::new();
        if let Some(entries) = declared {
            for e in entries {
                arities.insert(e.params.len());
                if !signatures.contains(&e.signature()) {
                    signatures.push(e.signature());
                }
            }
        }
        let script_arities = own.get(name);
        if let Some(own_arities) = script_arities {
            arities.extend(own_arities.iter().copied());
        }
        if !arities.is_empty() {
            let passes = if call.method {
                format!("this call passes {n} (and the value before the `.`)")
            } else {
                format!("this call passes {n}")
            };
            let hint = if !signatures.is_empty() {
                Some(format!("declared: {}", signatures.join("; ")))
            } else if script_arities.is_some() {
                Some(format!("`fn {name}` is defined in this script"))
            } else {
                None
            };
            return Some(vec![Diagnostic::error(
                codes::WRONG_ARITY,
                format!("`{name}` takes {}; {passes}", arg_counts(&arities)),
            )
            .hint_opt(hint)]);
        }

        // Nothing has the name
        let in_module = modules
            .iter()
            .find(|(_, fns)| fns.as_ref().is_some_and(|f| f.contains_key(name)))
            .map(|(alias, _)| format!("`{alias}::{name}` is the imported module's"));
        let near = in_module.or_else(|| {
            diagnostic::did_you_mean(name, api.names().chain(own.keys().map(String::as_str)))
        });
        let what = if call.method { "method" } else { "function" };
        Some(vec![Diagnostic::error(
            codes::UNKNOWN_FUNCTION,
            format!("no script {what} `{name}`"),
        )
        .hint(near.unwrap_or_else(|| {
            "`flint script api --filter <text>` lists the API; a game's own function \
             passes with `--allow <name>` or `--api <listing.json>`"
                .to_string()
        }))])
    }

    /// A call of a declared function at an arity it has: its literals, its
    /// deprecation and decision 5. `skip` is 1 for a method call (the value
    /// before the `.` is the first parameter).
    fn judge_declared(
        &self,
        call: &CallSite,
        at: &[&ApiEntry],
        skip: usize,
        per_frame: bool,
    ) -> Option<Vec<Diagnostic>> {
        let mut out = Vec::new();
        let fits = |e: &ApiEntry| -> Option<(usize, &'static str, &'static str)> {
            call.args.iter().enumerate().find_map(|(i, lit)| {
                let param = e.params[i + skip];
                let lit = (*lit)?;
                (!accepts(param.kind, lit)).then_some((i, param.name, param.kind))
            })
        };
        let entry = match at.iter().find(|e| fits(e).is_none()) {
            Some(e) => *e,
            None => {
                let (i, param, kind) = fits(at[0]).expect("no entry fits");
                let lit = call.args[i].expect("a literal");
                out.push(
                    Diagnostic::error(
                        codes::BAD_ARGUMENT,
                        format!(
                            "`{}` argument {} (`{param}: {kind}`) is {}",
                            call.name,
                            i + 1 + skip,
                            lit.described()
                        ),
                    )
                    .hint(format!("{}: {}", at[0].signature(), wants(kind))),
                );
                return Some(out);
            }
        };
        if at.iter().all(|e| e.deprecated.is_some()) {
            let note = entry.deprecated.unwrap_or_default();
            out.push(
                Diagnostic::warning(
                    codes::DEPRECATED_FUNCTION,
                    format!("`{}` is deprecated", call.name),
                )
                .hint(note),
            );
        }
        // Decision 5: ints and floats mixed in four or more `num` slots
        let nums: Vec<usize> = entry
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.kind == "num")
            .map(|(i, _)| i)
            .collect();
        if per_frame && !entry.typed && nums.len() >= MANY_NUMS {
            let lit_at = |i: usize| i.checked_sub(skip).and_then(|i| call.args[i]);
            let ints = nums
                .iter()
                .filter(|&&i| matches!(lit_at(i), Some(Lit::Int)))
                .count();
            let floats = nums
                .iter()
                .filter(|&&i| matches!(lit_at(i), Some(Lit::Float(_))))
                .count();
            if ints > 0 && floats > 0 {
                out.push(
                    Diagnostic::warning(
                        codes::MIXED_NUMBERS,
                        format!(
                            "`{}` mixes int and float literals in its number arguments \
                             ({ints} int, {floats} float), every frame: the call misses both \
                             exact signatures and takes Rhai's search, ~4 us instead of ~0.4",
                            call.name
                        ),
                    )
                    .hint("write its numbers all as floats (`1.0`) or all as ints"),
                );
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

/// `1 argument`, `8 or 9 arguments`.
fn arg_counts(arities: &BTreeSet<usize>) -> String {
    let list: Vec<String> = arities.iter().map(usize::to_string).collect();
    let plural = arities.len() > 1 || arities.first() != Some(&1);
    let joined = match list.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    };
    format!("{joined} argument{}", if plural { "s" } else { "" })
}

fn shown(path: &Path) -> String {
    flint_core::project::shown(path)
}

fn strip_shebang(source: &str) -> &str {
    if source.starts_with("#!") {
        // Keep the newline, so lines count from the file's first
        source.find('\n').map_or("", |n| &source[n..])
    } else {
        source
    }
}

/// A file's own functions; `public` only for a module seen from outside.
fn own_fns(ast: &AST, public: bool) -> Fns {
    let mut fns = Fns::new();
    for f in ast.iter_functions() {
        if public && f.access == rhai::FnAccess::Private {
            continue;
        }
        fns.entry(f.name.to_string())
            .or_default()
            .insert(f.params.len());
    }
    fns
}

// ------------------------------------------------------------- the walk ---

/// The kind of a literal argument, for the checks that read literals.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Lit {
    Int,
    Float(f64),
    Bool,
    Str,
    Array,
    Map,
}

impl Lit {
    fn of(e: &Expr) -> Option<Lit> {
        Some(match e {
            Expr::IntegerConstant(..) => Lit::Int,
            Expr::FloatConstant(f, _) => Lit::Float(**f),
            Expr::BoolConstant(..) => Lit::Bool,
            Expr::StringConstant(..) | Expr::InterpolatedString(..) => Lit::Str,
            Expr::Array(..) => Lit::Array,
            Expr::Map(..) => Lit::Map,
            Expr::DynamicConstant(d, _) => {
                if d.is_int() {
                    Lit::Int
                } else if d.is_float() {
                    Lit::Float(d.as_float().unwrap_or(0.5))
                } else if d.is_array() {
                    Lit::Array
                } else if d.is_map() {
                    Lit::Map
                } else if d.is_string() {
                    Lit::Str
                } else if d.is_bool() {
                    Lit::Bool
                } else {
                    return None;
                }
            }
            // `-1` and `-0.5`, if the parser left the minus as a call
            Expr::FnCall(x, _) if x.name == "-" && x.args.len() == 1 => {
                match Lit::of(&x.args[0])? {
                    Lit::Int => Lit::Int,
                    Lit::Float(f) => Lit::Float(-f),
                    _ => return None,
                }
            }
            _ => return None,
        })
    }

    fn described(self) -> String {
        match self {
            Lit::Int => "an int".to_string(),
            Lit::Float(f) => format!("the float {f}"),
            Lit::Bool => "a bool".to_string(),
            Lit::Str => "a string".to_string(),
            Lit::Array => "an array".to_string(),
            Lit::Map => "a map".to_string(),
        }
    }
}

/// Whether a parameter of `kind` (as the registry names it) takes `lit`.
/// A kind this doesn't know takes anything.
fn accepts(kind: &str, lit: Lit) -> bool {
    match kind {
        "num" => matches!(lit, Lit::Int | Lit::Float(_)),
        // `num` on the typed path: exactly a float
        "float" => matches!(lit, Lit::Float(_)),
        "int" => match lit {
            Lit::Int => true,
            Lit::Float(f) => f.fract() == 0.0,
            _ => false,
        },
        "bool" => lit == Lit::Bool,
        "entity" | "entity?" => lit == Lit::Int,
        "str" => lit == Lit::Str,
        "map" => lit == Lit::Map,
        "array" | "color array" => lit == Lit::Array,
        "color" => matches!(lit, Lit::Array | Lit::Str),
        _ => true,
    }
}

/// What a kind wants, for the hint.
fn wants(kind: &str) -> &'static str {
    match kind {
        "num" => "a num is an int or a float",
        "float" => "the function is typed: a float, written with a point (`1.0`)",
        "int" => "an int, or a float with nothing after the point",
        "bool" => "true or false",
        "entity" | "entity?" => "an entity id, an int",
        "str" => "a string",
        "map" => "a map, #{ ... }",
        "array" | "color array" => "an array, [ ... ]",
        "color" => "a colour: [r, g, b], [r, g, b, a] or \"#rrggbb\"",
        _ => "see `flint script api`",
    }
}

/// One call in a file.
#[derive(Debug, Clone)]
struct CallSite {
    name: String,
    /// `m` of `m::f()`.
    namespace: Option<String>,
    method: bool,
    /// Each argument's literal kind, or `None` where it isn't a literal.
    args: Vec<Option<Lit>>,
    /// The function it is in; `None` at the top level.
    within: Option<String>,
    line: usize,
    col: usize,
}

#[derive(Debug, Clone)]
struct Import {
    /// The module's path, when it is a string literal.
    path: Option<String>,
    alias: String,
    line: usize,
    col: usize,
}

/// What a walk of a file saw.
#[derive(Debug, Default)]
struct Seen {
    calls: Vec<CallSite>,
    imports: Vec<Import>,
    /// Every variable and parameter name: one may hold a function pointer.
    vars: BTreeSet<String>,
    /// Every key of a map literal: `obj.f()` may call a closure there.
    map_keys: BTreeSet<String>,
    /// Every string literal: a function may be named in one (`Fn("f")`).
    strings: BTreeSet<String>,
    /// For each function, the plain calls in it, for the per-frame set.
    calls_from: BTreeMap<String, BTreeSet<String>>,
}

/// Walk `stmts`, the body of `within` (or the top level).
fn collect(stmts: &[Stmt], within: Option<&str>, seen: &mut Seen) {
    for stmt in stmts {
        stmt.walk(&mut Vec::new(), &mut |nodes: &[ASTNode]| {
            visit(nodes, within, seen)
        });
    }
}

fn visit(nodes: &[ASTNode], within: Option<&str>, seen: &mut Seen) -> bool {
    let Some(node) = nodes.last() else {
        return true;
    };
    match node {
        ASTNode::Stmt(Stmt::FnCall(x, pos)) => call(x, *pos, false, within, seen),
        ASTNode::Expr(Expr::FnCall(x, pos)) => call(x, *pos, false, within, seen),
        ASTNode::Expr(Expr::MethodCall(x, pos)) => {
            call(x, *pos, true, within, seen);
            // Rhai's walk doesn't go into a method call's arguments
            for arg in x.args.iter() {
                arg.walk(&mut Vec::new(), &mut |nodes: &[ASTNode]| {
                    visit(nodes, within, seen)
                });
            }
        }
        ASTNode::Stmt(Stmt::Var(x, ..)) => {
            seen.vars.insert(x.0.name.to_string());
        }
        ASTNode::Stmt(Stmt::For(x, ..)) => {
            seen.vars.insert(x.0.name.to_string());
            if let Some(counter) = &x.1 {
                seen.vars.insert(counter.name.to_string());
            }
        }
        ASTNode::Stmt(Stmt::Import(x, pos)) => {
            let path = match &x.0 {
                Expr::StringConstant(s, _) => Some(s.to_string()),
                _ => None,
            };
            let (line, col) = place(*pos);
            seen.imports.push(Import {
                path,
                alias: x.1.name.to_string(),
                line,
                col,
            });
        }
        ASTNode::Expr(Expr::Map(x, _)) => {
            for (key, _) in x.0.iter() {
                seen.map_keys.insert(key.name.to_string());
            }
        }
        ASTNode::Expr(Expr::StringConstant(s, _)) => {
            seen.strings.insert(s.to_string());
        }
        _ => {}
    }
    true
}

fn place(pos: rhai::Position) -> (usize, usize) {
    (pos.line().unwrap_or(1), pos.position().unwrap_or(1))
}

fn call(x: &FnCallExpr, pos: rhai::Position, method: bool, within: Option<&str>, seen: &mut Seen) {
    let name = x.name.as_str();
    // Operators, and the parser's own names (`anon$...`)
    if x.is_operator_call()
        || name.contains('$')
        || !name.starts_with(|c: char| c.is_alphabetic() || c == '_')
    {
        return;
    }
    let namespace = (!x.namespace.is_empty()).then(|| x.namespace.root().to_string());
    if namespace.is_none() && !method {
        if let Some(f) = within {
            seen.calls_from
                .entry(f.to_string())
                .or_default()
                .insert(name.to_string());
        }
    }
    let (line, col) = place(pos);
    seen.calls.push(CallSite {
        name: name.to_string(),
        namespace,
        method,
        args: x.args.iter().map(Lit::of).collect(),
        within: within.map(str::to_string),
        line,
        col,
    });
}

/// The per-frame callbacks and every function of the file they call.
fn per_frame_fns(seen: &Seen) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = PER_FRAME.iter().map(|s| s.to_string()).collect();
    let mut todo: Vec<String> = out.iter().cloned().collect();
    while let Some(f) = todo.pop() {
        for callee in seen.calls_from.get(&f).into_iter().flatten() {
            if out.insert(callee.clone()) {
                todo.push(callee.clone());
            }
        }
    }
    out
}

// ------------------------------------------------------------ callbacks ---

/// A callback defined at the wrong arity, and an `fn` one edit from a
/// callback that nothing calls: the engine never would.
fn callbacks(path: &Path, source: &str, ast: &AST, seen: &Seen) -> Vec<Diagnostic> {
    let names: Vec<&str> = CALLBACKS.iter().map(|c| c.name).collect();
    let fold = |s: &str| -> String {
        s.chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
            .collect()
    };
    let called: BTreeSet<&str> = seen
        .calls
        .iter()
        .filter(|c| c.namespace.is_none())
        .map(|c| c.name.as_str())
        .collect();
    let mut out = Vec::new();
    for f in ast.iter_functions() {
        let at = |d: Diagnostic| {
            let (line, col) = fn_position(source, f.name);
            d.file(path).at(line, col)
        };
        if let Some(cb) = CALLBACKS.iter().find(|c| c.name == f.name) {
            if f.params.len() != cb.params.len() {
                let wanted = format!("fn {}({})", cb.name, cb.params.join(", "));
                let hint = if cb.name == flint_core::callbacks::ON_UPDATE {
                    format!("`{wanted}`: delta_time() is the frame's step")
                } else {
                    format!("`{wanted}`")
                };
                out.push(at(Diagnostic::error(
                    codes::CALLBACK_ARITY,
                    format!(
                        "`fn {}` has {} parameter{}; the engine calls it with {}, so it never runs",
                        f.name,
                        f.params.len(),
                        if f.params.len() == 1 { "" } else { "s" },
                        cb.params.len()
                    ),
                )
                .hint(hint)));
            }
            continue;
        }
        let looks_like_one =
            f.name.starts_with("on_") || names.iter().any(|n| fold(n) == fold(f.name));
        if !looks_like_one || called.contains(f.name) || seen.strings.contains(f.name) {
            continue;
        }
        if let Some(near) = diagnostic::closest(f.name, names.iter().copied()) {
            out.push(at(Diagnostic::error(
                codes::UNKNOWN_CALLBACK,
                format!(
                    "`fn {}` is not a callback, and nothing in this script calls it",
                    f.name
                ),
            )
            .hint(format!("did you mean `{near}`?"))));
        }
    }
    out
}

/// Where `fn <name>` is written: line and column of `fn`, from 1.
fn fn_position(source: &str, name: &str) -> (usize, usize) {
    for (i, line) in source.lines().enumerate() {
        let mut from = 0;
        while let Some(at) = line[from..].find("fn ") {
            let start = from + at;
            let before_ok = start == 0
                || !line[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let rest = line[start + 3..].trim_start();
            if before_ok
                && rest.starts_with(name)
                && rest[name.len()..].trim_start().starts_with('(')
            {
                return (i + 1, start + 1);
            }
            from = start + 3;
        }
    }
    (1, 1)
}

#[cfg(test)]
mod tests;
