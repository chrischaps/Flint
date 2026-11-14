//! What the renderer loads (ADR 0118, #736): models and their textures,
//! procedural meshes, textures by file or bytes, `ui_text` fonts, ray
//! targets, and the asset scope they are found through.

use super::*;

/// Point a model material's texture references at the names
/// [`SceneRenderer::upload_model_textures`] gave them: `<asset>::<texture>`,
/// and for a data map whose texture is also a colour map, the linear copy.
pub(super) fn namespace_textures(
    material: &mut flint_import::ImportedMaterial,
    asset: &str,
    both: &HashSet<String>,
) {
    let colour = |tex: &mut Option<String>| {
        if let Some(t) = tex {
            *t = format!("{asset}::{t}");
        }
    };
    let data = |tex: &mut Option<String>| {
        if let Some(t) = tex {
            let suffix = if both.contains(t.as_str()) {
                "#linear"
            } else {
                ""
            };
            *t = format!("{asset}::{t}{suffix}");
        }
    };
    colour(&mut material.base_color_texture);
    colour(&mut material.emissive_texture);
    data(&mut material.normal_texture);
    data(&mut material.metallic_roughness_texture);
}

impl SceneRenderer {
    /// Get an immutable reference to the texture cache.
    pub fn texture_cache(&self) -> Option<&TextureCache> {
        self.texture_cache.as_ref()
    }

    /// Clear all model/mesh data so a new model can be loaded cleanly.
    /// Used by the preview command when drag-and-dropping a replacement model.
    pub fn clear_model_data(&mut self) {
        self.mesh_cache.clear();
        self.draw_bind_cache.clear();
        self.skinning.bone_binds.clear();
        self.box_mesh_cache.clear();
        if let Some(tc) = &mut self.texture_cache {
            tc.clear_user_textures();
        }
        self.entity_draws.clear();
        self.skinned_entity_draws.clear();
        self.transparent_draws.clear();
        self.transparent_skinned_draws.clear();
        self.echo_draws.clear();
        self.culled_entity_draws.clear();
        self.culled_transparent_draws.clear();
        self.billboard_draws.clear();
        self.sprite2d_batches.clear();
        self.wireframe_overlay_draws.clear();
        self.normal_arrow_draws.clear();
        self.skeleton_overlay_draws.clear();
        self.debug_overlay_draws.clear();
        self.particle_draws.clear();
        self.particle_texture_bind_groups.clear();
        self.bitmap_font_cache.clear();
        self.bitmap_font_scope = None;
    }

