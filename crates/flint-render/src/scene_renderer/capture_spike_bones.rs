//! SPIKE (#1082, throwaway): the renderer's skinning state as a capture
//! would hold it, read back from the GPU (living props keep no CPU copy).

use super::SceneRenderer;
use web_time::Instant;

pub struct SpikeSkinning {
    pub bones: Vec<(flint_core::EntityId, Vec<[[f32; 4]; 4]>)>,
    pub shadow_bones: Vec<(flint_core::EntityId, Vec<[[f32; 4]; 4]>)>,
    pub part_tints: Vec<(flint_core::EntityId, Vec<[f32; 3]>)>,
    pub part_softness: Vec<(flint_core::EntityId, Vec<f32>)>,
    pub echo_lighten: Vec<flint_core::EntityId>,
    pub echo_entities: usize,
    pub echoes: Vec<(flint_core::EntityId, Vec<(f32, Palette)>)>,
    pub bytes: usize,
    pub read_ms: f64,
}

type Palette = Vec<[[f32; 4]; 4]>;

fn read_buffers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffers: &[(flint_core::EntityId, &wgpu::Buffer)],
) -> Vec<(flint_core::EntityId, Palette)> {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Spike Bones"),
    });
    let mut staging = Vec::new();
    for (id, b) in buffers {
        let s = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spike Bones"),
            size: b.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(b, 0, &s, 0, b.size());
        staging.push((*id, s));
    }
    queue.submit(Some(encoder.finish()));
    for (_, s) in &staging {
        s.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    }
    crate::wait_for_gpu(device);
    staging
        .into_iter()
        .map(|(id, s)| {
            let data = s.slice(..).get_mapped_range().expect("mapped");
            let m: Palette = bytemuck::cast_slice(&data).to_vec();
            (id, m)
        })
        .collect()
}

impl SceneRenderer {
    pub fn spike_skinning(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> SpikeSkinning {
        let t0 = Instant::now();
        let mut ids: Vec<_> = self
            .skinning
            .bone_buffers
            .iter()
            .map(|(k, v)| (*k, v))
            .collect();
        ids.sort_by_key(|(k, _)| k.raw());
        let bones = read_buffers(device, queue, &ids);
        let mut ids: Vec<_> = self
            .skinning
            .shadow_bone_buffers
            .iter()
            .map(|(k, v)| (*k, v))
            .collect();
        ids.sort_by_key(|(k, _)| k.raw());
        let shadow_bones = read_buffers(device, queue, &ids);
        let mut echoes = Vec::new();
        let mut echo_buffers = 0;
        let mut ents: Vec<_> = self.skinning.echoes.iter().collect();
        ents.sort_by_key(|(k, _)| k.raw());
        for (id, list) in ents {
            let bufs: Vec<_> = list.iter().map(|e| (*id, &e.bones)).collect();
            echo_buffers += bufs.len();
            let read = read_buffers(device, queue, &bufs);
            echoes.push((
                *id,
                list.iter()
                    .zip(read)
                    .map(|(e, (_, m))| (e.opacity, m))
                    .collect(),
            ));
        }
        let bytes = (bones.len() + shadow_bones.len() + echo_buffers) * crate::MAX_BONES * 64;
        SpikeSkinning {
            bones,
            shadow_bones,
            part_tints: self
                .skinning
                .part_tints
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
            part_softness: self
                .skinning
                .part_softness
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
            echo_lighten: self.skinning.echo_lighten.iter().copied().collect(),
            echo_entities: self.skinning.echoes.len(),
            echoes,
            bytes,
            read_ms: t0.elapsed().as_secs_f64() * 1000.0,
        }
    }

    pub fn spike_put_skinning(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        s: &SpikeSkinning,
    ) {
        for (id, m) in &s.bones {
            self.write_entity_bones(device, queue, *id, 0, m);
        }
        for (id, m) in &s.shadow_bones {
            self.write_entity_shadow_bones(device, queue, *id, m);
        }
        for (id, t) in &s.part_tints {
            self.skinning.part_tints.insert(*id, t.clone());
        }
        for (id, t) in &s.part_softness {
            self.skinning.part_softness.insert(*id, t.clone());
        }
        for (id, list) in &s.echoes {
            for (i, (opacity, m)) in list.iter().enumerate() {
                self.write_entity_echo_bones(device, queue, *id, i, *opacity, 0, m);
            }
        }
        for id in &s.echo_lighten {
            self.skinning.echo_lighten.insert(*id);
        }
    }
}
