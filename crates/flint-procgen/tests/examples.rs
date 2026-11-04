//! The example specs in `specs/examples/` and the crate README are
//! documentation, and documentation drifts. These tests hold both to the code:
//! every example parses, uses only keys its generator reads (what
//! `flint gen --strict` demands), generates valid output deterministically,
//! and the README's parameter catalog equals what the schemas say today.

use std::path::{Path, PathBuf};

use flint_procgen::{
    lint_params, register_built_in_generators, GeneratorOutput, GeneratorRegistry, OutputKind,
    ProcGenSpec,
};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn registry() -> GeneratorRegistry {
    let mut registry = GeneratorRegistry::new();
    register_built_in_generators(&mut registry);
    registry
}

fn example_specs() -> Vec<PathBuf> {
    let dir = crate_dir().join("../../specs/examples");
    let mut specs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".procgen.toml"))
        .collect();
    specs.sort();
    specs
}

fn name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn the_four_examples_the_docs_name_exist() {
    let names: Vec<String> = example_specs().iter().map(|p| name(p)).collect();
    for expected in [
        "oak_tree.procgen.toml",
        "pine_tree.procgen.toml",
        "stone_wall.procgen.toml",
        "wood_plank.procgen.toml",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing {expected}: {names:?}"
        );
    }
}

#[test]
fn every_example_is_strict_clean_and_generates_deterministically() {
    let registry = registry();
    let mut meta_names = Vec::new();
    for path in example_specs() {
        let file = name(&path);
        let spec = ProcGenSpec::from_file(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
        let generator = registry
            .get(&spec.generator)
            .unwrap_or_else(|| panic!("{file}: unknown generator {}", spec.generator));

        // What `flint gen --strict` refuses.
        let issues = lint_params(&generator.param_schema(), &spec.params);
        assert!(issues.is_empty(), "{file}: {issues:?}");

        // A commented example: it opens by saying what it makes.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("# "),
            "{file} should open with a header comment"
        );

        // Textures at 128 px: every op and key still runs, at a sixteenth
        // of the pixels (a 512 px pipeline takes ~17 s in a debug build).
        let mut spec = spec;
        if let Some(t) = spec.params.as_table_mut() {
            for side in ["width", "height"] {
                if t.contains_key(side) {
                    t.insert(side.into(), toml::Value::Integer(128));
                }
            }
        }
        let output = registry
            .generate_from_spec(&spec)
            .unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(output.kind(), generator.output_kind(), "{file}");
        match &output {
            GeneratorOutput::Mesh(mesh) => mesh.validate().unwrap(),
            GeneratorOutput::MeshWithLods(lods) => {
                assert_eq!(lods.len(), spec.lod.as_ref().map_or(1, Vec::len), "{file}");
                for (level, lod) in lods.iter().enumerate() {
                    lod.validate()
                        .unwrap_or_else(|e| panic!("{file} LOD {level}: {e}"));
                    let cap = spec.lod.as_ref().unwrap()[level].target_triangles as usize;
                    assert!(
                        lod.triangle_count() <= cap,
                        "{file} LOD {level} over its cap"
                    );
                }
            }
            GeneratorOutput::ImageSet(maps) => {
                assert_eq!(maps.len(), 3, "{file}: color, normal, roughness");
                for map in maps {
                    map.validate().unwrap();
                }
            }
            other => panic!("{file}: unexpected output {:?}", other.kind()),
        }
        assert_eq!(
            registry.generate_from_spec(&spec).unwrap(),
            output,
            "{file} is not deterministic"
        );
        if generator.output_kind() == OutputKind::Mesh {
            assert!(
                spec.lod.is_some(),
                "{file}: the tree examples show LOD chains"
            );
        }
        meta_names.push(spec.meta.name.clone());
    }
    // The resolver indexes specs by meta.name: examples must not collide.
    let mut unique = meta_names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        meta_names.len(),
        "duplicate meta.name: {meta_names:?}"
    );
}

#[test]
fn readme_catalog_matches_the_schemas() {
    let readme = std::fs::read_to_string(crate_dir().join("README.md"))
        .unwrap()
        .replace("\r\n", "\n");
    let begin = "<!-- catalog:begin -->\n";
    let end = "<!-- catalog:end -->";
    let a = readme
        .find(begin)
        .expect("README has no catalog:begin marker")
        + begin.len();
    let b = readme.find(end).expect("README has no catalog:end marker");
    let in_readme = readme[a..b].trim_end();
    let generated = registry().reference_markdown();
    let generated = generated.trim_end();
    if in_readme != generated {
        let first_diff = in_readme
            .lines()
            .zip(generated.lines())
            .find(|(x, y)| x != y)
            .map(|(x, y)| format!("README:  {x}\nschemas: {y}"))
            .unwrap_or_else(|| "(one is a prefix of the other)".into());
        panic!(
            "The README catalog is out of date. Regenerate it with\n  \
             cargo run -p flint-procgen --example param_catalog\nand paste it between \
             the catalog markers. First difference:\n{first_diff}"
        );
    }
}
