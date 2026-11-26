//! What `flint check` and `flint validate` hold living's scene tables to
//! (#686, #246, #1558): `[wooze]` against the field and the preset it
//! names, `[living.species]` against what the props would take and the
//! specs they grow from. The kernel's check asks through
//! `flint_package::cli::TableChecks`; without this package it holds the two
//! tables to their schemas alone, as a player without it reads neither.

use flint_core::diagnostic::codes;
use flint_core::Diagnostic;
use flint_package::cli::{TableCheck, TableChecks};
use flint_scene::SceneFile;
use std::path::Path;

/// The point living fills.
pub fn checks() -> TableChecks {
    TableChecks {
        refused: flint_procgen::LivingTables::problems,
        check,
    }
}

/// `[wooze]`'s problems in the field's words, which stand in for the scene
/// pass's errors there, then `[living.species]`'s.
fn check(at: &TableCheck) -> Vec<(Option<&'static str>, Vec<Diagnostic>)> {
    vec![
        (Some("wooze"), wooze(at.scene, at.file, at.registry)),
        (None, species(at.scene, at.file, at.world)),
    ]
}

/// What's wrong with the scene's `[wooze]` table and the preset it names.
/// The zone, when it is about one, is the diagnostic's field.
fn wooze(
    scene: &Path,
    file: &SceneFile,
    registry: &flint_schema::SchemaRegistry,
) -> Vec<Diagnostic> {
    let mut file = file.clone();
    let preset_issues = flint_procgen::apply_preset_file(&mut file, scene);
    let Some(table) = flint_procgen::LivingTables::of(&file)
        .ok()
        .and_then(|tables| tables.wooze)
    else {
        return Vec::new();
    };
    preset_issues
        .into_iter()
        .chain(table.check(registry))
        .map(|issue| {
            let d = Diagnostic::error(codes::BAD_WOOZE, issue.to_string())
                .file(scene)
                .component("wooze");
            match &issue.zone {
                Some(zone) => d.field(zone.clone()),
                None => d,
            }
        })
        .collect()
}

/// What's wrong with the scene's `[living.species.<spec>]` tables (#246): a
/// value the player would leave out, and a species no living prop in the
/// scene grows from, which is most likely a misspelt spec name and would
/// change nothing. The species is the diagnostic's field.
fn species(
    scene: &Path,
    file: &SceneFile,
    world: Option<&flint_ecs::FlintWorld>,
) -> Vec<Diagnostic> {
    let mut file = file.clone();
    // A preset the scene names can hold species too, laid under its own.
    flint_procgen::apply_preset_file(&mut file, scene);
    let living = flint_procgen::LivingTables::of(&file)
        .ok()
        .and_then(|tables| tables.living);
    let Some(living) = living.filter(|living| !living.species.is_empty()) else {
        return Vec::new();
    };
    let grown: Option<Vec<String>> = world.map(|world| {
        let mut grown: Vec<String> = world
            .all_entities()
            .iter()
            .filter_map(|entity| {
                let comps = world.get_components(entity.id)?;
                comps.get(crate::components::LIVING_PROP)?;
                let asset = comps.get(flint_core::components::MODEL)?.get("asset")?;
                Some(asset.as_str()?.to_string())
            })
            .collect();
        grown.sort();
        grown.dedup();
        grown
    });

    let problem = |name: &str, message: String| {
        Diagnostic::error(codes::BAD_SPECIES, message)
            .file(scene)
            .component("living")
            .field(format!("species.{name}"))
    };
    let mut names: Vec<&String> = living.species.keys().collect();
    names.sort_unstable();
    let mut found = Vec::new();
    for name in names {
        for message in crate::player::props::species_problems(&living.species[name]) {
            found.push(problem(name, message));
        }
        // Which specs grow is the loaded world's to say
        let Some(grown) = &grown else {
            continue;
        };
        if !grown.contains(name) {
            let species = if grown.is_empty() {
                "it has no living props".to_string()
            } else {
                format!("its living props grow from {}", grown.join(", "))
            };
            found.push(problem(
                name,
                format!(
                    "no living prop in this scene grows from '{name}', \
                     so it changes nothing; {species}"
                ),
            ));
        }
    }
    found
}
