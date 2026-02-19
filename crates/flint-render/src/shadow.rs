//! Cascaded Shadow Mapping
//!
//! Renders the scene from the primary directional light's perspective into
//! a depth texture array (one layer per cascade). The main shader samples
//! this texture to compute shadow factors.

use crate::primitives::{SkinnedVertex, Vertex};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Number of shadow cascades
pub const CASCADE_COUNT: usize = 3;

/// Default shadow map resolution per cascade
pub const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;

/// Uniform data for a single shadow draw call
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ShadowDrawUniforms {
    pub light_view_proj: [[f32; 4]; 4],
    pub model: [[f32; 4]; 4],
}

/// Uniform data passed to the main shader for shadow sampling
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ShadowUniforms {
    pub cascade_view_proj: [[[f32; 4]; 4]; CASCADE_COUNT],
    pub cascade_splits: [f32; 4], // 3 splits + padding
}

impl Default for ShadowUniforms {
    fn default() -> Self {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Self {
            cascade_view_proj: [identity; CASCADE_COUNT],
            cascade_splits: [0.0; 4],
        }
    }
}

/// The shadow mapping system
pub struct ShadowPass {
    pub shadow_pipeline: wgpu::RenderPipeline,
    pub shadow_bind_group_layout: wgpu::BindGroupLayout,
    pub skinned_shadow_pipeline: wgpu::RenderPipeline,
    pub skinned_shadow_bone_layout: wgpu::BindGroupLayout,
    pub shadow_texture: wgpu::Texture,
    pub shadow_view: wgpu::TextureView,
    pub cascade_views: Vec<wgpu::TextureView>,
    pub shadow_sampler: wgpu::Sampler,
    pub shadow_uniforms_buffer: wgpu::Buffer,
    pub resolution: u32,
    pub enabled: bool,
    shadow_uniforms: ShadowUniforms,
}

