//! The book's crate table (#764): `docs/book/src/architecture/crate-graph.md`
//! holds a block between two markers that this module writes from the
//! workspace's Cargo.toml files, so the table can't drift from them.
//!
//!     cargo run -p flint-arch-analyzer -- --book        # rewrite the block
//!     cargo run -p flint-arch-analyzer -- --check-book  # exit 1 if it's stale (CI)

use crate::cargo_parser::{self, CrateMeta, InternalDep};
use std::collections::BTreeMap;
use std::path::Path;

pub const PAGE: &str = "docs/book/src/architecture/crate-graph.md";
pub const BEGIN: &str = "<!-- crate-table:begin -->";
pub const END: &str = "<!-- crate-table:end -->";

/// `flint-debug-ui` -> `debug-ui`; the table's columns name crates short.
fn short(name: &str) -> &str {
    name.strip_prefix("flint-").unwrap_or(name)
}

fn note(dep: &InternalDep) -> String {
    let mut how = Vec::new();
    if dep.optional {
        how.push("optional".to_string());
    }
    if let Some(t) = &dep.target {
        how.push(format!("{t} only"));
    }
    if how.is_empty() {
        String::new()
    } else {
        format!(" ({})", how.join(", "))
    }
}

/// Longest path down to a crate with no engine dependencies: 0 for core.
fn tiers(crates: &[CrateMeta]) -> BTreeMap<String, u32> {
    fn tier(
        name: &str,
        by_name: &BTreeMap<&str, &CrateMeta>,
        memo: &mut BTreeMap<String, u32>,
    ) -> u32 {
        if let Some(t) = memo.get(name) {
            return *t;
        }
        let t = by_name
            .get(name)
            .map(|c| {
                c.internal_deps
                    .iter()
                    .map(|d| tier(d, by_name, memo) + 1)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        memo.insert(name.to_string(), t);
        t
    }
    let by_name: BTreeMap<&str, &CrateMeta> = crates.iter().map(|c| (c.name.as_str(), c)).collect();
    let mut memo = BTreeMap::new();
    for c in crates {
        tier(&c.name, &by_name, &mut memo);
    }
    memo
}

/// The block's text, markers included.
pub fn render(workspace_root: &Path) -> String {
    let crates: Vec<CrateMeta> = cargo_parser::find_crate_dirs(workspace_root)
        .iter()
        .map(|d| cargo_parser::parse_crate(workspace_root, d).0)
        .collect();
    let tiers = tiers(&crates);

    // Who depends on each crate, with the same notes.
    let mut users: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for c in &crates {
        for d in &c.internal_dep_notes {
            users.entry(d.name.as_str()).or_default().push(format!(
                "{}{}",
                short(&c.name),
                note(d)
            ));
        }
    }

    let mut rows: Vec<&CrateMeta> = crates.iter().collect();
    rows.sort_by_key(|c| (tiers[&c.name], c.name.clone()));

    let engine = crates
        .iter()
        .filter(|c| c.path.starts_with("crates"))
        .count();
    let tools: Vec<String> = crates
        .iter()
        .filter(|c| !c.path.starts_with("crates"))
        .map(|c| format!("`{}`", c.path.replace('\\', "/")))
        .collect();

    let mut out = String::new();
    out.push_str(BEGIN);
    out.push('\n');
    out.push_str(
        "<!-- Written by tools/arch-analyzer from each Cargo.toml; don't edit by hand.\n     \
         Rewrite: cargo run -p flint-arch-analyzer -- --book. CI's crate-table job runs --check-book. -->\n\n",
    );
    out.push_str(&format!(
        "The workspace has {engine} crates under `crates/`, plus {} ({}). \
         Rows run bottom-up: a crate's **tier** is the longest chain of engine crates beneath it, \
         so every crate depends only on crates of a lower tier. Optional and target-only \
         dependencies are marked; dev-dependencies are left out.\n\n",
        if tools.len() == 1 {
            "one tool".to_string()
        } else {
            format!("{} tools", tools.len())
        },
        tools.join(", "),
    ));
    out.push_str("| Tier | Crate | Depends On | Depended On By |\n");
    out.push_str("|-----:|-------|-----------|----------------|\n");
    for c in rows {
        let mut deps: Vec<String> = c
            .internal_dep_notes
            .iter()
            .map(|d| format!("{}{}", short(&d.name), note(d)))
            .collect();
        deps.dedup();
        let deps = if deps.is_empty() {
            "*(none)*".to_string()
        } else {
            deps.join(", ")
        };
        let by = match users.get(c.name.as_str()) {
            Some(u) => {
                let mut u = u.clone();
                u.sort();
                u.dedup();
                u.join(", ")
            }
            None => "*(none)*".to_string(),
        };
        let name = if c.path.starts_with("crates") {
            format!("`{}`", c.name)
        } else {
            format!("`{}` (`{}`)", c.name, c.path.replace('\\', "/"))
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            tiers[&c.name], name, deps, by
        ));
    }
    out.push('\n');
    out.push_str(END);
    out
}

/// `page` with its block replaced by `block`; None when the markers are missing.
pub fn splice(page: &str, block: &str) -> Option<String> {
    let start = page.find(BEGIN)?;
    let end = page[start..].find(END)? + start + END.len();
    Some(format!("{}{}{}", &page[..start], block, &page[end..]))
}

/// Write the block (`check` false) or compare it (`check` true). Ok(false) when
/// `check` found it stale.
pub fn run(workspace_root: &Path, check: bool) -> Result<bool, String> {
    let path = workspace_root.join(PAGE);
    let page = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // A Windows checkout may have CRLF; compare and write in LF.
    let page = page.replace("\r\n", "\n");
    let block = render(workspace_root);
    let new =
        splice(&page, &block).ok_or_else(|| format!("{PAGE} has no {BEGIN} ... {END} block"))?;
    if check {
        return Ok(new == page);
    }
    if new != page {
        std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_replaces_only_the_block() {
        let page = format!("a\n{BEGIN}\nold\n{END}\nb\n");
        let out = splice(&page, &format!("{BEGIN}\nnew\n{END}")).unwrap();
        assert_eq!(out, format!("a\n{BEGIN}\nnew\n{END}\nb\n"));
        assert!(splice("no markers", "x").is_none());
    }

    #[test]
    fn note_marks_optional_and_target() {
        let d = InternalDep {
            name: "flint-debug-ui".into(),
            optional: true,
            target: Some("not wasm32".into()),
        };
        assert_eq!(note(&d), " (optional, not wasm32 only)");
    }

    #[test]
    fn core_is_tier_zero_and_cli_is_on_top() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let block = render(&root);
        assert!(block.contains("| 0 | `flint-core` | *(none)* |"));
        let cli = block
            .lines()
            .find(|l| l.contains("| `flint-cli` |"))
            .unwrap();
        let top: u32 = cli.split('|').nth(1).unwrap().trim().parse().unwrap();
        for line in block
            .lines()
            .filter(|l| l.starts_with("| ") && !l.contains("Tier"))
        {
            let t: u32 = line.split('|').nth(1).unwrap().trim().parse().unwrap();
            assert!(t <= top, "{line}");
        }
    }
}
