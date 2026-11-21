//! The lit shaders' WGSL, each put together from the files they share (#738).
//!
//! WGSL has no include, so a shader that lights a surface is its own file with
//! the shared ones concatenated in front of it, the way `wooze_noise.wgsl`
//! goes in front of the shallow wooze stages. Each shared file says what it
//! needs; WGSL lets a module use a name before it declares it, so the order
//! below is only for reading.
//!
//! | File | Holds | Taken by |
//! |---|---|---|
//! | `lights.wgsl` | `LightUniforms` and its lights, `ShadowUniforms` | all six |
//! | `light_bindings.wgsl` | bind group 2, bindings 0-3 | all six |
//! | `brdf.wgsl` | GGX, Smith, Schlick, `attenuation`, the spot cone, `evaluate_light` | meshes, terrain, ocean, grass |
//! | `shadows.wgsl` | the sun's cascades (`shadow_factor`), the casting spot's map | meshes, terrain, ocean, grass |
//! | `direct_light.wgsl` | the loops over every light | meshes, terrain |
//! | `local_lights.wgsl` | one point or spot light at a point, for a look of its own | ocean, grass |
//! | `tonemap.wgsl` | `aces_filmic` | meshes, terrain, ocean, the composite |
//! | `pbr_surface.wgsl` | the PBR material and every fragment stage but the entry | meshes |
//!
//! Before #738 each shader carried its own copy of what it used, and the
//! copies drifted: #500 (spots lit backwards on meshes, not on terrain),
//! terrain's cascades without the normal offset, the skinned shader without
//! the clay levers or area sources, the ocean without PCSS.

/// One shader: the shared files named, then its own, as one `&'static str`.
macro_rules! wgsl {
    ($($file:literal),+ $(,)?) => {
        concat!($(include_str!($file), "\n"),+)
    };
}

/// A static mesh: `vs_main` / `fs_main` and the surface's other stages
/// (`fs_veil`, `fs_fog_veil`, `fs_fog_veil_light`, `fs_albedo`).
pub(crate) const SCENE: &str = wgsl!(
    "lights.wgsl",
    "light_bindings.wgsl",
    "brdf.wgsl",
    "shadows.wgsl",
    "direct_light.wgsl",
    "tonemap.wgsl",
    "pbr_surface.wgsl",
    "shader.wgsl",
);

/// A skinned mesh: `vs_skinned`, `fs_skinned`, `fs_skinned_lighten`, over
/// the same surface as [`SCENE`].
pub(crate) const SKINNED: &str = wgsl!(
    "lights.wgsl",
    "light_bindings.wgsl",
    "brdf.wgsl",
    "shadows.wgsl",
    "direct_light.wgsl",
    "tonemap.wgsl",
    "pbr_surface.wgsl",
    "skinned_shader.wgsl",
);

/// Terrain: its splat-map surface through the meshes' lighting.
pub(crate) const TERRAIN: &str = wgsl!(
    "lights.wgsl",
    "light_bindings.wgsl",
    "brdf.wgsl",
    "shadows.wgsl",
    "direct_light.wgsl",
    "tonemap.wgsl",
    "terrain_shader.wgsl",
);

/// The ocean and water bodies: their own stylized shading, the scene's
/// lights (the point and spot lights through `local_lights.wgsl`, #1048) and
/// the sun's cascades.
pub(crate) const OCEAN: &str = wgsl!(
    "lights.wgsl",
    "light_bindings.wgsl",
    "brdf.wgsl",
    "shadows.wgsl",
    "local_lights.wgsl",
    "tonemap.wgsl",
    "ocean_shader.wgsl",
);

/// Grass blades. The comb (#252) is shallow wooze's noise, so grass and a
/// bench standing in it are moved by one field. Lanterns reach the blades
/// through `local_lights.wgsl` (#1048).
pub(crate) const GRASS: &str = wgsl!(
    "wooze_noise.wgsl",
    "lights.wgsl",
    "light_bindings.wgsl",
    "brdf.wgsl",
    "shadows.wgsl",
    "local_lights.wgsl",
    "grass_render.wgsl",
);

/// The god rays' ray march, which reads the lights and the cascades.
pub(crate) const VOLUMETRIC: &str = wgsl!(
    "lights.wgsl",
    "light_bindings.wgsl",
    "volumetric_shader.wgsl",
);

