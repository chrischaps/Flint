//! GPU mesh cache — uploads imported meshes to GPU buffers

use crate::primitives::Vertex;
use flint_import::{ImportResult, ImportedMaterial};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

/// A single GPU-resident mesh primitive with its material data
pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub material: ImportedMaterial,
    // Keep raw data for creating per-entity copies
    vertex_data: Vec<u8>,
    index_data: Vec<u8>,
}

impl GpuMesh {
    /// Create a copy of the vertex buffer for a new draw call
    pub fn create_vertex_buffer_copy(&self, device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Imported Vertex Buffer Copy"),
            contents: &self.vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        })
    }

    /// Create a copy of the index buffer for a new draw call
    pub fn create_index_buffer_copy(&self, device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Imported Index Buffer Copy"),
            contents: &self.index_data,
            usage: wgpu::BufferUsages::INDEX,
        })
    }

    /// Reinterpret the raw index data as a slice of u32 triangle indices
    pub fn triangle_indices(&self) -> Vec<u32> {
        bytemuck::cast_slice::<u8, u32>(&self.index_data).to_vec()
    }

    /// Reinterpret the raw vertex data as a slice of Vertex structs
    pub fn vertices(&self) -> Vec<Vertex> {
        bytemuck::cast_slice::<u8, Vertex>(&self.vertex_data).to_vec()
    }
}

/// Cache of imported meshes uploaded to the GPU, keyed by asset name
#[derive(Default)]
pub struct MeshCache {
    meshes: HashMap<String, Vec<GpuMesh>>,
}

impl MeshCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upload an imported model's meshes to the GPU
    pub fn upload_imported(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        import_result: &ImportResult,
        default_color: [f32; 4],
    ) {
        let default_material = ImportedMaterial {
            name: "default".to_string(),
            base_color: default_color,
            metallic: 0.0,
            roughness: 0.5,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
        };

        let gpu_meshes: Vec<GpuMesh> = import_result
            .meshes
            .iter()
            .map(|mesh| {
                let material = mesh
                    .material_index
                    .and_then(|i| import_result.materials.get(i))
                    .cloned()
                    .unwrap_or_else(|| default_material.clone());

                let vertex_count = mesh.positions.len();
                let vertices: Vec<Vertex> = (0..vertex_count)
                    .map(|i| {
                        let position = mesh.positions[i];
                        let normal = if i < mesh.normals.len() {
                            mesh.normals[i]
                        } else {
                            [0.0, 1.0, 0.0]
                        };
                        let uv = if i < mesh.uvs.len() {
                            mesh.uvs[i]
                        } else {
                            [0.0, 0.0]
                        };

                        Vertex {
                            position,
                            normal,
                            color: material.base_color,
                            uv,
                        }
                    })
                    .collect();

                let vertex_data = bytemuck::cast_slice(&vertices).to_vec();
                let index_data = bytemuck::cast_slice(&mesh.indices).to_vec();

                let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{} Vertex Buffer", name)),
                    contents: &vertex_data,
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{} Index Buffer", name)),
                    contents: &index_data,
                    usage: wgpu::BufferUsages::INDEX,
                });

                GpuMesh {
                    vertex_buffer,
                    index_buffer,
                    index_count: mesh.indices.len() as u32,
                    material,
                    vertex_data,
                    index_data,
                }
            })
            .collect();

        self.meshes.insert(name.to_string(), gpu_meshes);
    }

    /// Get cached GPU meshes by asset name
    pub fn get(&self, name: &str) -> Option<&Vec<GpuMesh>> {
        self.meshes.get(name)
    }

    /// Check if a model is already cached
    pub fn contains(&self, name: &str) -> bool {
        self.meshes.contains_key(name)
    }
}
