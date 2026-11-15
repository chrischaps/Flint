//! Skinned entities' per-entity state, owned by [`SkinningState`] (ADR 0118,
//! #736): each entity's own bone buffer (so entities instancing one asset can
//! play different clips) and the CPU copy the style scissor bounds with
//! (#385), shadow bones of their own, living props' echoes (#228) and
//! whether they may only lighten, per-part tints and soft silhouettes
//! (#582), and the skinned draws' bone bind groups kept across frames.
//! Hosts write it through `SceneRenderer`'s setters, which forward here; the
//! skinned extraction in `extract.rs` reads it while it builds the draws.

use super::SceneRenderer;
use flint_core::EntityId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The tables the skinned draws are built from.
#[derive(Default)]
pub(crate) struct SkinningState {
    /// Per-entity bone matrix buffers. The `GpuSkinnedMesh` bone buffer is
    /// shared by every entity that instances the asset, so entities playing
    /// different clips need their own storage or the last upload wins.
    pub(crate) bone_buffers: HashMap<EntityId, wgpu::Buffer>,
    /// The bone matrices last written for each entity, and for each
    /// asset's shared buffer, kept on the CPU to bound the posed meshes
    /// for the per-entity style scissor (#385).
    pub(crate) bone_matrices: HashMap<EntityId, Vec<[[f32; 4]; 4]>>,
    pub(crate) asset_bone_matrices: HashMap<String, Vec<[[f32; 4]; 4]>>,
    /// Entities whose shadow follows other bones than they're drawn with.
    pub(crate) shadow_bone_buffers: HashMap<EntityId, wgpu::Buffer>,
    /// Per entity, what each skinned part's vertex colour is multiplied by.
    pub(crate) part_tints: HashMap<EntityId, Vec<[f32; 3]>>,
    /// Per entity, how soft each skinned part's silhouette is (#582); a part
    /// missing or at 0 is a hard edge.
    pub(crate) part_softness: HashMap<EntityId, Vec<f32>>,
    /// Living props' echoes, in the order they draw. Empty costs nothing.
    pub(crate) echoes: HashMap<EntityId, Vec<EntityEcho>>,
    /// The entities whose echoes may only add light.
    pub(crate) echo_lighten: HashSet<EntityId>,
    /// Bone bind groups of skinned draws, by entity and sub-mesh.
    pub(crate) bone_binds: HashMap<(EntityId, u32), CachedBoneBind>,
}

/// One skinned draw's bone bind group, kept across frames like
/// [`CachedDrawBinds`](super::CachedDrawBinds). It points at the entity's own bone buffer once
/// `update_bone_matrices` has made one, and at the asset's shared buffer
/// before that; `from_entity` records which, so the switch rebuilds it.
pub(crate) struct CachedBoneBind {
    pub from_entity: bool,
    pub bind_group: Arc<wgpu::BindGroup>,
    pub touched: u64,
}

/// Write `matrices` into a bone buffer from bone `first`, stopping at its end.
pub(crate) fn write_bones(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    first: usize,
    matrices: &[[[f32; 4]; 4]],
) {
    let room = crate::skinned_pipeline::MAX_BONES.saturating_sub(first);
    let matrices = &matrices[..matrices.len().min(room)];
    if !matrices.is_empty() {
        queue.write_buffer(
            buffer,
            (first * std::mem::size_of::<[[f32; 4]; 4]>()) as u64,
            bytemuck::cast_slice(matrices),
        );
    }
}

/// The `bone_binds` key bit for a draw's shadow bones, when it has its
/// own apart from the bones it's drawn with.
pub(crate) const SHADOW_BONE_KEY: u32 = 1 << 30;

/// One faint copy of a skinned entity (#228): bones of its own and how opaque
/// it draws. The rest it takes from the entity's own draw at extraction.
pub(crate) struct EntityEcho {
    pub(crate) bones: wgpu::Buffer,
    pub(crate) bind_group: Arc<wgpu::BindGroup>,
    pub(crate) opacity: f32,
}

