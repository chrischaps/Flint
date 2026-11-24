//! The shaders' WGSL, each put together from the files they share (#738,
//! #1244).
//!
//! WGSL has no include, so a shader is its own file with the shared ones
//! concatenated in front of it, the way `wooze_noise.wgsl` goes in front of
//! the shallow wooze stages. Each shared file says what it needs; WGSL lets a
//! module use a name before it declares it, so the order below is only for
//! reading.
//!
//! | File | Holds | Taken by |
//! |---|---|---|
//! | `lights.wgsl` | `LightUniforms` and its lights, `ShadowUniforms` | the six lit |
//! | `light_bindings.wgsl` | bind group 2, bindings 0-3 | the six lit |
//! | `brdf.wgsl` | GGX, Smith, Schlick, `attenuation`, the spot cone, `evaluate_light` | meshes, terrain, ocean, grass |
//! | `shadows.wgsl` | the sun's cascades (`shadow_factor`), the casting spot's map | meshes, terrain, ocean, grass |
//! | `direct_light.wgsl` | the loops over every light | meshes, terrain |
//! | `local_lights.wgsl` | one point or spot light at a point, for a look of its own | ocean, grass |
//! | `tonemap.wgsl` | `aces_filmic` | meshes, terrain, ocean, the composite |
//! | `pbr_surface.wgsl` | the PBR material and every fragment stage but the entry | meshes |
//! | `mesh_vertex.wgsl` | `TransformUniforms` at group 0, `world_of` / `clip_of` / `world_normal_of`, a woozy draw's `drifted` | meshes, terrain, ocean, grass, wireframe, the woozy stages, both masks |
//! | `skinning.wgsl` | `skin_transform` (each shader binds its own `bone_matrices`) | skinned meshes, shadows, wireframe, the woozy stages, both masks |
//! | `fog_veil.wgsl` | the fog veil's texel (#513) | meshes, particles |
//!
//! Before #738 each shader carried its own copy of what it used, and the
//! copies drifted: #500 (spots lit backwards on meshes, not on terrain),
//! terrain's cascades without the normal offset, the skinned shader without
//! the clay levers or area sources, the ocean without PCSS. Before #1244 the
//! vertex side was copied too: `TransformUniforms` in seven files, and the
//! clip products the depth-matched passes must repeat exactly (the style and
//! selection masks test each fragment against the scene's depth) written
//! out again in four, where a drift shows as holes in a mask.
//!
//! A woozy module (#215, #737) is its plain sources with every line's
//! `//FLINT_WOOZE ` prefix cut ([`woozy`]): mesh_vertex.wgsl's drift field
//! and `drifted` come on, so the woozy stages read the same transform and
//! the same products. The plain modules keep those lines as comments, so a
//! scene with no woozy draw compiles what it did.

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
    "mesh_vertex.wgsl",
    "fog_veil.wgsl",
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
    "mesh_vertex.wgsl",
    "skinning.wgsl",
    "fog_veil.wgsl",
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
    "mesh_vertex.wgsl",
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
    "mesh_vertex.wgsl",
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
    "mesh_vertex.wgsl",
    "grass_render.wgsl",
);

/// The skinned wireframe (#602): its edge lines on the scene's pose.
pub(crate) const WIRE: &str = wgsl!(
    "mesh_vertex.wgsl",
    "skinning.wgsl",
    "skinned_wire_shader.wgsl"
);

/// The shadow pass's two vertex stages, static and skinned.
pub(crate) const SHADOW: &str = wgsl!("skinning.wgsl", "shadow_shader.wgsl");

/// Particles, whose fog veil is the meshes' texel.
pub(crate) const PARTICLE: &str = wgsl!("fog_veil.wgsl", "particle_shader.wgsl");

/// The shadow pass's woozy stages (#215), the plain ones' with the drift:
/// a caster moves as it does in the main pass.
pub(crate) const WOOZE_SHADOW: &str = wgsl!(
    "wooze_noise.wgsl",
    "skinning.wgsl",
    "wooze_shadow_shader.wgsl"
);

