//! What `flint check` and `flint validate` hold a terrain to beyond its
//! schema (#1697): the recipe its `spec` names, read as the binder reads it.
//! A spec that doesn't parse is a `bad-terrain-spec` error, and a key in it
//! nothing reads a warning of the same code, on the spec's file. A key the
//! component writes with another value than its recipe's is an
//! `overrides-spec` warning: the component's wins at load, so the scene
//! still plays, but the two copies have drifted, which is what naming the
//! recipe was meant to end. So is a `heightmap` or `splat_map` beside a
//! `spec`, which draws in place of the bake.
//!
//! A spec the search can't find is the asset pass's `missing-asset`
//! (`terrain.spec` is one of its fields), so it isn't said again here.
//! Registered on the kernel's `SceneChecks` point ([`super::extensions`]).

use crate::source::{spec_path, SpecSource};
use crate::terrain::SPEC_KEYS;
use crate::TerrainConfig;
use flint_core::components as comp;
use flint_core::diagnostic::codes;
use flint_core::project::{AssetKind, Project};
use flint_core::toml_util::reads_same;
use flint_core::Diagnostic;
use flint_ecs::ComponentView;
use flint_package::cli::TableCheck;
use std::path::Path;

/// Every terrain of the scene that names a recipe, against it.
pub fn check(at: &TableCheck) -> Vec<Diagnostic> {
    let Some(world) = at.world else {
        return Vec::new();
    };
    let scene_dir = at.scene.parent().unwrap_or(Path::new(""));
    let mut out = Vec::new();
    for entity in world.all_entities() {
        let view = world.view(entity.id, comp::TERRAIN);
        let Some(Ok(path)) = spec_path(&view, at.project, scene_dir) else {
            continue;
        };
        let on_terrain = |d: Diagnostic, field: &str| {
            d.file(at.scene)
                .entity(entity.name.clone())
                .component(comp::TERRAIN)
                .field(field.to_string())
        };
        let source = match SpecSource::read(&path) {
            Ok(source) => source,
            Err(e) => {
                out.push(
                    on_terrain(Diagnostic::error(codes::BAD_TERRAIN_SPEC, e), "spec")
                        .hint("the terrain isn't drawn until its recipe parses"),
                );
                continue;
            }
        };
        for issue in &source.issues {
            out.push(
                Diagnostic::warning(codes::BAD_TERRAIN_SPEC, issue.message.clone())
                    .file(&path)
                    .field(issue.path.clone())
                    .hint(format!(
                        "the recipe of entity '{}' in {}",
                        entity.name,
                        flint_core::project::shown(at.scene)
                    )),
            );
        }
        let spec = TerrainConfig::from_spec(&source, scene_dir);
        // The recipe as the scene names it, not the path the search made
        let shown = view.str("spec", "").to_string();
        for (key, place) in SPEC_KEYS {
            let Some(written) = view.raw(key) else {
                continue;
            };
            let Some(theirs) = spec_value(&spec, key) else {
                continue;
            };
            if agrees(key, written, &theirs, at.project, scene_dir) {
                continue;
            }
            out.push(
                on_terrain(
                    Diagnostic::warning(
                        codes::OVERRIDES_SPEC,
                        format!(
                            "terrain.{key} = {} overrides {shown}'s {place} = {theirs}",
                            written
                        ),
                    ),
                    key,
                )
                .hint("the component's key wins; leave it out to take the recipe's, or change the recipe"),
            );
        }
        out.extend(
            grass_overrides(&view, &source, &shown)
                .into_iter()
                .map(|(key, d)| on_terrain(d, &key)),
        );
        for (key, what) in [("heightmap", "height layers"), ("splat_map", "splat rules")] {
            if view.opt_str(key).is_some_and(|s| !s.is_empty()) {
                out.push(
                    on_terrain(
                        Diagnostic::warning(
                            codes::OVERRIDES_SPEC,
                            format!(
                                "terrain.{key} stands in for the bake of {shown}: its {what} aren't drawn"
                            ),
                        ),
                        key,
                    )
                    .hint(format!("leave `{key}` out to draw what the recipe bakes")),
                );
            }
        }
    }
    out
}

/// The recipe's value of one of [`SPEC_KEYS`], as TOML. A texture layer
/// the recipe names none for is no disagreement: the component adds one.
fn spec_value(spec: &TerrainConfig, key: &str) -> Option<toml::Value> {
    let float = |v: f32| Some(toml::Value::Float(v as f64));
    match key {
        "width" => float(spec.width),
        "depth" => float(spec.depth),
        "height_scale" => float(spec.height_scale),
        "chunk_resolution" => Some(toml::Value::Integer(spec.chunk_resolution as i64)),
        "texture_tile" => float(spec.texture_tile),
        "metallic" => float(spec.metallic),
        "roughness" => float(spec.roughness),
        _ => {
            let layer = key.strip_prefix("layer")?.strip_suffix("_texture")?;
            let path = &spec.layer_textures[layer.parse::<usize>().ok()?];
            (!path.is_empty()).then(|| toml::Value::String(path.clone()))
        }
    }
}

/// Whether the component's value says what the recipe's does: numbers as
/// a file reads them back, texture paths by the file each finds.
fn agrees(
    key: &str,
    written: &toml::Value,
    theirs: &toml::Value,
    project: &Project,
    scene_dir: &Path,
) -> bool {
    if !key.ends_with("_texture") {
        return reads_same(written, theirs);
    }
    let (Some(a), Some(b)) = (written.as_str(), theirs.as_str()) else {
        return reads_same(written, theirs);
    };
    let found = |name: &str| project.resolve(AssetKind::Files, name, scene_dir).ok();
    match (found(a), found(b)) {
        (Some(a), Some(b)) => flint_core::project::same_path(&a, &b),
        _ => a == b,
    }
}

/// Each `grass.<key>` the component writes with another value than the
/// recipe's `[grass]` (when it has one: grass that lives only in the scene
/// is no second copy).
fn grass_overrides(
    view: &ComponentView<'_>,
    source: &SpecSource,
    shown: &str,
) -> Vec<(String, Diagnostic)> {
    let Some(grass) = &source.spec.grass else {
        return Vec::new();
    };
    let Ok(toml::Value::Table(theirs)) = toml::Value::try_from(grass) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, value) in &theirs {
        let field = format!("grass.{key}");
        let Some(written) = view.raw(&field) else {
            continue;
        };
        if reads_same(written, value) {
            continue;
        }
        out.push((
            field.clone(),
            Diagnostic::warning(
                codes::OVERRIDES_SPEC,
                format!("terrain.{field} = {written} overrides {shown}'s [grass] {key} = {value}"),
            )
            .hint(
                "the component's key wins; leave it out to take the recipe's, or change the recipe",
            ),
        ));
    }
    out
}
