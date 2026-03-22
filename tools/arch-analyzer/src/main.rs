mod cargo_parser;
mod metrics;
mod model;
mod source_parser;

use model::ArchData;
use std::path::PathBuf;

fn main() {
    let workspace_root = find_workspace_root();
    let output_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("tools/arch-viewer/arch-data.json"));

    let data = analyze(&workspace_root);

    let json = serde_json::to_string_pretty(&data).expect("Failed to serialize");
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&output_path, &json).expect("Failed to write output");
    println!(
        "Wrote {} crates, {} edges to {}",
        data.crates.len(),
        data.edges.len(),
        output_path.display()
    );
}

fn find_workspace_root() -> PathBuf {
    let mut dir = std::env::current_dir().expect("No current dir");
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml).unwrap();
            if content.contains("[workspace]") {
                return dir;
            }
        }
        if !dir.pop() {
            panic!("Could not find workspace root (no Cargo.toml with [workspace] found)");
        }
    }
}

fn analyze(workspace_root: &std::path::Path) -> ArchData {
    let crate_dirs = cargo_parser::find_crate_dirs(workspace_root);
    let mut crates = Vec::new();
    let mut edges = Vec::new();

    for crate_dir in &crate_dirs {
        let (info, crate_edges) = cargo_parser::parse_crate(workspace_root, crate_dir);
        edges.extend(crate_edges);
        let modules = source_parser::parse_crate_sources(crate_dir);
        let lines = metrics::total_lines(&modules);
        crates.push(model::CrateInfo {
            name: info.name,
            path: info.path,
            description: info.description,
            lines,
            external_deps: info.external_deps,
            internal_deps: info.internal_deps,
            tier: 0,
            modules,
        });
    }

    metrics::compute_tiers(&mut crates);

    ArchData {
        generated_at: chrono::Utc::now().to_rfc3339(),
        crates,
        edges,
    }
}
