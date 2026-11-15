mod book;
mod cargo_parser;
mod metrics;
mod model;
mod source_parser;

use model::ArchData;
use std::path::PathBuf;

const USAGE: &str = "\
flint-arch-analyzer: the workspace's crates, their dependencies and public items

  flint-arch-analyzer [out.json]   write the explorer's data (default
                                   tools/arch-viewer/arch-data.json; open
                                   tools/arch-viewer/index.html over a static server)
  flint-arch-analyzer --book       rewrite the crate table in
                                   docs/book/src/architecture/crate-graph.md
  flint-arch-analyzer --check-book exit 1 if that table is stale (CI)";

fn main() {
    let workspace_root = find_workspace_root();
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return;
        }
        Some(flag @ ("--book" | "--check-book")) => {
            let check = flag == "--check-book";
            match book::run(&workspace_root, check) {
                Ok(true) if check => println!("{} is up to date", book::PAGE),
                Ok(true) => println!("wrote the crate table in {}", book::PAGE),
                Ok(false) => {
                    eprintln!(
                        "{} is stale: a Cargo.toml changed its engine dependencies.\n\
                         Run `cargo run -p flint-arch-analyzer -- --book` and commit the page.",
                        book::PAGE
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("flint-arch-analyzer: {e}");
                    std::process::exit(2);
                }
            }
            return;
        }
        Some(a) if a.starts_with('-') => {
            eprintln!("unknown flag {a}\n\n{USAGE}");
            std::process::exit(2);
        }
        _ => {}
    }
    let output_path = arg
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
