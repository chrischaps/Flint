//! `import "name"` resolution through the file layer.
//!
//! rhai's `FileModuleResolver` reads modules straight from the disk (and does
//! not exist on wasm32). [`VfsModuleResolver`] resolves paths the same way but
//! reads through [`flint_core::vfs`], so `import` works from a memory source
//! too, on every target.

use flint_core::project::{AssetKind, NotFound, Project};
use flint_core::vfs;
use rhai::{Engine, EvalAltResult, Module, ModuleResolver, Position, Scope, Shared, AST};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Where `import "name"` looks for `name.rhai`.
#[derive(Debug, Clone)]
pub enum ModuleBase {
    /// One directory: `<dir>/name.rhai`.
    Dir(PathBuf),
    /// The project's one asset search (ADR 0114): the script
    /// `lib/name.rhai`, so `scripts/lib/` beside the scene, in each
    /// directory above it up to the project root, then in the engine.
    Project {
        project: Project,
        scene_dir: PathBuf,
    },
}

impl ModuleBase {
    /// The file an import names, or everywhere it was looked for. A
    /// relative path is searched for, an absolute one is kept, and the
    /// extension is forced to `.rhai` (as rhai's `FileModuleResolver` does).
    pub fn file_path(&self, path: &str) -> Result<PathBuf, NotFound> {
        let named = Path::new(path).with_extension("rhai");
        if !named.is_relative() {
            return Ok(named);
        }
        match self {
            ModuleBase::Dir(dir) => Ok(dir.join(named)),
            ModuleBase::Project { project, scene_dir } => {
                let name = format!("lib/{}", named.to_string_lossy().replace('\\', "/"));
                project.resolve(AssetKind::Scripts, &name, scene_dir)
            }
        }
    }
}

/// Loads `<name>.rhai` modules from a [`ModuleBase`] and caches each by its
/// resolved path. The cache lives as long as the resolver: install a fresh
/// one to clear it.
pub struct VfsModuleResolver {
    base: ModuleBase,
    cache: RwLock<HashMap<PathBuf, Shared<Module>>>,
    /// Modules already read and compiled elsewhere (hot reload's worker,
    /// #633), by resolved path: used in place of reading the file, so an
    /// import after a lib change doesn't wait on the disk in a frame. An
    /// error is the compile's, reported where the module is imported.
    compiled: HashMap<PathBuf, Result<AST, String>>,
}

impl VfsModuleResolver {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self::over(ModuleBase::Dir(base_path.into()))
    }

    pub fn over(base: ModuleBase) -> Self {
        Self {
            base,
            cache: RwLock::new(HashMap::new()),
            compiled: HashMap::new(),
        }
    }

    /// Resolve these paths from their compiled ASTs instead of the files.
    pub fn with_compiled(mut self, compiled: HashMap<PathBuf, Result<AST, String>>) -> Self {
        self.compiled = compiled;
        self
    }

    /// The file an import names ([`ModuleBase::file_path`]).
    pub fn file_path(&self, path: &str) -> Result<PathBuf, NotFound> {
        self.base.file_path(path)
    }
}

impl ModuleResolver for VfsModuleResolver {
    fn resolve(
        &self,
        engine: &Engine,
        _source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> Result<Shared<Module>, Box<EvalAltResult>> {
        let file_path = self.file_path(path).map_err(|not_found| {
            // Every place the module was looked for
            tracing::warn!("import \"{path}\": {not_found}");
            Box::new(EvalAltResult::ErrorModuleNotFound(path.to_string(), pos))
        })?;

        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        if let Some(module) = cache.get(&file_path) {
            return Ok(module.clone());
        }
        drop(cache);

        let mut ast = match self.compiled.get(&file_path) {
            Some(Ok(ast)) => ast.clone(),
            Some(Err(e)) => {
                return Err(Box::new(EvalAltResult::ErrorInModule(
                    path.to_string(),
                    e.as_str().into(),
                    pos,
                )))
            }
            None => compile_module(engine, &file_path, path, pos)?,
        };
        ast.set_source(path);

        let module: Shared<Module> = Module::eval_ast_as_new(Scope::new(), &ast, engine)
            .map_err(|err| Box::new(EvalAltResult::ErrorInModule(path.to_string(), err, pos)))?
            .into();

        self.cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(file_path, Shared::clone(&module));
        Ok(module)
    }
}

/// Read and compile the module `path` names (at `file_path`), as rhai's file
/// loader does.
pub fn compile_module(
    engine: &Engine,
    file_path: &Path,
    path: &str,
    pos: Position,
) -> Result<AST, Box<EvalAltResult>> {
    let source = vfs::read_to_string(file_path)
        .map_err(|_| Box::new(EvalAltResult::ErrorModuleNotFound(path.to_string(), pos)))?;
    compile_module_source(engine, source, path, pos)
}

/// Compile a module's text as rhai's file loader does: a leading shebang
/// line is skipped. The hot-reload worker compiles lib modules with this,
/// off the frame thread (#633).
pub fn compile_module_source(
    engine: &Engine,
    mut source: String,
    path: &str,
    pos: Position,
) -> Result<AST, Box<EvalAltResult>> {
    if source.starts_with("#!") {
        match source.find('\n') {
            Some(n) => drop(source.drain(..n)),
            None => source.clear(),
        }
    }
    engine.compile(&source).map_err(|err| {
        Box::new(EvalAltResult::ErrorInModule(
            path.to_string(),
            err.into(),
            pos,
        ))
    })
}
