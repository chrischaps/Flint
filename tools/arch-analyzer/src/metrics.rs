use crate::model::{CrateInfo, ModuleInfo};
use std::collections::HashMap;

pub fn total_lines(modules: &[ModuleInfo]) -> usize {
    modules
        .iter()
        .map(|m| m.lines + total_lines(&m.children))
        .sum()
}

pub fn compute_tiers(crates: &mut [CrateInfo]) {
    let name_to_idx: HashMap<&str, usize> = crates
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_str(), i))
        .collect();

    let mut tiers = vec![0u32; crates.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..crates.len() {
            let max_dep_tier = crates[i]
                .internal_deps
                .iter()
                .filter_map(|dep| name_to_idx.get(dep.as_str()))
                .map(|&idx| tiers[idx])
                .max();
            if let Some(max_t) = max_dep_tier {
                let new_tier = max_t + 1;
                if new_tier != tiers[i] {
                    tiers[i] = new_tier;
                    changed = true;
                }
            }
        }
    }

    for (i, krate) in crates.iter_mut().enumerate() {
        krate.tier = tiers[i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn make_crate(name: &str, deps: Vec<&str>) -> CrateInfo {
        CrateInfo {
            name: name.to_string(),
            path: format!("crates/{}", name),
            description: None,
            lines: 0,
            external_deps: vec![],
            internal_deps: deps.into_iter().map(String::from).collect(),
            tier: 0,
            modules: vec![],
        }
    }

    #[test]
    fn test_total_lines() {
        let modules = vec![ModuleInfo {
            name: "lib".into(),
            path: "src/lib.rs".into(),
            lines: 50,
            public_items: vec![],
            children: vec![ModuleInfo {
                name: "child".into(),
                path: "src/child.rs".into(),
                lines: 30,
                public_items: vec![],
                children: vec![],
            }],
        }];
        assert_eq!(total_lines(&modules), 80);
    }

    #[test]
    fn test_compute_tiers_leaf_is_zero() {
        let mut crates = vec![make_crate("flint-core", vec![])];
        compute_tiers(&mut crates);
        assert_eq!(crates[0].tier, 0);
    }

    #[test]
    fn test_compute_tiers_chain() {
        let mut crates = vec![
            make_crate("flint-core", vec![]),
            make_crate("flint-schema", vec!["flint-core"]),
            make_crate("flint-ecs", vec!["flint-core", "flint-schema"]),
        ];
        compute_tiers(&mut crates);
        let tier_of = |name: &str| crates.iter().find(|c| c.name == name).unwrap().tier;
        assert_eq!(tier_of("flint-core"), 0);
        assert_eq!(tier_of("flint-schema"), 1);
        assert_eq!(tier_of("flint-ecs"), 2);
    }

    #[test]
    fn test_compute_tiers_diamond() {
        let mut crates = vec![
            make_crate("d", vec![]),
            make_crate("b", vec!["d"]),
            make_crate("c", vec!["d"]),
            make_crate("a", vec!["b", "c"]),
        ];
        compute_tiers(&mut crates);
        let tier_of = |name: &str| crates.iter().find(|c| c.name == name).unwrap().tier;
        assert_eq!(tier_of("d"), 0);
        assert_eq!(tier_of("b"), 1);
        assert_eq!(tier_of("c"), 1);
        assert_eq!(tier_of("a"), 2);
    }
}
