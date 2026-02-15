// Particle shader — GPU-instanced camera-facing quads with per-particle color, size, rotation
// Reads instance data from a storage buffer for efficient rendering of thousands of particles

struct ParticleUniforms {
    view_proj: mat4x4<f32>,
    camera_right: vec3<f32>,
    _pad0: f32,
    camera_up: vec3<f32>,
    _pad1: f32,
};

struct ParticleInstance {
    pos_size: vec4<f32>,    // xyz = world position, w = size
    color: vec4<f32>,       // rgba tint
    rotation_frame: vec4<f32>, // x = rotation radians, y = frame index, z/w = unused
};

@group(0) @binding(0)
var<uniform> uniforms: ParticleUniforms;

@group(1) @binding(0)
var<storage, read> instances: array<ParticleInstance>;

@group(2) @binding(0)
var particle_texture: texture_2d<f32>;
@group(2) @binding(1)
var particle_sampler: sampler;

// Sprite sheet dimensions passed as push constant would be ideal,
// but for simplicity we store frames_x and frames_y in the unused z/w of rotation_frame
// Actually, we'll just use frames_x = rotation_frame.z, frames_y = rotation_frame.w
// (Redefine: rotation_frame = { rotation, frame_index, frames_x, frames_y })

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_particle(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let inst = instances[instance_index];
    let world_pos = inst.pos_size.xyz;
    let size = inst.pos_size.w;
    let rotation = inst.rotation_frame.x;
    let frame = inst.rotation_frame.y;
    let frames_x = inst.rotation_frame.z;
    let frames_y = inst.rotation_frame.w;

    // Unit quad corners: BL(0), BR(1), TL(2), TR(3)
    var local_x: f32;
    var local_y: f32;
    var u: f32;
    var v: f32;

    switch vertex_index {
        case 0u: {
            local_x = -0.5;
            local_y = -0.5;
            u = 0.0;
            v = 1.0;
        }
        case 1u: {
            local_x = 0.5;
            local_y = -0.5;
            u = 1.0;
            v = 1.0;
        }
        case 2u: {
            local_x = -0.5;
            local_y = 0.5;
            u = 0.0;
            v = 0.0;
        }
        case 3u: {
            local_x = 0.5;
            local_y = 0.5;
            u = 1.0;
            v = 0.0;
        }
        default: {
            local_x = 0.0;
            local_y = 0.0;
            u = 0.0;
            v = 0.0;
        }
    }

    // Apply per-particle rotation around the camera-facing normal
    let cos_r = cos(rotation);
    let sin_r = sin(rotation);
    let rotated_x = local_x * cos_r - local_y * sin_r;
    let rotated_y = local_x * sin_r + local_y * cos_r;

    // Sprite sheet UV calculation
    let fx = max(frames_x, 1.0);
    let fy = max(frames_y, 1.0);
    let frame_i = u32(frame);
    let frame_col = f32(frame_i % u32(fx));
    let frame_row = f32(frame_i / u32(fx));
    let cell_w = 1.0 / fx;
    let cell_h = 1.0 / fy;
    let final_u = (frame_col + u) * cell_w;
    let final_v = (frame_row + v) * cell_h;

    // Billboard in world space
    let world = world_pos
        + uniforms.camera_right * rotated_x * size
        + uniforms.camera_up * rotated_y * size;

    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(final_u, final_v);
    out.color = inst.color;
    return out;
}

@fragment
fn fs_particle(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(particle_texture, particle_sampler, in.uv);
    return tex_color * in.color;
}
