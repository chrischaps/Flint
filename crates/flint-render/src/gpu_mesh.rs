//! GPU mesh cache — uploads imported meshes to GPU buffers

use crate::primitives::{SkinnedVertex, Vertex};
use crate::skinned_pipeline::MAX_BONES;
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

/// A GPU-resident skinned mesh with bone buffer for skeletal animation
pub struct GpuSkinnedMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub material: ImportedMaterial,
    pub bone_buffer: wgpu::Buffer,
    pub bone_bind_group: wgpu::BindGroup,
    pub skin_index: usize,
    vertex_data: Vec<u8>,
    index_data: Vec<u8>,
}

impl GpuSkinnedMesh {
    pub fn create_vertex_buffer_copy(&self, device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Vertex Buffer Copy"),
            contents: &self.vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        })
    }

    pub fn create_index_buffer_copy(&self, device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Index Buffer Copy"),
            contents: &self.index_data,
            usage: wgpu::BufferUsages::INDEX,
        })
    }
}

/// Cache of imported meshes uploaded to the GPU, keyed by asset name
#[derive(Default)]
pub struct MeshCache {
    meshes: HashMap<String, Vec<GpuMesh>>,
    skinned_meshes: HashMap<String, Vec<GpuSkinnedMesh>>,
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
            use_vertex_color: false,
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

