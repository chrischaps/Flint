// Flint PBR mesh shader: the vertex stage of a static mesh, over the shared
// surface (pbr_surface.wgsl, with lights.wgsl, light_bindings.wgsl,
// brdf.wgsl, shadows.wgsl, direct_light.wgsl, tonemap.wgsl and
// mesh_vertex.wgsl's products in front of it; `wgsl_sources::SCENE`).
// Cook-Torrance BRDF, metallic-roughness workflow.

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let world_pos = world_of(vec4<f32>(in.position, 1.0));
    out.clip_position = clip_of(world_pos);
    out.color = in.color;
    out.normal = world_normal_of(in.normal);
    out.world_pos = world_pos.xyz;
    out.uv = in.uv;

    return out;
}

@fragment
fn fs_main(input: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade(facing(input, front));
}
