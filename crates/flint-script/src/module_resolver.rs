//! `import "name"` resolution through the file layer.
//!
//! rhai's `FileModuleResolver` reads modules straight from the disk (and does
//! not exist on wasm32). [`VfsModuleResolver`] resolves paths the same way but
//! reads through [`flint_core::vfs`], so `import` works from a memory source
//! too, on every target.

use flint_core::vfs;
use rhai::{Engine, EvalAltResult, Module, ModuleResolver, Position, Scope, Shared, AST};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Loads `<base>/<path>.rhai` modules and caches each by its resolved path.
/// The cache lives as long as the resolver: install a fresh one to clear it.
pub struct VfsModuleResolver {
    base_path: PathBuf,
    cache: RwLock<HashMap<PathBuf, Shared<Module>>>,
    /// Modules already read and compiled elsewhere (hot reload's worker,
    /// #633), by resolved path: used in place of reading the file, so an
    /// import after a lib change doesn't wait on the disk in a frame. An
    /// error is the compile's, reported where the module is imported.
    compiled: HashMap<PathBuf, Result<AST, String>>,
}

impl VfsModuleResolver {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
            cache: RwLock::new(HashMap::new()),
            compiled: HashMap::new(),
        }
    }

    /// Resolve these paths from their compiled ASTs instead of the files.
    pub fn with_compiled(mut self, compiled: HashMap<PathBuf, Result<AST, String>>) -> Self {
        self.compiled = compiled;
        self
    }

    /// The file an import names: relative paths join the base directory,
    /// absolute ones are kept, and the extension is forced to `.rhai`
    /// (`FileModuleResolver::get_file_path` with a base path set).
    pub fn file_path(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        let mut file_path = if path.is_relative() {
            self.base_path.join(path)
        } else {
            path.to_path_buf()
        };
        file_path.set_extension("rhai");
        file_path
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
        let file_path = self.file_path(path);

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
