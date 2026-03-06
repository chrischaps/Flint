//! Entity extraction helpers for `update_from_world()`.
//!
//! Each method examines one entity type and pushes draw calls into the
//! appropriate draw-list on `SceneRenderer`.

use super::helpers::{extract_bounds_info, mat4_inv_transpose, parse_blend_mode};
use super::{SceneRenderer, SkinnedDrawCall};
use crate::billboard_pipeline::{BillboardDrawCall, BillboardUniforms, SpriteInstance};
use crate::bitmap_font::{anchor_origin, apply_fill, BitmapFont};
use crate::pipeline::{BlendMode, MaterialUniforms, TransformUniforms};
use crate::primitives::{
    create_box_mesh, create_wireframe_box_mesh, generate_normal_arrows,
    triangles_to_wireframe_indices, Mesh,
};
use crate::sprite2d_pipeline::Sprite2dInstanceGpu;
use crate::texture_cache::TextureCache;
use flint_core::components as comp;
use flint_core::toml_util::{toml_color as extract_color, toml_f32, toml_vec4};
use flint_ecs::DynamicComponents;
use flint_ecs::FlintWorld;
use wgpu::util::DeviceExt;

impl SceneRenderer {
    /// Extract a skinned (skeletal) entity into draw calls.
    /// Returns `true` if skinned meshes were found (caller should `continue`).
    pub(super) fn extract_skinned_entity(
        &mut self,
        device: &wgpu::Device,
        tex_cache: &TextureCache,
        world: &FlintWorld,
        entity_id: flint_core::EntityId,
        asset_name: &str,
        model_matrix: [[f32; 4]; 4],
    ) -> bool {
        let skinned_meshes = match self.mesh_cache.get_skinned(asset_name) {
            Some(meshes) => meshes,
            None => return false,
        };

        let inv_transpose = mat4_inv_transpose(&model_matrix);

        for gpu_mesh in skinned_meshes {
            let transform_uniforms = TransformUniforms {
                view_proj: [[0.0; 4]; 4],
                model: model_matrix,
                model_inv_transpose: inv_transpose,
                camera_pos: [0.0; 3],
                _pad: 0.0,
            };

            let (bc_view, bc_sampler, has_bc) = Self::resolve_texture(
                tex_cache,
                gpu_mesh.material.base_color_texture.as_deref(),
                &tex_cache.default_white,
            );
            let (nm_view, nm_sampler, has_nm) = Self::resolve_texture(
                tex_cache,
                gpu_mesh.material.normal_texture.as_deref(),
                &tex_cache.default_normal,
            );
            let (mr_view, mr_sampler, has_mr) = Self::resolve_texture(
                tex_cache,
                gpu_mesh.material.metallic_roughness_texture.as_deref(),
                &tex_cache.default_metallic_roughness,
            );

            let mut material_uniforms = MaterialUniforms::from_pbr(
                gpu_mesh.material.base_color,
                gpu_mesh.material.metallic,
                gpu_mesh.material.roughness,
            );
            material_uniforms.has_base_color_tex = if has_bc { 1 } else { 0 };
            material_uniforms.has_normal_map = if has_nm { 1 } else { 0 };
            material_uniforms.has_metallic_roughness_tex = if has_mr { 1 } else { 0 };
            if gpu_mesh.material.use_vertex_color {
                material_uniforms.use_vertex_color = 1;
            }

            let (transform_buffer, transform_bind_group) =
                Self::create_transform_bind(device, &self.pipeline, &transform_uniforms);
            let (material_buffer, material_bind_group) = Self::create_material_bind_with_textures(
                device,
                &self.pipeline,
                &material_uniforms,
                bc_view,
                bc_sampler,
                nm_view,
                nm_sampler,
                mr_view,
                mr_sampler,
            );

            let bone_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self
                    .skinned_pipeline
                    .as_ref()
                    .unwrap()
                    .bone_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: gpu_mesh.bone_buffer.as_entire_binding(),
                }],
                label: Some("Skinned Draw Bone Bind Group"),
            });

            // Check transparency from glTF material and ECS material component
            let gltf_alpha = gpu_mesh.material.base_color[3];
            let ecs_opacity = world
                .get_components(entity_id)
                .and_then(|c| c.get(comp::MATERIAL))
                .and_then(|m| m.get("opacity"))
                .and_then(toml_f32)
                .unwrap_or(1.0);
            let ecs_blend_mode_str = world
                .get_components(entity_id)
                .and_then(|c| c.get(comp::MATERIAL))
                .and_then(|m| m.get("blend_mode"))
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            let blend_mode = parse_blend_mode(&ecs_blend_mode_str);
            material_uniforms.opacity = ecs_opacity;
            material_uniforms.texture_scale = world
                .get_components(entity_id)
                .and_then(|c| c.get(comp::MATERIAL))
                .and_then(|m| m.get("texture_scale"))
                .and_then(toml_f32)
                .unwrap_or(1.0);

            let is_transparent = ecs_opacity < 1.0
                || gltf_alpha < 1.0
                || blend_mode != BlendMode::Alpha
                || gpu_mesh.material.alpha_mode == flint_import::AlphaMode::Blend;

            let skinned_draw = SkinnedDrawCall {
                vertex_buffer: gpu_mesh.create_vertex_buffer_copy(device),
                index_buffer: gpu_mesh.create_index_buffer_copy(device),
                index_count: gpu_mesh.index_count,
                transform_buffer,
                transform_bind_group,
                material_buffer,
                material_bind_group,
                bone_bind_group,
                model: model_matrix,
                model_inv_transpose: inv_transpose,
                entity_id: Some(entity_id),
                blend_mode,
                sort_depth: 0.0,
            };

            if is_transparent {
                self.transparent_skinned_draws.push(skinned_draw);
            } else {
                self.skinned_entity_draws.push(skinned_draw);
            }
        }

        true
    }

    /// Extract a standard (non-skinned) model entity into draw calls.
    /// Returns `true` if meshes were found (caller should `continue`).
    pub(super) fn extract_model_entity(
        &mut self,
        device: &wgpu::Device,
        tex_cache: &TextureCache,
        world: &FlintWorld,
        entity_id: flint_core::EntityId,
        asset_name: &str,
        model_matrix: [[f32; 4]; 4],
        need_overlay: bool,
        need_normals: bool,
        arrow_length: f32,
    ) -> bool {
        let gpu_meshes = match self.mesh_cache.get(asset_name) {
            Some(meshes) => meshes,
            None => return false,
        };

        let inv_transpose = mat4_inv_transpose(&model_matrix);

        for gpu_mesh in gpu_meshes {
            let transform_uniforms = TransformUniforms {
                view_proj: [[0.0; 4]; 4],
                model: model_matrix,
                model_inv_transpose: inv_transpose,
                camera_pos: [0.0; 3],
                _pad: 0.0,
            };

            // Resolve textures for this material
            let (bc_view, bc_sampler, has_bc) = Self::resolve_texture(
                tex_cache,
                gpu_mesh.material.base_color_texture.as_deref(),
                &tex_cache.default_white,
            );
            let (nm_view, nm_sampler, has_nm) = Self::resolve_texture(
                tex_cache,
                gpu_mesh.material.normal_texture.as_deref(),
                &tex_cache.default_normal,
            );
            let (mr_view, mr_sampler, has_mr) = Self::resolve_texture(
                tex_cache,
                gpu_mesh.material.metallic_roughness_texture.as_deref(),
                &tex_cache.default_metallic_roughness,
            );

            // Material color override: check entity first, then inherit from parent.
            // This lets scripts color a parent entity and have all child meshes
            // (e.g. expanded GLB nodes) pick up the tint automatically.
            let extract_color_fn = |m: &toml::Value| -> Option<[f32; 4]> {
                let r = m.get("base_color_r")?.as_float()? as f32;
                let g = m.get("base_color_g")?.as_float()? as f32;
                let b = m.get("base_color_b")?.as_float()? as f32;
                let a = m
                    .get("base_color_a")
                    .and_then(|v| v.as_float())
                    .unwrap_or(1.0) as f32;
                Some([r, g, b, a])
            };
            let base_color = world
                .get_components(entity_id)
                .and_then(|c| c.get(comp::MATERIAL))
                .and_then(|m| extract_color_fn(m))
                .or_else(|| {
                    world
                        .get_parent(entity_id)
                        .and_then(|pid| world.get_components(pid))
                        .and_then(|c| c.get(comp::MATERIAL))
                        .and_then(|m| extract_color_fn(m))
                })
                .unwrap_or(gpu_mesh.material.base_color);

            let mut material_uniforms = MaterialUniforms::from_pbr(
                base_color,
                gpu_mesh.material.metallic,
                gpu_mesh.material.roughness,
            );
            material_uniforms.has_base_color_tex = if has_bc { 1 } else { 0 };
            material_uniforms.has_normal_map = if has_nm { 1 } else { 0 };
            material_uniforms.has_metallic_roughness_tex = if has_mr { 1 } else { 0 };
            if gpu_mesh.material.use_vertex_color {
                material_uniforms.use_vertex_color = 1;
            }

            // Read opacity and blend_mode from ECS material component
            let ecs_opacity = world
                .get_components(entity_id)
                .and_then(|c| c.get(comp::MATERIAL))
                .and_then(|m| m.get("opacity"))
                .and_then(toml_f32)
                .unwrap_or(1.0);
            let ecs_blend_mode_str = world
                .get_components(entity_id)
                .and_then(|c| c.get(comp::MATERIAL))
                .and_then(|m| m.get("blend_mode"))
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            let blend_mode = parse_blend_mode(&ecs_blend_mode_str);
            material_uniforms.opacity = ecs_opacity;
            material_uniforms.texture_scale = world
                .get_components(entity_id)
                .and_then(|c| c.get(comp::MATERIAL))
                .and_then(|m| m.get("texture_scale"))
                .and_then(toml_f32)
                .unwrap_or(1.0);

            let gltf_alpha = gpu_mesh.material.base_color[3];
            let is_transparent = ecs_opacity < 1.0
                || gltf_alpha < 1.0
                || base_color[3] < 1.0
                || blend_mode != BlendMode::Alpha
                || gpu_mesh.material.alpha_mode == flint_import::AlphaMode::Blend;

            let mut draw = Self::create_imported_draw_call(
                device,
                &self.pipeline,
                gpu_mesh,
                transform_uniforms,
                material_uniforms,
                bc_view,
                bc_sampler,
                nm_view,
                nm_sampler,
                mr_view,
                mr_sampler,
            );
            draw.entity_id = Some(entity_id);
            draw.blend_mode = blend_mode;

            if is_transparent {
                self.transparent_draws.push(draw);
            } else {
                self.entity_draws.push(draw);
            }

            // Generate wireframe overlay for imported meshes
            if need_overlay {
                let tri_indices = gpu_mesh.triangle_indices();
                let wire_indices = triangles_to_wireframe_indices(&tri_indices);
                if !wire_indices.is_empty() {
                    let vertices = gpu_mesh.vertices();
                    let black_verts: Vec<_> = vertices
                        .iter()
                        .map(|v| crate::primitives::Vertex {
                            color: [0.0, 0.0, 0.0, 1.0],
                            ..*v
                        })
                        .collect();
                    let wire_mesh = Mesh {
                        vertices: black_verts,
                        indices: wire_indices,
                    };
                    let wire_transform = TransformUniforms {
                        view_proj: [[0.0; 4]; 4],
                        model: model_matrix,
                        model_inv_transpose: inv_transpose,
                        camera_pos: [0.0; 3],
                        _pad: 0.0,
                    };
                    let overlay = Self::create_draw_call(
                        device,
                        &self.pipeline,
                        &wire_mesh,
                        true,
                        wire_transform,
                        MaterialUniforms::procedural(),
                        tex_cache,
                    );
                    self.wireframe_overlay_draws.push(overlay);
                }
            }

            // Generate normal arrows for imported meshes
            if need_normals {
                let tri_indices = gpu_mesh.triangle_indices();
                let vertices = gpu_mesh.vertices();
                let arrows = generate_normal_arrows(&vertices, &tri_indices, arrow_length);
                if !arrows.indices.is_empty() {
                    let arrow_transform = TransformUniforms {
                        view_proj: [[0.0; 4]; 4],
                        model: model_matrix,
                        model_inv_transpose: inv_transpose,
                        camera_pos: [0.0; 3],
                        _pad: 0.0,
                    };
                    let arrow_draw = Self::create_draw_call(
                        device,
                        &self.pipeline,
                        &arrows,
                        true,
                        arrow_transform,
                        MaterialUniforms::procedural(),
                        tex_cache,
                    );
                    self.normal_arrow_draws.push(arrow_draw);
                }
            }
        }

        true
    }

    /// Extract a sprite entity (sprite2d or billboard) into draw calls.
    pub(super) fn extract_sprite_entity(
        &mut self,
        device: &wgpu::Device,
        tex_cache: &TextureCache,
        world: &FlintWorld,
        entity_id: flint_core::EntityId,
        components: &DynamicComponents,
        sprite: &toml::Value,
        world_pos: [f32; 3],
        sprite2d_collected: &mut Vec<(String, i32, Sprite2dInstanceGpu)>,
    ) {
        let visible = sprite
            .get("visible")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let mode = sprite
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("billboard");

        if visible && mode == "sprite2d" {
            // ── Sprite2D path: collect for batched instanced rendering ──
            self.extract_sprite2d(
                tex_cache,
                world,
                entity_id,
                components,
                sprite,
                world_pos,
                sprite2d_collected,
            );
        } else if visible {
            // ── Billboard path: existing per-entity rendering ──
            self.extract_billboard(device, tex_cache, entity_id, sprite, world_pos);
        }
    }

    /// Sprite2D extraction — collect instances for batched rendering.
    fn extract_sprite2d(
        &mut self,
        tex_cache: &TextureCache,
        _world: &FlintWorld,
        _entity_id: flint_core::EntityId,
        components: &DynamicComponents,
        sprite: &toml::Value,
        world_pos: [f32; 3],
        sprite2d_collected: &mut Vec<(String, i32, Sprite2dInstanceGpu)>,
    ) {
        let tex_name = sprite.get("texture").and_then(|v| v.as_str()).unwrap_or("");
        let width = sprite.get("width").and_then(toml_f32).unwrap_or(1.0);
        let height = sprite.get("height").and_then(toml_f32).unwrap_or(1.0);
        let anchor_y = sprite.get("anchor_y").and_then(toml_f32).unwrap_or(0.0);
        let layer = sprite
            .get("layer")
            .and_then(|v| v.as_integer())
            .unwrap_or(0) as i32;
        let flip_x = sprite
            .get("flip_x")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let flip_y = sprite
            .get("flip_y")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Read tint color
        let tint = sprite
            .get("tint")
            .and_then(toml_vec4)
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);

        // Read source_rect [x, y, w, h] in pixels
        let source_rect = sprite
            .get("source_rect")
            .and_then(toml_vec4)
            .unwrap_or([0.0, 0.0, 0.0, 0.0]);

        // Convert source_rect to UV coordinates
        let uv_rect = if source_rect[2] > 0.0 && source_rect[3] > 0.0 {
            // Get texture dimensions for pixel → UV conversion
            let (tw, th) = tex_cache.get_dimensions(tex_name).unwrap_or((1, 1));
            let tw = tw as f32;
            let th = th as f32;
            [
                source_rect[0] / tw,
                source_rect[1] / th,
                (source_rect[0] + source_rect[2]) / tw,
                (source_rect[1] + source_rect[3]) / th,
            ]
        } else {
            // Full texture
            [0.0, 0.0, 1.0, 1.0]
        };

        // ── Screen anchor vs parallax position ──
        let screen_anchor = components.get(comp::SCREEN_ANCHOR);
        let (adjusted_x, adjusted_y, skip_tiling) = if let Some(sa) = screen_anchor {
            let anchor_name = sa
                .get("anchor")
                .and_then(|v| v.as_str())
                .unwrap_or("center");
            let off_x = sa.get("offset_x").and_then(toml_f32).unwrap_or(0.0);
            let off_y = sa.get("offset_y").and_then(toml_f32).unwrap_or(0.0);
            let half_w = self.ortho_height * self.aspect_ratio * 0.5;
            let half_h = self.ortho_height * 0.5;
            let (ax, ay) = anchor_origin(anchor_name, half_w, half_h);
            (
                self.camera_offset[0] + ax + off_x,
                self.camera_offset[1] + ay + off_y,
                true,
            )
        } else {
            let parallax = components.get("parallax");
            let scroll_rate = parallax
                .and_then(|p| p.get("scroll_rate"))
                .and_then(toml_f32)
                .unwrap_or(1.0);
            (
                world_pos[0] + self.camera_offset[0] * (1.0 - scroll_rate),
                world_pos[1] + self.camera_offset[1] * (1.0 - scroll_rate),
                false,
            )
        };

        // ── ui_fill clipping ──
        let (uv_rect, width, height, fill_x_off, fill_y_off) =
            if let Some(fill) = components.get(comp::UI_FILL) {
                let value = fill.get("value").and_then(toml_f32).unwrap_or(1.0);
                let direction = fill
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("left_to_right");
                apply_fill(uv_rect, width, height, value, direction)
            } else {
                (uv_rect, width, height, 0.0, 0.0)
            };

        let final_x = adjusted_x + fill_x_off;
        let final_y = adjusted_y + fill_y_off;

        // ── Parallax tiling (skipped for screen-anchored sprites) ──
        if !skip_tiling {
            let parallax = components.get("parallax");
            let repeat_x = parallax
                .and_then(|p| p.get("repeat_x"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let repeat_y = parallax
                .and_then(|p| p.get("repeat_y"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if repeat_x || repeat_y {
                let viewport_h = self.ortho_height;
                let viewport_w = viewport_h * self.aspect_ratio;
                let cam_x = self.camera_offset[0];
                let cam_y = self.camera_offset[1];

                let (start_tx, end_tx) = if repeat_x && width > 0.0 {
                    let left = cam_x - viewport_w * 0.5;
                    let right = cam_x + viewport_w * 0.5;
                    let start = ((left - final_x) / width).floor() as i32 - 1;
                    let end = ((right - final_x) / width).ceil() as i32 + 1;
                    (start, end)
                } else {
                    (0, 1)
                };
                let (start_ty, end_ty) = if repeat_y && height > 0.0 {
                    let bottom = cam_y - viewport_h * 0.5;
                    let top = cam_y + viewport_h * 0.5;
                    let start = ((bottom - final_y) / height).floor() as i32 - 1;
                    let end = ((top - final_y) / height).ceil() as i32 + 1;
                    (start, end)
                } else {
                    (0, 1)
                };

                for ty in start_ty..end_ty {
                    for tx in start_tx..end_tx {
                        let tile_x = final_x + tx as f32 * width;
                        let tile_y = final_y + ty as f32 * height;
                        let instance = Sprite2dInstanceGpu {
                            pos_layer: [tile_x, tile_y, 0.5, layer as f32 * 0.01],
                            size: [width, height, anchor_y, 0.0],
                            uv_rect,
                            tint,
                            flags: [
                                if flip_x { 1.0 } else { 0.0 },
                                if flip_y { 1.0 } else { 0.0 },
                                0.0,
                                0.0,
                            ],
                        };
                        sprite2d_collected.push((tex_name.to_string(), layer, instance));
                    }
                }
                return; // tiled sprites already emitted
            }
        }

        // Single sprite instance
        let instance = Sprite2dInstanceGpu {
            pos_layer: [final_x, final_y, 0.5, layer as f32 * 0.01],
            size: [width, height, anchor_y, 0.0],
            uv_rect,
            tint,
            flags: [
                if flip_x { 1.0 } else { 0.0 },
                if flip_y { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        };
        sprite2d_collected.push((tex_name.to_string(), layer, instance));

        // ── ui_text: expand text to glyph sprite instances ──
        if let Some(ui_text) = components.get(comp::UI_TEXT) {
            let font_path_str = ui_text.get("font").and_then(|v| v.as_str()).unwrap_or("");
            let text = ui_text.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let text_size = ui_text.get("size").and_then(toml_f32).unwrap_or(1.0);
            let text_color = ui_text
                .get("color")
                .and_then(toml_vec4)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let align = ui_text
                .get("align")
                .and_then(|v| v.as_str())
                .unwrap_or("left");

            if !font_path_str.is_empty() && !text.is_empty() {
                // Load/cache the bitmap font
                if !self.bitmap_font_cache.contains_key(font_path_str) {
                    if let Some(scene_dir) = &self.scene_dir {
                        let font_file = scene_dir.join(font_path_str);
                        if let Some(font) = BitmapFont::load(&font_file) {
                            self.bitmap_font_cache
                                .insert(font_path_str.to_string(), font);
                        }
                    }
                }

                if let Some(font) = self.bitmap_font_cache.get(font_path_str) {
                    let (tex_w, tex_h) = tex_cache.get_dimensions(&font.texture).unwrap_or((1, 1));
                    let glyphs = font.layout_text(
                        text, final_x, final_y, text_size, text_color, layer, align, tex_w, tex_h,
                    );
                    sprite2d_collected.extend(glyphs);
                }
            }
        }
    }

    /// Billboard sprite extraction — create per-entity draw call.
    fn extract_billboard(
        &mut self,
        device: &wgpu::Device,
        tex_cache: &TextureCache,
        entity_id: flint_core::EntityId,
        sprite: &toml::Value,
        world_pos: [f32; 3],
    ) {
        let bp = match &self.billboard_pipeline {
            Some(bp) => bp,
            None => return,
        };

        let tex_name = sprite.get("texture").and_then(|v| v.as_str()).unwrap_or("");
        let width = sprite.get("width").and_then(toml_f32).unwrap_or(1.0);
        let height = sprite.get("height").and_then(toml_f32).unwrap_or(1.0);
        let frame = sprite
            .get("frame")
            .and_then(|v| v.as_integer())
            .unwrap_or(0) as u32;
        let frames_x = sprite
            .get("frames_x")
            .and_then(|v| v.as_integer())
            .unwrap_or(1) as u32;
        let frames_y = sprite
            .get("frames_y")
            .and_then(|v| v.as_integer())
            .unwrap_or(1) as u32;
        let anchor_y = sprite.get("anchor_y").and_then(toml_f32).unwrap_or(0.0);
        let fullbright = sprite
            .get("fullbright")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let sprite_instance = SpriteInstance {
            world_pos,
            width,
            height,
            frame,
            frames_x,
            frames_y,
            anchor_y,
            fullbright: if fullbright { 1 } else { 0 },
            selection_highlight: 0,
            _pad1: 0.0,
        };

        // Billboard uniforms will be filled during render (need camera)
        let billboard_uniforms = BillboardUniforms {
            view_proj: [[0.0; 4]; 4],
            camera_right: [1.0, 0.0, 0.0],
            _pad0: 0.0,
            camera_up: [0.0, 1.0, 0.0],
            _pad1: 0.0,
        };

        let billboard_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Billboard Uniform Buffer"),
            contents: bytemuck::cast_slice(&[billboard_uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let sprite_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sprite Instance Buffer"),
            contents: bytemuck::cast_slice(&[sprite_instance]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let billboard_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bp.billboard_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: billboard_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sprite_buffer.as_entire_binding(),
                },
            ],
            label: Some("Billboard Bind Group"),
        });

        // Resolve sprite texture
        let (tex_view, tex_sampler, _has_tex) = Self::resolve_texture(
            tex_cache,
            if tex_name.is_empty() {
                None
            } else {
                Some(tex_name)
            },
            &tex_cache.default_white,
        );

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bp.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(tex_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(tex_sampler),
                },
            ],
            label: Some("Billboard Texture Bind Group"),
        });

        self.billboard_draws.push(BillboardDrawCall {
            billboard_buffer,
            sprite_buffer,
            billboard_bind_group,
            texture_bind_group,
            entity_id: Some(entity_id),
        });
    }

    /// Extract a standalone `ui_text` entity (without a sprite component).
    /// Returns `true` if the entity was processed (caller should `continue`).
    pub(super) fn extract_ui_text_entity(
        &mut self,
        tex_cache: &TextureCache,
        entity_id: flint_core::EntityId,
        components: &DynamicComponents,
        world_pos: [f32; 3],
        sprite2d_collected: &mut Vec<(String, i32, Sprite2dInstanceGpu)>,
    ) -> bool {
        let has_sprite = components.get(comp::SPRITE).is_some();
        if has_sprite {
            return false;
        }

        let ui_text = match components.get(comp::UI_TEXT) {
            Some(ut) => ut,
            None => return false,
        };

        let font_path_str = ui_text.get("font").and_then(|v| v.as_str()).unwrap_or("");
        let text = ui_text.get("text").and_then(|v| v.as_str()).unwrap_or("");

        if font_path_str.is_empty() || text.is_empty() {
            return false;
        }

        let text_size = ui_text.get("size").and_then(toml_f32).unwrap_or(1.0);
        let text_color = ui_text
            .get("color")
            .and_then(toml_vec4)
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);
        let align = ui_text
            .get("align")
            .and_then(|v| v.as_str())
            .unwrap_or("left");
        let layer = ui_text
            .get("layer")
            .and_then(|v| v.as_integer())
            .unwrap_or(0) as i32;

        // Compute position from screen_anchor or world pos
        let (text_x, text_y) = if let Some(sa) = components.get(comp::SCREEN_ANCHOR) {
            let anchor_name = sa
                .get("anchor")
                .and_then(|v| v.as_str())
                .unwrap_or("center");
            let off_x = sa.get("offset_x").and_then(toml_f32).unwrap_or(0.0);
            let off_y = sa.get("offset_y").and_then(toml_f32).unwrap_or(0.0);
            let half_w = self.ortho_height * self.aspect_ratio * 0.5;
            let half_h = self.ortho_height * 0.5;
            let (ax, ay) = anchor_origin(anchor_name, half_w, half_h);
            (
                self.camera_offset[0] + ax + off_x,
                self.camera_offset[1] + ay + off_y,
            )
        } else {
            (world_pos[0], world_pos[1])
        };

        // Load/cache the bitmap font
        if !self.bitmap_font_cache.contains_key(font_path_str) {
            if let Some(scene_dir) = &self.scene_dir {
                let font_file = scene_dir.join(font_path_str);
                if let Some(font) = BitmapFont::load(&font_file) {
                    self.bitmap_font_cache
                        .insert(font_path_str.to_string(), font);
                }
            }
        }

        if let Some(font) = self.bitmap_font_cache.get(font_path_str) {
            let (tex_w, tex_h) = tex_cache.get_dimensions(&font.texture).unwrap_or((1, 1));
            let glyphs = font.layout_text(
                text, text_x, text_y, text_size, text_color, layer, align, tex_w, tex_h,
            );
            sprite2d_collected.extend(glyphs);
        }

        let _ = entity_id; // used for future selection support
        true
    }

    /// Extract a bounds-only entity into procedural geometry draw calls.
    pub(super) fn extract_bounds_entity(
        &mut self,
        device: &wgpu::Device,
        tex_cache: &TextureCache,
        world: &FlintWorld,
        entity_id: flint_core::EntityId,
        visual: &super::ArchetypeVisual,
        model_matrix: [[f32; 4]; 4],
        need_overlay: bool,
        need_normals: bool,
        arrow_length: f32,
    ) {
        // Fall back to procedural shapes
        let (size, bounds_center) = if let Some(components) = world.get_components(entity_id) {
            if let Some(bounds) = components.get(comp::BOUNDS) {
                extract_bounds_info(bounds).unwrap_or((visual.default_size, [0.0, 0.0, 0.0]))
            } else {
                (visual.default_size, [0.0, 0.0, 0.0])
            }
        } else {
            (visual.default_size, [0.0, 0.0, 0.0])
        };

        let mesh = if visual.wireframe {
            create_wireframe_box_mesh(size[0], size[1], size[2], visual.color)
        } else {
            create_box_mesh(size[0], size[1], size[2], visual.color)
        };

        let mut model = model_matrix;
        // Apply bounds_center in local space so rotation pivots around entity position
        let rx = model[0][0] * bounds_center[0]
            + model[1][0] * bounds_center[1]
            + model[2][0] * bounds_center[2];
        let ry = model[0][1] * bounds_center[0]
            + model[1][1] * bounds_center[1]
            + model[2][1] * bounds_center[2];
        let rz = model[0][2] * bounds_center[0]
            + model[1][2] * bounds_center[1]
            + model[2][2] * bounds_center[2];
        model[3][0] += rx;
        model[3][1] += ry;
        model[3][2] += rz;

        let inv_transpose = mat4_inv_transpose(&model);

        let transform_uniforms = TransformUniforms {
            view_proj: [[0.0; 4]; 4],
            model,
            model_inv_transpose: inv_transpose,
            camera_pos: [0.0; 3],
            _pad: 0.0,
        };

        // Check for material.texture to use file-based textures on procedural geometry
        let material_component = world
            .get_components(entity_id)
            .and_then(|components| components.get(comp::MATERIAL).cloned());

        let material_texture = material_component
            .as_ref()
            .and_then(|m| m.get("texture").and_then(|v| v.as_str().map(String::from)));

        // Extract opacity and blend_mode for procedural geometry
        let proc_opacity = material_component
            .as_ref()
            .and_then(|m| m.get("opacity"))
            .and_then(toml_f32)
            .unwrap_or(1.0);
        let proc_texture_scale = material_component
            .as_ref()
            .and_then(|m| m.get("texture_scale"))
            .and_then(toml_f32)
            .unwrap_or(1.0);
        let proc_blend_mode_str = material_component
            .as_ref()
            .and_then(|m| m.get("blend_mode"))
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let proc_blend_mode = parse_blend_mode(&proc_blend_mode_str);
        let proc_is_transparent = proc_opacity < 1.0 || proc_blend_mode != BlendMode::Alpha;

        if !visual.wireframe {
            if let Some(tex_name) = &material_texture {
                let (bc_view, bc_sampler, has_bc) = Self::resolve_texture(
                    tex_cache,
                    Some(tex_name.as_str()),
                    &tex_cache.default_white,
                );

                if has_bc {
                    let metallic = material_component
                        .as_ref()
                        .and_then(|m| m.get("metallic"))
                        .and_then(toml_f32)
                        .unwrap_or(0.0);
                    let roughness = material_component
                        .as_ref()
                        .and_then(|m| m.get("roughness"))
                        .and_then(toml_f32)
                        .unwrap_or(0.7);

                    // Try companion PBR maps (e.g. tavern_brick_normal, tavern_brick_roughness)
                    let normal_name = format!("{}_normal", tex_name);
                    let roughness_name = format!("{}_roughness", tex_name);
                    let (nm_view, nm_sampler, has_nm) = Self::resolve_texture(
                        tex_cache,
                        Some(normal_name.as_str()),
                        &tex_cache.default_normal,
                    );
                    let (mr_view, mr_sampler, has_mr) = Self::resolve_texture(
                        tex_cache,
                        Some(roughness_name.as_str()),
                        &tex_cache.default_metallic_roughness,
                    );

                    let mut material_uniforms =
                        MaterialUniforms::from_pbr([1.0, 1.0, 1.0, 1.0], metallic, roughness);
                    material_uniforms.has_base_color_tex = 1;
                    material_uniforms.has_normal_map = if has_nm { 1 } else { 0 };
                    material_uniforms.has_metallic_roughness_tex = if has_mr { 1 } else { 0 };
                    material_uniforms.opacity = proc_opacity;
                    material_uniforms.texture_scale = proc_texture_scale;

                    let mut draw = Self::create_textured_draw_call(
                        device,
                        &self.pipeline,
                        &mesh,
                        transform_uniforms,
                        material_uniforms,
                        bc_view,
                        bc_sampler,
                        nm_view,
                        nm_sampler,
                        mr_view,
                        mr_sampler,
                    );
                    draw.entity_id = Some(entity_id);
                    draw.blend_mode = proc_blend_mode;
                    if proc_is_transparent {
                        self.transparent_draws.push(draw);
                    } else {
                        self.entity_draws.push(draw);
                    }
                } else {
                    let mut draw = Self::create_draw_call(
                        device,
                        &self.pipeline,
                        &mesh,
                        false,
                        transform_uniforms,
                        MaterialUniforms::procedural(),
                        tex_cache,
                    );
                    draw.entity_id = Some(entity_id);
                    self.entity_draws.push(draw);
                }
            } else {
                // Use material.color for PBR base color if present
                let mat_color = material_component
                    .as_ref()
                    .and_then(|m| m.get("color"))
                    .and_then(|v| extract_color(v));

                let mut mat_uniforms = if let Some(color) = mat_color {
                    let metallic = material_component
                        .as_ref()
                        .and_then(|m| m.get("metallic"))
                        .and_then(toml_f32)
                        .unwrap_or(0.0);
                    let roughness = material_component
                        .as_ref()
                        .and_then(|m| m.get("roughness"))
                        .and_then(toml_f32)
                        .unwrap_or(0.7);
                    MaterialUniforms::from_pbr(color, metallic, roughness)
                } else {
                    MaterialUniforms::procedural()
                };
                mat_uniforms.opacity = proc_opacity;
                mat_uniforms.texture_scale = proc_texture_scale;

                let mut draw = Self::create_draw_call(
                    device,
                    &self.pipeline,
                    &mesh,
                    false,
                    transform_uniforms,
                    mat_uniforms,
                    tex_cache,
                );
                draw.entity_id = Some(entity_id);
                draw.blend_mode = proc_blend_mode;
                if proc_is_transparent {
                    self.transparent_draws.push(draw);
                } else {
                    self.entity_draws.push(draw);
                }
            }
        } else {
            let mut draw = Self::create_draw_call(
                device,
                &self.pipeline,
                &mesh,
                true,
                transform_uniforms,
                MaterialUniforms::procedural(),
                tex_cache,
            );
            draw.entity_id = Some(entity_id);
            self.entity_draws.push(draw);
        }

        // Generate wireframe overlay for procedural solid shapes
        if need_overlay && !visual.wireframe {
            let wire_indices = triangles_to_wireframe_indices(&mesh.indices);
            if !wire_indices.is_empty() {
                let black_verts: Vec<_> = mesh
                    .vertices
                    .iter()
                    .map(|v| crate::primitives::Vertex {
                        color: [0.0, 0.0, 0.0, 1.0],
                        ..*v
                    })
                    .collect();
                let wire_mesh = Mesh {
                    vertices: black_verts,
                    indices: wire_indices,
                };
                let wire_transform = TransformUniforms {
                    view_proj: [[0.0; 4]; 4],
                    model,
                    model_inv_transpose: inv_transpose,
                    camera_pos: [0.0; 3],
                    _pad: 0.0,
                };
                let overlay = Self::create_draw_call(
                    device,
                    &self.pipeline,
                    &wire_mesh,
                    true,
                    wire_transform,
                    MaterialUniforms::procedural(),
                    tex_cache,
                );
                self.wireframe_overlay_draws.push(overlay);
            }
        }

        // Generate normal arrows for procedural solid shapes
        if need_normals && !visual.wireframe {
            let arrows = generate_normal_arrows(&mesh.vertices, &mesh.indices, arrow_length);
            if !arrows.indices.is_empty() {
                let arrow_transform = TransformUniforms {
                    view_proj: [[0.0; 4]; 4],
                    model,
                    model_inv_transpose: inv_transpose,
                    camera_pos: [0.0; 3],
                    _pad: 0.0,
                };
                let arrow_draw = Self::create_draw_call(
                    device,
                    &self.pipeline,
                    &arrows,
                    true,
                    arrow_transform,
                    MaterialUniforms::procedural(),
                    tex_cache,
                );
                self.normal_arrow_draws.push(arrow_draw);
            }
        }
    }

    /// Sort collected sprite2d instances and batch them by texture for instanced rendering.
    pub(super) fn batch_sprite2d_instances(
        &mut self,
        device: &wgpu::Device,
        tex_cache: &TextureCache,
        mut collected: Vec<(String, i32, Sprite2dInstanceGpu)>,
    ) {
        if collected.is_empty() {
            return;
        }

        let sp = match &self.sprite2d_pipeline {
            Some(sp) => sp,
            None => return,
        };

        // Sort by (layer, texture_name) for optimal batching
        collected.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

        // Group consecutive same-texture sprites into batches
        let mut batch_start = 0;
        while batch_start < collected.len() {
            let batch_tex = &collected[batch_start].0;
            let mut batch_end = batch_start + 1;
            while batch_end < collected.len() && collected[batch_end].0 == *batch_tex {
                batch_end += 1;
            }

            let instances: Vec<Sprite2dInstanceGpu> = collected[batch_start..batch_end]
                .iter()
                .map(|(_, _, inst)| *inst)
                .collect();

            let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Sprite2D Instance Buffer"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::STORAGE,
            });

            let instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &sp.instance_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                }],
                label: Some("Sprite2D Instance Bind Group"),
            });

            // Resolve texture
            let (tex_view, tex_sampler, _) = Self::resolve_texture(
                tex_cache,
                if batch_tex.is_empty() {
                    None
                } else {
                    Some(batch_tex.as_str())
                },
                &tex_cache.default_white,
            );

            let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &sp.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(tex_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(tex_sampler),
                    },
                ],
                label: Some("Sprite2D Texture Bind Group"),
            });

            self.sprite2d_batches
                .push(crate::sprite2d_pipeline::Sprite2dBatch {
                    instance_buffer,
                    instance_count: instances.len() as u32,
                    texture_bind_group,
                    instance_bind_group,
                });

            batch_start = batch_end;
        }
    }
}