impl ShadowPass {
    pub fn new(
        device: &wgpu::Device,
        resolution: u32,
    ) -> Self {
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shadow Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shadow_shader.wgsl").into()),
        });

        // Bind group layout for shadow draw uniforms
        let shadow_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Shadow Draw Bind Group Layout"),
            });

        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Shadow Pipeline Layout"),
                bind_group_layouts: &[&shadow_bind_group_layout],
                push_constant_ranges: &[],
            });

        let shadow_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Shadow Depth Pipeline"),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_shadow"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: None, // Depth only, no fragment shader
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        // Skinned shadow pipeline: bind group 0 = shadow uniforms, bind group 1 = bone matrices
        let skinned_shadow_bone_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Skinned Shadow Bone Bind Group Layout"),
            });

        let skinned_shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Skinned Shadow Pipeline Layout"),
                bind_group_layouts: &[&shadow_bind_group_layout, &skinned_shadow_bone_layout],
                push_constant_ranges: &[],
            });

        let skinned_shadow_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Skinned Shadow Depth Pipeline"),
                layout: Some(&skinned_shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_skinned_shadow"),
                    buffers: &[SkinnedVertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: None,
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        // Create shadow depth texture array (one layer per cascade)
        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shadow Map Array"),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: CASCADE_COUNT as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        // Full array view for sampling in the main shader
        let shadow_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Shadow Map Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        // Per-cascade views for rendering
        let cascade_views: Vec<wgpu::TextureView> = (0..CASCADE_COUNT as u32)
            .map(|i| {
                shadow_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Shadow Cascade {} View", i)),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        // Comparison sampler for PCF
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shadow Comparison Sampler"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Shadow uniforms for the main shader (cascade matrices + splits)
        let shadow_uniforms = ShadowUniforms::default();
        let shadow_uniforms_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Shadow Uniforms Buffer"),
                contents: bytemuck::cast_slice(&[shadow_uniforms]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        Self {
            shadow_pipeline,
            shadow_bind_group_layout,
            skinned_shadow_pipeline,
            skinned_shadow_bone_layout,
            shadow_texture,
            shadow_view,
            cascade_views,
            shadow_sampler,
            shadow_uniforms_buffer,
            resolution,
            enabled: true,
            shadow_uniforms,
        }
    }

    /// Compute cascade splits and light view-projection matrices
    pub fn update_cascades(
        &mut self,
        light_dir: [f32; 3],
        camera_pos: [f32; 3],
        camera_view_proj_inv: [[f32; 4]; 4],
        near: f32,
        far: f32,
    ) {
        // Practical cascade split scheme (logarithmic/uniform blend)
        let lambda = 0.5f32; // blend factor
        let splits = compute_cascade_splits(near, far, CASCADE_COUNT, lambda);

        self.shadow_uniforms.cascade_splits = [
            splits[1],
            splits[2],
            splits[3],
            0.0,
        ];

        let light_dir_norm = normalize_3(light_dir);

        for i in 0..CASCADE_COUNT {
            let cascade_near = splits[i];
            let cascade_far = splits[i + 1];

            // Compute frustum corners for this cascade
            let corners = frustum_corners(cascade_near, cascade_far, near, far, &camera_view_proj_inv);

            // Compute tight orthographic bounds from light's perspective
            let light_vp = compute_light_matrix(&corners, &light_dir_norm, camera_pos, self.resolution);
            self.shadow_uniforms.cascade_view_proj[i] = light_vp;
        }
    }

    pub fn shadow_uniforms(&self) -> &ShadowUniforms {
        &self.shadow_uniforms
    }
}

/// Compute logarithmic/uniform blend cascade split distances
fn compute_cascade_splits(near: f32, far: f32, count: usize, lambda: f32) -> Vec<f32> {
    let mut splits = Vec::with_capacity(count + 1);
    splits.push(near);

    for i in 1..count {
        let ratio = i as f32 / count as f32;
        let log_split = near * (far / near).powf(ratio);
        let uniform_split = near + (far - near) * ratio;
        splits.push(lambda * log_split + (1.0 - lambda) * uniform_split);
    }

    splits.push(far);
    splits
}

/// Get the 8 corners of a frustum sub-section in world space
fn frustum_corners(
    cascade_near: f32,
    cascade_far: f32,
    cam_near: f32,
    cam_far: f32,
    inv_view_proj: &[[f32; 4]; 4],
) -> [[f32; 3]; 8] {
    // Perspective-correct NDC Z mapping (OpenGL convention: near → -1, far → +1)
    // The relationship between view distance d and NDC Z is hyperbolic, not linear:
    //   ndc_z = (F+N)/(F-N) - 2*F*N / ((F-N)*d)
    let depth_range = cam_far - cam_near;
    let sum_ratio = (cam_far + cam_near) / depth_range;
    let prod_term = 2.0 * cam_far * cam_near / depth_range;
    let ndc_near = sum_ratio - prod_term / cascade_near;
    let ndc_far = sum_ratio - prod_term / cascade_far;

    let ndc_corners = [
        [-1.0, -1.0, ndc_near],
        [ 1.0, -1.0, ndc_near],
        [ 1.0,  1.0, ndc_near],
        [-1.0,  1.0, ndc_near],
        [-1.0, -1.0, ndc_far],
        [ 1.0, -1.0, ndc_far],
        [ 1.0,  1.0, ndc_far],
        [-1.0,  1.0, ndc_far],
    ];

    let mut world_corners = [[0.0f32; 3]; 8];
    for (i, ndc) in ndc_corners.iter().enumerate() {
        let p = mat4_mul_point(inv_view_proj, ndc);
        world_corners[i] = p;
    }
    world_corners
}

/// Compute a tight orthographic light view-projection matrix from frustum corners.
/// Snaps the shadow origin to texel boundaries to eliminate shadow swimming/shimmering
/// caused by sub-texel shifts when the camera moves.
fn compute_light_matrix(
    corners: &[[f32; 3]; 8],
    light_dir: &[f32; 3],
    _camera_pos: [f32; 3],
    shadow_resolution: u32,
) -> [[f32; 4]; 4] {
    // Compute frustum center
    let mut center = [0.0f32; 3];
    for c in corners {
        center[0] += c[0];
        center[1] += c[1];
        center[2] += c[2];
    }
    center[0] /= 8.0;
    center[1] /= 8.0;
    center[2] /= 8.0;

    // Compute the radius that encompasses all corners
    let mut radius = 0.0f32;
    for c in corners {
        let dx = c[0] - center[0];
        let dy = c[1] - center[1];
        let dz = c[2] - center[2];
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
        if dist > radius {
            radius = dist;
        }
    }

    // Snap radius to avoid shimmering
    radius = radius.ceil();

    // Light view matrix: position eye where the light source is (along light direction)
    let eye = [
        center[0] + light_dir[0] * radius,
        center[1] + light_dir[1] * radius,
        center[2] + light_dir[2] * radius,
    ];

    let view = look_at(&eye, &center);
    let proj = ortho(-radius, radius, -radius, radius, 0.0, 2.0 * radius);

    let mut light_matrix = mat4_mul(&proj, &view);

    // Snap shadow origin to texel boundaries to prevent shadow swimming.
    // As the camera moves, the frustum center shifts continuously, causing
    // the shadow map texels to land on slightly different world positions
    // each frame. By rounding the projected origin to the nearest texel,
    // we quantize the shadow map position so it only moves in whole-texel
    // increments.
    let half_res = shadow_resolution as f32 * 0.5;
    // Column-major: m[col][row]. Column 3 holds the translation-like terms.
    let texel_x = light_matrix[3][0] * half_res;
    let texel_y = light_matrix[3][1] * half_res;
    light_matrix[3][0] += (texel_x.round() - texel_x) / half_res;
    light_matrix[3][1] += (texel_y.round() - texel_y) / half_res;

    light_matrix
}

fn normalize_3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-10 {
        return [0.0, -1.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

fn cross(a: &[f32; 3], b: &[f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Look-at view matrix (column-major for wgpu)
fn look_at(eye: &[f32; 3], target: &[f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize_3([
        target[0] - eye[0],
        target[1] - eye[1],
        target[2] - eye[2],
    ]);
    let up = [0.0, 1.0, 0.0];
    let s = normalize_3(cross(&f, &up));
    let u = cross(&s, &f);

    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(&s, eye), -dot(&u, eye), dot(&f, eye), 1.0],
    ]
}

/// Orthographic projection matrix (column-major)
fn ortho(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let rml = right - left;
    let tmb = top - bottom;
    let fmn = far - near;

    [
        [2.0 / rml, 0.0, 0.0, 0.0],
        [0.0, 2.0 / tmb, 0.0, 0.0],
        [0.0, 0.0, -1.0 / fmn, 0.0],
        [
            -(right + left) / rml,
            -(top + bottom) / tmb,
            -near / fmn,
            1.0,
        ],
    ]
}

fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    out
}

fn mat4_mul_point(m: &[[f32; 4]; 4], p: &[f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 4];
    for row in 0..4 {
        out[row] = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row] * 1.0;
    }
    let w = out[3];
    if w.abs() < 1e-10 {
        return [out[0], out[1], out[2]];
    }
    [out[0] / w, out[1] / w, out[2] / w]
}
