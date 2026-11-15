//! Building and keeping draws (ADR 0118, #736): the per-entity bind cache,
//! shared materials, procedural boxes, LOD and frustum culls, the bind
//! groups a draw call is made of, and shallow wooze's per-draw drift.
//! The draw types themselves are in `mod.rs` until #737's `DrawItem`.

use super::*;

impl SceneRenderer {
    /// Transform + material resources for one draw of `key`, created on
    /// first use and reused afterwards; the material side is looked up again
    /// when `material_key` changes, and shared with every other draw of the
    /// same key (see `SharedMaterial`). Free-standing over the cache and pipeline so
    /// callers holding other `self` fields borrowed can still use it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cached_binds<'a>(
        cache: &'a mut DrawBindCache,
        frame: u64,
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        key: (flint_core::EntityId, u32),
        transform_uniforms: &TransformUniforms,
        material_uniforms: &MaterialUniforms,
        material_key: u64,
        base_color: &wgpu::TextureView,
        normal: &wgpu::TextureView,
        metallic_roughness: &wgpu::TextureView,
        emissive: &wgpu::TextureView,
    ) -> &'a CachedDrawBinds {
        let (mb, mbg) = Self::shared_material(
            cache,
            frame,
            device,
            layouts,
            material_uniforms,
            material_key,
            base_color,
            normal,
            metallic_roughness,
            emissive,
        );
        let arena = &mut cache.arena;
        let entry = cache.draws.entry(key).or_insert_with(|| {
            let (slot, page) = arena.alloc(device);
            arena.stage(slot, bytemuck::bytes_of(transform_uniforms));
            let tbg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &layouts.transform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &page,
                        offset: slot.offset(),
                        // The whole slot: a plain draw reads its first 208
                        // bytes, and shallow wooze's the 48 past them too.
                        size: wgpu::BufferSize::new(crate::bind_cache::SLOT_STRIDE),
                    }),
                }],
                label: Some("Transform Bind Group (arena)"),
            });
            CachedDrawBinds {
                transform: TransformTarget::Slot(slot),
                transform_bind_group: Arc::new(tbg),
                material_buffer: mb.clone(),
                material_bind_group: mbg.clone(),
                material_key,
                touched: frame,
            }
        });
        if entry.material_key != material_key {
            entry.material_buffer = mb;
            entry.material_bind_group = mbg;
            entry.material_key = material_key;
        }
        entry.touched = frame;
        entry
    }

    /// The material for `material_key`, made on first use and shared with every
    /// draw of that key.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shared_material(
        cache: &mut DrawBindCache,
        frame: u64,
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        material_uniforms: &MaterialUniforms,
        material_key: u64,
        base_color: &wgpu::TextureView,
        normal: &wgpu::TextureView,
        metallic_roughness: &wgpu::TextureView,
        emissive: &wgpu::TextureView,
    ) -> (Arc<MaterialBuffer>, Arc<wgpu::BindGroup>) {
        let shared = cache.materials.entry(material_key).or_insert_with(|| {
            let (b, bg) = Self::create_material_bind_with_textures(
                device,
                layouts,
                material_uniforms,
                base_color,
                normal,
                metallic_roughness,
                emissive,
            );
            SharedMaterial {
                buffer: Arc::new(MaterialBuffer::new(b)),
                bind_group: Arc::new(bg),
                touched: frame,
            }
        });
        shared.touched = frame;
        (shared.buffer.clone(), shared.bind_group.clone())
    }

    /// The uploaded box for `key`, building it with `build` on a miss.
    pub(super) fn cached_box_mesh<'a>(
        cache: &'a mut HashMap<u64, CachedBoxMesh>,
        device: &wgpu::Device,
        key: u64,
        build: impl FnOnce() -> crate::primitives::Mesh,
    ) -> &'a CachedBoxMesh {
        cache.entry(key).or_insert_with(|| {
            let mesh = build();
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Box Vertex Buffer (cached)"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Box Index Buffer (cached)"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            CachedBoxMesh {
                vertex_buffer: Arc::new(vertex_buffer),
                index_buffer: Arc::new(index_buffer),
                index_count: mesh.indices.len() as u32,
            }
        })
    }

    /// A draw call over cached geometry and cached binds.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_from_cache(
        vertex_buffer: &Arc<wgpu::Buffer>,
        index_buffer: &Arc<wgpu::Buffer>,
        index_count: u32,
        is_wireframe: bool,
        binds: &CachedDrawBinds,
        model: [[f32; 4]; 4],
        model_inv_transpose: [[f32; 4]; 4],
        entity_id: flint_core::EntityId,
        blend_mode: BlendMode,
    ) -> DrawCall {
        DrawCall {
            vertex_buffer: vertex_buffer.clone(),
            index_buffer: index_buffer.clone(),
            index_count,
            is_wireframe,
            casts_shadow: true,
            transform: binds.transform.clone(),
            transform_bind_group: binds.transform_bind_group.clone(),
            material_buffer: binds.material_buffer.clone(),
            material_bind_group: binds.material_bind_group.clone(),
            model,
            model_inv_transpose,
            entity_id: Some(entity_id),
            blend_mode,
            sort_depth: 0.0,
            world_aabb: None,
            style_aabb: None,
            lod: None,
            wooze: None,
            double_sided: false,
        }
    }

    /// The distance levers in force (#56).
    pub fn lod_settings(&self) -> LodSettings {
        self.lod_settings
    }

    /// Change the distance levers (#56): takes effect at the next
    /// `render_to`, no reload. Scales are clamped to
    /// [`LodSettings::MIN_SCALE`] and non-finite values are ignored.
    pub fn set_lod_settings(&mut self, settings: LodSettings) {
        let clamp = |v: f32, old: f32| {
            if v.is_finite() {
                v.max(LodSettings::MIN_SCALE)
            } else {
                old
            }
        };
        self.lod_settings = LodSettings {
            lod_scale: clamp(settings.lod_scale, self.lod_settings.lod_scale),
            draw_distance_scale: clamp(
                settings.draw_distance_scale,
                self.lod_settings.draw_distance_scale,
            ),
        };
    }

    /// Draws set aside by the last `render_to` for their level of detail or
    /// draw distance (they are also in `culled_draws`).
    pub fn lod_culled_draws(&self) -> u32 {
        self.lod_culled
    }

    /// Move draws whose level of detail isn't the one for a camera at `eye`,
    /// or which are past their draw distance, into the culled lists, before
    /// the shadow pass: a far chunk's shadow comes from its simple level too.
    pub(super) fn cull_draws_to_lod(&mut self, eye: [f32; 3]) {
        let LodSettings {
            lod_scale,
            draw_distance_scale,
        } = self.lod_settings;
        let mut culled = 0u32;
        let mut split = |draws: &mut Vec<DrawCall>, out: &mut Vec<DrawCall>| {
            if !draws.iter().any(|d| d.lod.is_some()) {
                return;
            }
            let (keep, cull): (Vec<_>, Vec<_>) = draws.drain(..).partition(|d| {
                d.lod.map_or(true, |l| {
                    l.visible_from(eye, lod_scale, draw_distance_scale)
                })
            });
            *draws = keep;
            culled += cull.len() as u32;
            out.extend(cull);
        };
        split(&mut self.entity_draws, &mut self.culled_entity_draws);
        split(
            &mut self.transparent_draws,
            &mut self.culled_transparent_draws,
        );
        self.lod_culled = culled;
    }

    /// Move opaque and transparent draws whose world bounds miss the camera
    /// frustum into the culled lists. Wireframe draws and draws without
    /// bounds always stay (ADR 0074).
    pub(super) fn cull_draws_to_camera(&mut self) {
        let Some(frustum) = &self.camera_frustum else {
            return;
        };
        let split = |draws: &mut Vec<DrawCall>, culled: &mut Vec<DrawCall>| {
            let (keep, cull): (Vec<_>, Vec<_>) = draws
                .drain(..)
                .partition(|d| d.is_wireframe || draw_visible(d, frustum));
            *draws = keep;
            culled.extend(cull);
        };
        split(&mut self.entity_draws, &mut self.culled_entity_draws);
        split(
            &mut self.transparent_draws,
            &mut self.culled_transparent_draws,
        );
    }

    pub(super) fn create_draw_call(
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        mesh: &crate::primitives::Mesh,
        is_wireframe: bool,
        transform_uniforms: TransformUniforms,
        material_uniforms: MaterialUniforms,
        texture_cache: &TextureCache,
    ) -> DrawCall {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (transform_buffer, transform_bind_group) =
            Self::create_transform_bind(device, layouts, &transform_uniforms);
        let (material_buffer, material_bind_group) =
            Self::create_material_bind(device, layouts, &material_uniforms, texture_cache);

        DrawCall {
            vertex_buffer: Arc::new(vertex_buffer),
            index_buffer: Arc::new(index_buffer),
            index_count: mesh.indices.len() as u32,
            is_wireframe,
            casts_shadow: true,
            transform: TransformTarget::Own(Arc::new(transform_buffer)),
            transform_bind_group: Arc::new(transform_bind_group),
            material_buffer: Arc::new(MaterialBuffer::new(material_buffer)),
            material_bind_group: Arc::new(material_bind_group),
            model: transform_uniforms.model,
            model_inv_transpose: transform_uniforms.model_inv_transpose,
            entity_id: None,
            blend_mode: BlendMode::Alpha,
            sort_depth: 0.0,
            world_aabb: None,
            style_aabb: None,
            lod: None,
            wooze: None,
            double_sided: false,
        }
    }

    /// A skinned draw's bind group over `bones`, the entity's own bone
    /// buffer or the asset's shared one.
    pub(super) fn create_bone_bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        bones: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bones.as_entire_binding(),
            }],
            label: Some("Skinned Draw Bone Bind Group"),
        })
    }

    pub(super) fn create_transform_bind(
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        uniforms: &TransformUniforms,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Transform Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &layouts.transform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: Some("Transform Bind Group"),
        });

        (buffer, bind_group)
    }

    pub(super) fn create_material_bind(
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        uniforms: &MaterialUniforms,
        texture_cache: &TextureCache,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        Self::create_material_bind_with_textures(
            device,
            layouts,
            uniforms,
            &texture_cache.default_white.view,
            &texture_cache.default_normal.view,
            &texture_cache.default_metallic_roughness.view,
            &texture_cache.default_white.view,
        )
    }

    pub(super) fn create_material_bind_with_textures(
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        uniforms: &MaterialUniforms,
        base_color_view: &wgpu::TextureView,
        normal_view: &wgpu::TextureView,
        mr_view: &wgpu::TextureView,
        emissive_view: &wgpu::TextureView,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Material Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &layouts.material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(base_color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(mr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(emissive_view),
                },
            ],
            label: Some("Material Bind Group"),
        });

        (buffer, bind_group)
    }

    /// Resolve a texture reference, returning the view, sampler, and whether a real texture was found
    pub(super) fn resolve_texture<'a>(
        cache: &'a TextureCache,
        name: Option<&str>,
        default: &'a crate::texture_cache::GpuTexture,
    ) -> (&'a wgpu::TextureView, &'a wgpu::Sampler, bool) {
        if let Some(name) = name {
            if let Some(gpu_tex) = cache.get(name) {
                return (&gpu_tex.view, &gpu_tex.sampler, true);
            }
        }
        (&default.view, &default.sampler, false)
    }

    /// Replace which entities have shallow wooze this frame, and how far along
    /// each is (#215). What's named here shows from the next `update_from_world`.
    /// An empty list, the default, costs a scene nothing.
    pub fn set_shallow_wooze(
        &mut self,
        draws: impl IntoIterator<Item = (flint_core::EntityId, ShallowWoozeDraw)>,
    ) {
        self.shallow_wooze.clear();
        self.shallow_wooze.extend(draws);
    }

    /// Which of shallow wooze's pipelines exist: standard, skinned and shadow.
    #[cfg(test)]
    pub(crate) fn wooze_built(&self) -> (bool, bool, bool) {
        (
            self.pipelines.wooze_standard.is_some(),
            self.pipelines.wooze_skinned.is_some(),
            self.shadow_pass
                .as_ref()
                .is_some_and(|sp| sp.wooze.is_some()),
        )
    }

    /// Give this frame's draws of woozy entities their drift, grow their bounds to
    /// match, and build the pipelines they need the first time they need them.
    pub(super) fn apply_shallow_wooze(&mut self, device: &wgpu::Device) {
        if self.shallow_wooze.is_empty() {
            return;
        }
        let map = &self.shallow_wooze;
        let drift = |id: Option<flint_core::EntityId>| Some(map.get(&id?)?.uniforms());

        let mut any_standard = false;
        for draw in self
            .entity_draws
            .iter_mut()
            .chain(self.transparent_draws.iter_mut())
            .filter(|d| !d.is_wireframe)
        {
            let Some(wooze) = drift(draw.entity_id) else {
                continue;
            };
            if let Some(bounds) = &mut draw.world_aabb {
                grow_bounds(bounds, &wooze);
            }
            draw.wooze = Some(wooze);
            any_standard = true;
        }

        let mut any_skinned = false;
        for draw in self
            .skinned_entity_draws
            .iter_mut()
            .chain(self.transparent_skinned_draws.iter_mut())
        {
            let Some(wooze) = drift(draw.entity_id) else {
                continue;
            };
            draw.wooze = Some(wooze);
            any_skinned = true;
        }

        self.pipelines
            .ensure_wooze(device, any_standard, any_skinned);
        if any_standard || any_skinned {
            if let Some(shadow_pass) = &mut self.shadow_pass {
                shadow_pass.ensure_wooze(device);
            }
        }
    }
}