    /// Upload a subset of an imported model's meshes to the GPU, identified by mesh indices.
    /// Used to upload individual node's primitives under a node-specific cache key.
    /// If `bake_transform` is provided, vertex positions and normals are transformed
    /// by the given 4x4 column-major matrix before upload (used to flatten hierarchies
    /// with non-uniform scale during multi-node GLB expansion).
    pub fn upload_mesh_subset(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        import_result: &ImportResult,
        mesh_indices: &[usize],
        default_color: [f32; 4],
        bake_transform: Option<&[[f32; 4]; 4]>,
    ) {
        // Compute the normal matrix (inverse-transpose of upper-left 3x3) if baking
        let normal_mat = bake_transform.map(|m| {
            let a = m[0][0]; let b = m[1][0]; let c = m[2][0];
            let d = m[0][1]; let e = m[1][1]; let f = m[2][1];
            let g = m[0][2]; let h = m[1][2]; let i = m[2][2];
            let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
            let inv_det = if det.abs() > 1e-10 { 1.0 / det } else { 1.0 };
            // Cofactor matrix rows (= inverse-transpose columns)
            [
                [(e * i - f * h) * inv_det, (f * g - d * i) * inv_det, (d * h - e * g) * inv_det],
                [(c * h - b * i) * inv_det, (a * i - c * g) * inv_det, (b * g - a * h) * inv_det],
                [(b * f - c * e) * inv_det, (c * d - a * f) * inv_det, (a * e - b * d) * inv_det],
            ]
        });

        let default_material = ImportedMaterial {
            name: "default".to_string(),
            base_color: default_color,
            metallic: 0.0,
            roughness: 0.5,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            use_vertex_color: false,
        };

        let gpu_meshes: Vec<GpuMesh> = mesh_indices
            .iter()
            .filter_map(|&idx| import_result.meshes.get(idx))
            .map(|mesh| {
                let material = mesh
                    .material_index
                    .and_then(|i| import_result.materials.get(i))
                    .cloned()
                    .unwrap_or_else(|| default_material.clone());

                let vertex_count = mesh.positions.len();
                let vertices: Vec<Vertex> = (0..vertex_count)
                    .map(|i| {
                        let mut position = mesh.positions[i];
                        let mut normal = if i < mesh.normals.len() {
                            mesh.normals[i]
                        } else {
                            [0.0, 1.0, 0.0]
                        };
                        let uv = if i < mesh.uvs.len() {
                            mesh.uvs[i]
                        } else {
                            [0.0, 0.0]
                        };

                        // Bake transform into vertex data if provided
                        if let Some(m) = bake_transform {
                            let [px, py, pz] = position;
                            position = [
                                m[0][0] * px + m[1][0] * py + m[2][0] * pz + m[3][0],
                                m[0][1] * px + m[1][1] * py + m[2][1] * pz + m[3][1],
                                m[0][2] * px + m[1][2] * py + m[2][2] * pz + m[3][2],
                            ];
                        }
                        if let Some(nm) = &normal_mat {
                            let [nx, ny, nz] = normal;
                            let tnx = nm[0][0] * nx + nm[1][0] * ny + nm[2][0] * nz;
                            let tny = nm[0][1] * nx + nm[1][1] * ny + nm[2][1] * nz;
                            let tnz = nm[0][2] * nx + nm[1][2] * ny + nm[2][2] * nz;
                            let len = (tnx * tnx + tny * tny + tnz * tnz).sqrt();
                            if len > 1e-6 {
                                normal = [tnx / len, tny / len, tnz / len];
                            }
                        }

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
        self.meshes.contains_key(name) || self.skinned_meshes.contains_key(name)
    }

    /// Upload skinned meshes from an imported model to the GPU
    pub fn upload_skinned(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        import_result: &ImportResult,
        default_color: [f32; 4],
        bone_bind_group_layout: &wgpu::BindGroupLayout,
    ) {
        let default_material = ImportedMaterial {
            name: "default".to_string(),
            base_color: default_color,
            metallic: 0.0,
            roughness: 0.5,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            use_vertex_color: false,
        };

        let identity_bones: Vec<[[f32; 4]; 4]> = vec![
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ];
            MAX_BONES
        ];

        let gpu_skinned: Vec<GpuSkinnedMesh> = import_result
            .meshes
            .iter()
            .filter(|mesh| mesh.joint_indices.is_some() && mesh.joint_weights.is_some())
            .map(|mesh| {
                let material = mesh
                    .material_index
                    .and_then(|i| import_result.materials.get(i))
                    .cloned()
                    .unwrap_or_else(|| default_material.clone());

                let skin_index = mesh.skin_index.unwrap_or(0);
                let vertex_count = mesh.positions.len();
                let joints = mesh.joint_indices.as_ref().unwrap();
                let weights = mesh.joint_weights.as_ref().unwrap();

                let vertices: Vec<SkinnedVertex> = (0..vertex_count)
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
                        let ji = if i < joints.len() {
                            [
                                joints[i][0] as u32,
                                joints[i][1] as u32,
                                joints[i][2] as u32,
                                joints[i][3] as u32,
                            ]
                        } else {
                            [0, 0, 0, 0]
                        };
                        let jw = if i < weights.len() {
                            weights[i]
                        } else {
                            [1.0, 0.0, 0.0, 0.0]
                        };

                        SkinnedVertex {
                            position,
                            normal,
                            color: material.base_color,
                            uv,
                            joint_indices: ji,
                            joint_weights: jw,
                        }
                    })
                    .collect();

                let vertex_data = bytemuck::cast_slice(&vertices).to_vec();
                let index_data = bytemuck::cast_slice(&mesh.indices).to_vec();

                let vertex_buffer =
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("{} Skinned Vertex Buffer", name)),
                        contents: &vertex_data,
                        usage: wgpu::BufferUsages::VERTEX,
                    });

                let index_buffer =
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("{} Skinned Index Buffer", name)),
                        contents: &index_data,
                        usage: wgpu::BufferUsages::INDEX,
                    });

                // Create bone storage buffer initialized to identity
                let bone_buffer =
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("{} Bone Buffer", name)),
                        contents: bytemuck::cast_slice(&identity_bones),
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    });

                let bone_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: bone_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: bone_buffer.as_entire_binding(),
                    }],
                    label: Some(&format!("{} Bone Bind Group", name)),
                });

                GpuSkinnedMesh {
                    vertex_buffer,
                    index_buffer,
                    index_count: mesh.indices.len() as u32,
                    material,
                    bone_buffer,
                    bone_bind_group,
                    skin_index,
                    vertex_data,
                    index_data,
                }
            })
            .collect();

        if !gpu_skinned.is_empty() {
            self.skinned_meshes.insert(name.to_string(), gpu_skinned);
        }
    }

    /// Get cached skinned GPU meshes by asset name
    pub fn get_skinned(&self, name: &str) -> Option<&Vec<GpuSkinnedMesh>> {
        self.skinned_meshes.get(name)
    }

    /// Get mutable reference to skinned GPU meshes (for bone buffer updates)
    pub fn get_skinned_mut(&mut self, name: &str) -> Option<&mut Vec<GpuSkinnedMesh>> {
        self.skinned_meshes.get_mut(name)
    }

    /// Upload a procedural mesh from raw vertex/index data to the GPU
    pub fn upload_procedural(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        vertices: &[Vertex],
        indices: &[u32],
        material: ImportedMaterial,
    ) {
        let vertex_data = bytemuck::cast_slice(vertices).to_vec();
        let index_data = bytemuck::cast_slice(indices).to_vec();

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} Procedural Vertex Buffer", name)),
            contents: &vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} Procedural Index Buffer", name)),
            contents: &index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        let gpu_mesh = GpuMesh {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            material,
            vertex_data,
            index_data,
        };

        self.meshes.insert(name.to_string(), vec![gpu_mesh]);
    }

    /// Check if a model has skinned meshes cached
    pub fn contains_skinned(&self, name: &str) -> bool {
        self.skinned_meshes.contains_key(name)
    }
}
