use crate::model;
use std::path::Path;

pub struct CrateMeta {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub external_deps: Vec<String>,
    pub internal_deps: Vec<String>,
}

pub fn find_crate_dirs(_workspace_root: &Path) -> Vec<std::path::PathBuf> {
    vec![]
}

pub fn parse_crate(_workspace_root: &Path, _crate_dir: &Path) -> (CrateMeta, Vec<model::Edge>) {
    todo!()
}
