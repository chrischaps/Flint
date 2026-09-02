use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchData {
    pub generated_at: String,
    pub crates: Vec<CrateInfo>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CrateInfo {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub lines: usize,
    pub external_deps: Vec<String>,
    pub internal_deps: Vec<String>,
    pub tier: u32,
    pub modules: Vec<ModuleInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub name: String,
    pub path: String,
    pub lines: usize,
    pub public_items: Vec<PublicItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ModuleInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublicItem {
    pub kind: ItemKind,
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Struct,
    Enum,
    Trait,
    Fn,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
}