impl SkinningState {
    /// Upload an entity's bone matrices to the GPU.
    ///
    /// Each skinned entity gets its own storage buffer (created on first use),
    /// so several entities instancing the same asset can play different clips.
    /// The asset's shared buffer is also written so code paths that draw the
    /// asset without an entity (previews) keep animating.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update_bone_matrices(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        motion_hold: &mut crate::motion_hold::MotionHold,
        mesh_cache: &crate::gpu_mesh::MeshCache,
        entity_id: flint_core::EntityId,
        asset_name: &str,
        matrices: &[[[f32; 4]; 4]],
    ) {
        // A pose held on twos (#405) is written once a tick; the frames
        // between draw the buffer as it was, CPU copies included
        if self.bone_buffers.contains_key(&entity_id) && !motion_hold.bones_due(entity_id) {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(matrices);
        let capacity = crate::skinned_pipeline::MAX_BONES * std::mem::size_of::<[[f32; 4]; 4]>();
        let buffer = self.bone_buffers.entry(entity_id).or_insert_with(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!(
                    "{} Bone Buffer (entity {:?})",
                    asset_name, entity_id
                )),
                size: capacity as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        queue.write_buffer(buffer, 0, &bytes[..bytes.len().min(capacity)]);
        let kept = &matrices[..matrices.len().min(crate::skinned_pipeline::MAX_BONES)];
        let entity = self.bone_matrices.entry(entity_id).or_default();
        entity.clear();
        entity.extend_from_slice(kept);
        if let Some(asset) = self.asset_bone_matrices.get_mut(asset_name) {
            asset.clear();
            asset.extend_from_slice(kept);
        } else {
            self.asset_bone_matrices
                .insert(asset_name.to_string(), kept.to_vec());
        }
        // Its sub-meshes share one buffer
        if let Some(mesh) = mesh_cache
            .get_skinned(asset_name)
            .and_then(|meshes| meshes.first())
        {
            queue.write_buffer(&mesh.bone_buffer, 0, bytes);
        }
    }

    /// Write part of an entity's own bones, from bone `first`, and nothing else.
    ///
    /// This is [`update_bone_matrices`](Self::update_bone_matrices) for a palette
    /// that changes every frame on many entities (#212): that one writes the
    /// asset's shared buffer too, which forty trees would do forty times to no
    /// effect, since each draws from its own.
    pub(crate) fn write_entity_bones(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        entity_id: flint_core::EntityId,
        first: usize,
        matrices: &[[[f32; 4]; 4]],
    ) {
        let buffer = self
            .bone_buffers
            .entry(entity_id)
            .or_insert_with(|| create_entity_bones(device, "Entity Bone Buffer"));
        write_bones(queue, buffer, first, matrices);
    }

    /// Give an entity's shadow its own bones, from the shadow pass's next frame,
    /// or write them again.
    pub(crate) fn write_entity_shadow_bones(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        entity_id: flint_core::EntityId,
        matrices: &[[[f32; 4]; 4]],
    ) {
        let buffer = self
            .shadow_bone_buffers
            .entry(entity_id)
            .or_insert_with(|| create_entity_bones(device, "Entity Shadow Bone Buffer"));
        write_bones(queue, buffer, 0, matrices);
    }

    /// Write part of the bones of echo `echo` of a skinned entity, from bone
    /// `first`, and say how opaque it draws. Writing an echo is what makes it
    /// exist. It then draws from the next extraction, after everything opaque,
    /// in the entity's own colours and with no depth write, in `echo` order. An
    /// entity that's never written one pays nothing.
    ///
    /// Write the whole palette first: until then its bones are zero, which puts
    /// it nowhere.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_entity_echo_bones(
        &mut self,
        bone_layout: Option<&wgpu::BindGroupLayout>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        entity_id: flint_core::EntityId,
        echo: usize,
        opacity: f32,
        first: usize,
        matrices: &[[[f32; 4]; 4]],
    ) {
        let Some(bone_layout) = bone_layout else {
            return;
        };
        let echoes = self.echoes.entry(entity_id).or_default();
        while echoes.len() <= echo {
            let bones = create_entity_bones(device, "Entity Echo Bone Buffer");
            let bind_group = SceneRenderer::create_bone_bind(device, bone_layout, &bones);
            echoes.push(EntityEcho {
                bones,
                bind_group: Arc::new(bind_group),
                opacity: 0.0,
            });
        }
        echoes[echo].opacity = opacity;
        write_bones(queue, &echoes[echo].bones, first, matrices);
    }

    /// Stop drawing an entity's echoes from `count` on. Their buffers stay, ready
    /// for the next write, so a slider sweeping a count allocates nothing on its
    /// way back up: an opacity of 0 is what leaves one out at extraction.
    pub(crate) fn set_entity_echo_count(&mut self, entity_id: flint_core::EntityId, count: usize) {
        if let Some(echoes) = self.echoes.get_mut(&entity_id) {
            for echo in echoes.iter_mut().skip(count) {
                echo.opacity = 0.0;
            }
        }
    }

    /// How an entity's echoes meet what's behind them: over it at their
    /// opacity (`false`, the default), or `lighten`, the brighter of the two,
    /// so an echo can only add light and never darken the form it's behind.
    pub(crate) fn set_entity_echo_lighten(
        &mut self,
        entity_id: flint_core::EntityId,
        lighten: bool,
    ) {
        if lighten {
            self.echo_lighten.insert(entity_id);
        } else {
            self.echo_lighten.remove(&entity_id);
        }
    }

    /// Multiply the vertex colour of one part of an entity's skinned mesh by
    /// `tint`. Materials are shared by content, so a few distinct tints cost a
    /// few materials, however many entities use them: keep them few.
    pub(crate) fn set_entity_part_tint(
        &mut self,
        entity_id: flint_core::EntityId,
        part: usize,
        tint: [f32; 3],
    ) {
        let tints = self.part_tints.entry(entity_id).or_default();
        if tints.len() <= part {
            tints.resize(part + 1, [1.0; 3]);
        }
        tints[part] = tint;
    }

    /// How soft one part of an entity's skinned mesh is at its silhouette
    /// (#582), 0 to 1: above 0 its alpha falls where the surface turns away
    /// from the eye, so the outline feathers, and the part draws translucent,
    /// after everything opaque; its echoes feather with it. 0 is a hard edge,
    /// drawn exactly as with no softness at all. Shared by content like a
    /// tint, so a few distinct values cost a few materials.
    pub(crate) fn set_entity_part_softness(
        &mut self,
        entity_id: flint_core::EntityId,
        part: usize,
        softness: f32,
    ) {
        let softness = if softness.is_finite() {
            softness.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let parts = self.part_softness.entry(entity_id).or_default();
        if parts.len() <= part {
            parts.resize(part + 1, 0.0);
        }
        parts[part] = softness;
    }

    /// Drop every table (a scene is unloaded); the caller clears the hold.
    pub(crate) fn clear(&mut self) {
        self.bone_buffers.clear();
        self.bone_matrices.clear();
        self.asset_bone_matrices.clear();
        self.shadow_bone_buffers.clear();
        self.part_tints.clear();
        self.part_softness.clear();
        self.echoes.clear();
        self.echo_lighten.clear();
        self.bone_binds.clear();
    }
}

fn create_entity_bones(device: &wgpu::Device, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (crate::skinned_pipeline::MAX_BONES * std::mem::size_of::<[[f32; 4]; 4]>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