/// Every assembled shader, by the name its errors should carry.
#[cfg(test)]
pub(crate) const ALL: [(&str, &str); 6] = [
    ("shader.wgsl", SCENE),
    ("skinned_shader.wgsl", SKINNED),
    ("terrain_shader.wgsl", TERRAIN),
    ("ocean_shader.wgsl", OCEAN),
    ("grass_render.wgsl", GRASS),
    ("volumetric_shader.wgsl", VOLUMETRIC),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str, source: &str) -> (naga::Module, naga::valid::ModuleInfo) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        (module, info)
    }

    #[test]
    fn every_lit_shader_parses_and_validates() {
        for (name, source) in ALL {
            module(name, source);
        }
    }

    /// Every file in `src/` that is WGSL, by name and text (the stylize
    /// folder's too), read from disk so a new copy anywhere is seen.
    #[allow(clippy::disallowed_methods)] // a test reading the crate's own source tree
    fn wgsl_files() -> Vec<(String, String)> {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "wgsl") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    out.push((path.display().to_string(), text));
                }
            }
        }
        out
    }

    /// #738's done-when: the light layout is declared in one file, and the
    /// lighting functions are written once, so a lighting fix is one edit.
    #[test]
    fn the_lighting_is_written_once() {
        let files = wgsl_files();
        for (decl, home) in [
            ("struct LightUniforms", "lights.wgsl"),
            ("struct ShadowUniforms", "lights.wgsl"),
            ("struct SpotLight", "lights.wgsl"),
            ("fn evaluate_light(", "brdf.wgsl"),
            ("fn spot_cone_factor(", "brdf.wgsl"),
            ("fn distribution_ggx(", "brdf.wgsl"),
            ("fn shadow_factor(", "shadows.wgsl"),
            ("fn spot_shadow_factor(", "shadows.wgsl"),
            ("fn point_shadow_factor(", "shadows.wgsl"),
            ("fn direct_light(", "direct_light.wgsl"),
            ("fn point_light_at(", "local_lights.wgsl"),
            ("fn spot_light_at(", "local_lights.wgsl"),
            ("fn attenuation(", "brdf.wgsl"),
            ("fn aces_filmic(", "tonemap.wgsl"),
        ] {
            let found: Vec<&str> = files
                .iter()
                .filter(|(_, text)| text.lines().any(|l| l.trim_start().starts_with(decl)))
                .map(|(path, _)| path.as_str())
                .collect();
            assert!(
                found.len() == 1 && found[0].ends_with(home),
                "`{decl}` should be declared once, in {home}; found in {found:?}"
            );
        }
    }

    /// The WGSL structs' sizes are the Rust structs' the CPU writes, in every
    /// shader that declares them. With `min_binding_size: None` a mismatch
    /// would only show as a draw-time validation error.
    #[test]
    fn wgsl_layouts_match_rust() {
        let want = [
            (
                "LightUniforms",
                std::mem::size_of::<crate::pipeline::LightUniforms>(),
            ),
            (
                "ShadowUniforms",
                std::mem::size_of::<crate::shadow::ShadowUniforms>(),
            ),
            (
                "SpotLight",
                std::mem::size_of::<crate::pipeline::SpotLight>(),
            ),
            (
                "PointLight",
                std::mem::size_of::<crate::pipeline::PointLight>(),
            ),
            (
                "DirectionalLight",
                std::mem::size_of::<crate::pipeline::DirectionalLight>(),
            ),
        ];
        for (name, source) in ALL {
            let (module, _) = module(name, source);
            for (ty, size) in want {
                let (_, t) = module
                    .types
                    .iter()
                    .find(|(_, t)| t.name.as_deref() == Some(ty))
                    .unwrap_or_else(|| panic!("{name}: no {ty}"));
                assert_eq!(
                    t.inner.size(module.to_ctx()) as usize,
                    size,
                    "{name}: {ty}'s WGSL size against the Rust struct"
                );
            }
        }
    }

    /// The meshes' two modules draw one surface: every fragment stage the
    /// static mesh has, the skinned mesh has too, but the entry that takes
    /// `front_facing` (`fs_main` against `fs_skinned`), so a veil, fog veil
    /// or albedo pass finds its entry in either.
    #[test]
    fn both_mesh_modules_share_their_fragment_stages() {
        let entries = |source: &str| -> Vec<String> {
            let (module, _) = module("mesh", source);
            module
                .entry_points
                .iter()
                .filter(|e| e.stage == naga::ShaderStage::Fragment)
                .map(|e| e.name.clone())
                .filter(|n| n != "fs_main" && n != "fs_skinned" && n != "fs_skinned_lighten")
                .collect()
        };
        let plain = entries(SCENE);
        assert_eq!(plain, entries(SKINNED));
        for name in ["fs_veil", "fs_fog_veil", "fs_fog_veil_light", "fs_albedo"] {
            assert!(plain.iter().any(|n| n == name), "{name} missing");
        }
    }
}