/// The prefix a woozy module's lines lose (#737): written in front of a
/// line that only a woozy draw's module has (mesh_vertex.wgsl's drift field
/// and `drifted`, the selection mask's drift).
pub(crate) const WOOZE_SWITCH: &str = "//FLINT_WOOZE ";

/// `source` with its woozy lines on.
pub(crate) fn woozy(source: &str) -> String {
    source.replace(WOOZE_SWITCH, "")
}

/// The main pass's woozy vertex stages (#215), `vs_wooze` and
/// `vs_skinned_wooze`, over [`SCENE`]'s and [`SKINNED`]'s fragment stages.
pub(crate) fn wooze_vertex() -> String {
    woozy(wgsl!(
        "wooze_noise.wgsl",
        "mesh_vertex.wgsl",
        "skinning.wgsl",
        "wooze_vertex.wgsl"
    ))
}

/// A mesh's vertex side for a mask that redraws its triangles (#1244): the
/// transform and its products, and the pose for its skinned stage. The
/// style mask wraps it in a mesh block (`style_mask::STYLE_MASK_WGSL`).
pub(crate) const MASK_VERTEX: &str = wgsl!("mesh_vertex.wgsl", "skinning.wgsl");

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

    /// Every module that takes `mesh_vertex.wgsl`, by the name its errors
    /// should carry: the lit ones but the god rays, then the rest.
    fn mesh_modules() -> Vec<(&'static str, String)> {
        let mut out: Vec<(&'static str, String)> = ALL
            .iter()
            .filter(|(name, _)| *name != "volumetric_shader.wgsl")
            .map(|&(name, source)| (name, source.to_string()))
            .collect();
        out.extend([
            ("skinned_wire_shader.wgsl", WIRE.to_string()),
            ("wooze_vertex.wgsl", wooze_vertex()),
            (
                "style_mask.wgsl",
                crate::style_mask::STYLE_MASK_WGSL.to_string(),
            ),
            (
                "style_mask.wgsl (woozy)",
                crate::style_mask::woozy_mask_source(),
            ),
            (
                "selection_mask.wgsl",
                crate::selection_outline::mask_source(false),
            ),
            (
                "selection_mask.wgsl (woozy)",
                crate::selection_outline::mask_source(true),
            ),
        ]);
        out
    }

    /// The modules #1244 put together that light nothing validate too.
    #[test]
    fn every_vertex_module_parses_and_validates() {
        for (name, source) in mesh_modules() {
            module(name, &source);
        }
        for (name, source) in [
            ("shadow_shader.wgsl", SHADOW),
            ("particle_shader.wgsl", PARTICLE),
            ("wooze_shadow_shader.wgsl", WOOZE_SHADOW),
        ] {
            module(name, source);
        }
    }

    /// #1244's done-when: the transform is declared in one file, and every
    /// pass that draws a mesh's triangles reaches the world and the screen
    /// through the same products and poses a skinned vertex through the same
    /// sum, so a depth-matched pass can't drift from the scene's.
    #[test]
    fn the_vertex_side_is_written_once() {
        let files = wgsl_files();
        for (decl, home) in [
            ("struct TransformUniforms", "mesh_vertex.wgsl"),
            ("struct WoozyTransform", "mesh_vertex.wgsl"),
            ("fn world_of(", "mesh_vertex.wgsl"),
            ("fn clip_of(", "mesh_vertex.wgsl"),
            ("fn world_normal_of(", "mesh_vertex.wgsl"),
            ("fn skin_transform(", "skinning.wgsl"),
            ("fn fog_veil_texel(", "fog_veil.wgsl"),
        ] {
            let found: Vec<&str> = files
                .iter()
                .filter(|(_, text)| text.lines().any(|l| l.trim_start().starts_with(decl)))
                .map(|(path, _)| path.as_str())
                .collect();
            // WoozyTransform is gone (#1244): it may be declared nowhere
            let ok = if decl == "struct WoozyTransform" {
                found.is_empty()
            } else {
                found.len() == 1 && found[0].ends_with(home)
            };
            assert!(
                ok,
                "`{decl}` should be declared once, in {home}; found in {found:?}"
            );
        }
        // The products themselves: what reads the mesh transform's matrices,
        // and what weighs the bones
        for (product, home) in [
            ("transform.model", "mesh_vertex.wgsl"),
            ("transform.view_proj", "mesh_vertex.wgsl"),
            ("transform.model_inv_transpose", "mesh_vertex.wgsl"),
            ("bone_matrices[", "skinning.wgsl"),
        ] {
            for (path, text) in &files {
                if path.ends_with(home) {
                    continue;
                }
                for (n, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    assert!(
                        !code.contains(product),
                        "{path}:{}: `{product}` outside {home}: {line}",
                        n + 1
                    );
                }
            }
        }
    }

    /// A piece that no longer compiles alone is only ever taken through this
    /// module: an `include_str!` of one elsewhere builds a module that fails
    /// on the GPU at its first draw (#1244's A/B found the style veil's and
    /// the fog veil's particle modules still taking `particle_shader.wgsl`
    /// bare), which no naga test here would see.
    #[test]
    #[allow(clippy::disallowed_methods)] // a test reading the crate's own source tree
    fn the_pieces_are_only_taken_assembled() {
        const PIECES: [&str; 20] = [
            "lights.wgsl",
            "light_bindings.wgsl",
            "brdf.wgsl",
            "shadows.wgsl",
            "direct_light.wgsl",
            "local_lights.wgsl",
            "pbr_surface.wgsl",
            "mesh_vertex.wgsl",
            "skinning.wgsl",
            "fog_veil.wgsl",
            "shader.wgsl",
            "skinned_shader.wgsl",
            "terrain_shader.wgsl",
            "ocean_shader.wgsl",
            "grass_render.wgsl",
            "volumetric_shader.wgsl",
            "skinned_wire_shader.wgsl",
            "shadow_shader.wgsl",
            "particle_shader.wgsl",
            "wooze_vertex.wgsl",
        ];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut dirs = vec![src];
        let mut checked = 0;
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("wgsl_sources.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (n, line) in text.lines().enumerate() {
                    let Some(at) = line.find("include_str!(\"") else {
                        continue;
                    };
                    let rest = &line[at + "include_str!(\"".len()..];
                    let file = rest.split('"').next().unwrap_or("");
                    let name = file.rsplit('/').next().unwrap_or(file);
                    checked += 1;
                    // The style mask wraps the vertex side in its mesh block
                    let wrapped = path.ends_with("style_mask.rs")
                        && (name == "mesh_vertex.wgsl" || name == "skinning.wgsl");
                    assert!(
                        wrapped || !PIECES.contains(&name),
                        "{}:{}: `{name}` taken bare; use wgsl_sources",
                        path.display(),
                        n + 1
                    );
                }
            }
        }
        assert!(checked > 20, "the walk found the crate's include_str!s");
    }

    /// The transform the WGSL reads is the Rust slot's, in every module,
    /// plain or woozy, and a woozy one reads the whole 256-byte arena slot
    /// the drift rides in.
    #[test]
    fn the_transform_is_the_rust_structs_in_every_module() {
        let plain = std::mem::size_of::<crate::pipeline::TransformUniforms>();
        let drift = std::mem::size_of::<crate::wooze_pipeline::ShallowWoozeUniforms>();
        for (name, source) in mesh_modules() {
            let (module, _) = module(name, &source);
            let (_, t) = module
                .types
                .iter()
                .find(|(_, t)| t.name.as_deref() == Some("TransformUniforms"))
                .unwrap_or_else(|| panic!("{name}: no TransformUniforms"));
            // A plain module keeps mesh_vertex.wgsl's switched lines; a woozy
            // one has none left
            let woozy = !source.contains(WOOZE_SWITCH);
            assert_eq!(woozy, name.contains("wooze") || name.contains("woozy"));
            let want = if woozy { plain + drift } else { plain };
            assert_eq!(
                t.inner.size(module.to_ctx()) as usize,
                want,
                "{name}: TransformUniforms' WGSL size against the Rust struct"
            );
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