    /// Load an imported model into the GPU mesh cache
    pub fn load_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        import_result: &ImportResult,
    ) {
        let default_color = self
            .archetype_visuals
            .get("furniture")
            .map(|v| v.color)
            .unwrap_or([0.5, 0.5, 0.5, 1.0]);

        self.mesh_cache
            .upload_imported(device, name, import_result, default_color);

        // Upload textures referenced by materials, namespaced by asset name
        let both = self.upload_model_textures(device, queue, name, import_result);

        // Patch material texture references on cached meshes to use namespaced names
        if let Some(gpu_meshes) = self.mesh_cache.get_mut(name) {
            for mesh in gpu_meshes.iter_mut() {
                namespace_textures(&mut mesh.material, name, &both);
            }
        }
    }

    /// Load skinned meshes from an imported model into the GPU mesh cache
    pub fn load_skinned_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        import_result: &ImportResult,
    ) {
        let default_color = self
            .archetype_visuals
            .get("character")
            .map(|v| v.color)
            .unwrap_or([0.5, 0.5, 0.5, 1.0]);

        if let Some(sp) = &self.pipelines.skinned {
            self.mesh_cache.upload_skinned(
                device,
                name,
                import_result,
                default_color,
                &sp.bone_bind_group_layout,
            );
        }

        // Also upload textures, namespaced by asset name
        let both = self.upload_model_textures(device, queue, name, import_result);

        // Patch material texture references on cached skinned meshes
        if let Some(gpu_meshes) = self.mesh_cache.get_skinned_mut(name) {
            for mesh in gpu_meshes.iter_mut() {
                namespace_textures(&mut mesh.material, name, &both);
            }
        }
    }

    /// Upload a model's textures as `<asset>::<texture>`, each in the colour
    /// space its materials read it in: normal and metallic-roughness maps
    /// linear, base-colour and emissive maps (and unreferenced textures)
    /// sRGB. A texture used both ways is uploaded twice, the linear copy as
    /// `<asset>::<texture>#linear`; returns the names of those, for
    /// [`namespace_textures`].
    pub(super) fn upload_model_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        import_result: &ImportResult,
    ) -> HashSet<String> {
        let mut colour = HashSet::new();
        let mut data = HashSet::new();
        for m in &import_result.materials {
            colour.extend(m.base_color_texture.iter().chain(&m.emissive_texture));
            data.extend(m.normal_texture.iter().chain(&m.metallic_roughness_texture));
        }
        let mut both = HashSet::new();
        if let Some(cache) = &mut self.texture_cache {
            for texture in &import_result.textures {
                let namespaced = format!("{}::{}", name, texture.name);
                let is_data = data.contains(&texture.name);
                if !is_data || colour.contains(&texture.name) {
                    cache.upload(device, queue, &namespaced, texture, false);
                }
                if is_data && colour.contains(&texture.name) {
                    cache.upload(
                        device,
                        queue,
                        &format!("{namespaced}#linear"),
                        texture,
                        true,
                    );
                    both.insert(texture.name.clone());
                } else if is_data {
                    cache.upload(device, queue, &namespaced, texture, true);
                }
            }
        }
        both
    }

    /// Tell the renderer the scene's project and folder: where its styles
    /// find palettes, glyph sets, papers and strokes (ADR 0114). Sets
    /// `scene_dir` too. A host that only sets `scene_dir` gets the project
    /// found from that folder, once.
    pub fn set_asset_scope(&mut self, scope: flint_core::project::AssetScope) {
        self.scene_dir = Some(scope.scene_dir.clone());
        *self.asset_scope.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((self.scene_dir.clone(), Arc::new(scope)));
    }

    /// The project and folder the styles' files are searched from: the
    /// scope the host set, else the project found from `scene_dir`, else
    /// (no scene) the working directory's.
    pub fn asset_scope(&self) -> Arc<flint_core::project::AssetScope> {
        let mut kept = self.asset_scope.lock().unwrap_or_else(|e| e.into_inner());
        match &*kept {
            Some((dir, scope)) if *dir == self.scene_dir => scope.clone(),
            _ => {
                let dir = self.scene_dir.as_deref().unwrap_or(Path::new("."));
                let scope = Arc::new(flint_core::project::AssetScope::for_dir(dir));
                *kept = Some((self.scene_dir.clone(), scope.clone()));
                scope
            }
        }
    }

    /// Load the font a `ui_text.font` names (`builtin`, a glyph set, or a
    /// `.font.toml`; ADR 0113) and upload its atlas to the texture cache,
    /// once per name and scope. Returns whether the font is ready to draw.
    ///
    /// A name is only a name within one scope: after a transition into
    /// another folder, `pixel` is that folder's `fonts/pixel.glyphs.toml`.
    /// So a load from a scope other than the last one's forgets every font
    /// loaded before it, their atlases too (#312).
    pub fn load_ui_font(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spec: &str,
        scope: &flint_core::project::AssetScope,
    ) -> bool {
        if self.bitmap_font_scope.as_ref() != Some(scope) {
            self.forget_ui_fonts();
            self.bitmap_font_scope = Some(scope.clone());
        }
        if let Some(found) = self.bitmap_font_cache.get(spec) {
            return found.is_some();
        }
        let font = match BitmapFont::resolve(spec, scope) {
            Ok(font) => match &mut self.texture_cache {
                Some(cache) => {
                    let (rgba, [w, h]) = font.rgba();
                    match cache.upload_rgba(device, queue, &font.texture, w, h, &rgba, false) {
                        Ok(_) => Some(font),
                        Err(e) => {
                            tracing::warn!("ui_text font `{spec}`: {e}");
                            None
                        }
                    }
                }
                None => None,
            },
            Err(e) => {
                tracing::warn!("ui_text font `{spec}`: {e}");
                None
            }
        };
        let ready = font.is_some();
        self.bitmap_font_cache.insert(spec.to_string(), font);
        ready
    }

    /// Drop every loaded `ui_text` font and its uploaded atlas, so the next
    /// [`load_ui_font`](Self::load_ui_font) reads and uploads afresh (the
    /// texture cache keeps a name's first upload otherwise).
    pub(super) fn forget_ui_fonts(&mut self) {
        for font in self.bitmap_font_cache.drain().filter_map(|(_, f)| f) {
            if let Some(cache) = &mut self.texture_cache {
                cache.remove_texture(&font.texture);
            }
        }
        self.bitmap_font_scope = None;
    }

    /// The `ui_text` font `spec` names, if it loaded and its atlas is
    /// uploaded: what the scene's `ui_text` draws with.
    pub fn loaded_ui_font(&self, spec: &str) -> Option<&BitmapFont> {
        self.ui_font(self.texture_cache.as_ref()?, spec)
    }

    /// A loaded `ui_text` font whose atlas is in `tex_cache`.
    pub(super) fn ui_font(&self, tex_cache: &TextureCache, spec: &str) -> Option<&BitmapFont> {
        self.bitmap_font_cache
            .get(spec)?
            .as_ref()
            .filter(|f| tex_cache.contains(&f.texture))
    }

    /// Load a texture from an image file into the texture cache
    pub fn load_texture_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: &Path,
    ) -> Result<bool, String> {
        if let Some(cache) = &mut self.texture_cache {
            cache.load_file(device, queue, name, path)
        } else {
            Err("Texture cache not initialized".to_string())
        }
    }

    /// Load a texture from an image file with a full mip chain (see
    /// [`TextureCache::load_file_mipmapped`](crate::TextureCache::load_file_mipmapped)).
    pub fn load_texture_file_mipmapped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: &Path,
    ) -> Result<bool, String> {
        if let Some(cache) = &mut self.texture_cache {
            cache.load_file_mipmapped(device, queue, name, path)
        } else {
            Err("Texture cache not initialized".to_string())
        }
    }

    /// Load raw RGBA8 pixel data into the texture cache.
    ///
    /// Returns `Ok(true)` if newly uploaded, `Ok(false)` if already cached.
    pub fn load_texture_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        data: &[u8],
        is_normal_map: bool,
    ) -> Result<bool, String> {
        if let Some(cache) = &mut self.texture_cache {
            cache.upload_rgba(device, queue, name, width, height, data, is_normal_map)
        } else {
            Err("Texture cache not initialized".to_string())
        }
    }

    /// Create or replace the named mesh from raw vertex/index data.
    ///
    /// Any entity whose `model.asset` is `name` draws it from the next
    /// `update_from_world`. Replacing a mesh frees the old GPU buffers once
    /// the current frame's draw lists let go of them, so a mesh can be
    /// rebuilt as often as needed without growing GPU memory. Vertex colours
    /// are used when `material.use_vertex_color` is set; they are linear,
    /// so convert sRGB colours with [`crate::srgb_to_linear`].
    pub fn load_procedural_mesh(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        vertices: &[crate::primitives::Vertex],
        indices: &[u32],
        material: flint_import::ImportedMaterial,
    ) {
        self.ray_meshes.remove(name);
        self.mesh_cache
            .upload_procedural(device, name, vertices, indices, material);
    }

    /// [`load_procedural_mesh`](Self::load_procedural_mesh) for a mesh of
    /// several parts, each with its own material, all drawn under `name`.
    pub fn load_procedural_mesh_parts(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        parts: Vec<(
            Vec<crate::primitives::Vertex>,
            Vec<u32>,
            flint_import::ImportedMaterial,
        )>,
    ) {
        self.ray_meshes.remove(name);
        self.mesh_cache.upload_procedural_parts(device, name, parts);
    }

    /// Upload raw RGBA8 pixels as the texture `name`, replacing one already
    /// there. Cached material binds name their textures, so they are dropped
    /// and remade on the next `update_from_world` to pick up the new one:
    /// fine for a swap (a placeholder giving way), costly every frame.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_texture_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        data: &[u8],
        is_normal_map: bool,
    ) -> Result<(), String> {
        self.remove_texture(name);
        self.load_texture_rgba(device, queue, name, width, height, data, is_normal_map)
            .map(|_| ())
    }

    /// Remove the texture `name`, if loaded, and drop the cached material
    /// binds that may hold it. Returns whether it was there.
    pub fn remove_texture(&mut self, name: &str) -> bool {
        let Some(cache) = &mut self.texture_cache else {
            return false;
        };
        if !cache.contains(name) {
            return false;
        }
        cache.remove_texture(name);
        self.draw_bind_cache.clear();
        true
    }

    /// Make the mesh `name` a ray target (ADR 0105): from the next
    /// `update_from_world`, every entity drawing it places `mesh` in
    /// [`ray_scene`](Self::ray_scene) at its world matrix. Build `mesh` from
    /// the positions and indices the mesh was uploaded with. `None` stops it
    /// being one. Uploading the mesh again, or removing it, also stops it,
    /// so set it again after each upload.
    pub fn set_ray_mesh(
        &mut self,
        name: &str,
        mesh: Option<Arc<flint_geometry::ray_mesh::RayMesh>>,
    ) {
        match mesh {
            Some(m) => {
                self.ray_meshes.insert(name.to_string(), m);
            }
            None => {
                self.ray_meshes.remove(name);
            }
        }
    }

    /// The ray targets as the last `update_from_world` found them: each
    /// entity drawing a mesh made one with [`set_ray_mesh`](Self::set_ray_mesh),
    /// at its world matrix (ADR 0105). Cast rays through it to find what's
    /// drawn along a line. A new scene is made each update, so a clone held
    /// across frames goes on answering for the frame it came from.
    pub fn ray_scene(&self) -> Arc<flint_geometry::ray_mesh::RayScene> {
        self.ray_scene.clone()
    }

    /// Remove the named mesh. Returns whether it existed. Entities still
    /// naming it in `model.asset` draw nothing (or their `bounds` box, if
    /// they have one) until a mesh with that name is uploaded again (ADR 0074).
    pub fn remove_mesh(&mut self, name: &str) -> bool {
        self.ray_meshes.remove(name);
        self.mesh_cache.remove(name)
    }

    /// Get a mutable reference to the mesh cache
    pub fn mesh_cache_mut(&mut self) -> &mut MeshCache {
        &mut self.mesh_cache
    }

    /// Get a reference to the mesh cache
    pub fn mesh_cache(&self) -> &MeshCache {
        &self.mesh_cache
    }
}
