// The style mask's vertex stages for a woozy draw (#737), appended to the
// woozy module only (style_mask.rs, `mask_source`).
//
// Shallow wooze (#215) moves a mesh's vertices in the main and shadow
// passes' own vertex stages (wooze_vertex.wgsl). The mask tests each of its
// fragments against the scene's depth, so it has to put the vertices where
// the scene drew them: these call wooze_vertex.wgsl's products
// (mesh_vertex.wgsl's `world_of`, `drifted` and `clip_of`, #1244), and the
// drift rides in the same transform slot. Masked at its rest pose, a
// drifting mesh had holes where it had moved off and spill where it had
// moved from.

@vertex
fn vs_static_wooze(in: StaticVertex) -> @builtin(position) vec4<f32> {
    return clip_of(drifted(world_of(vec4<f32>(in.position, 1.0))));
}

@vertex
fn vs_skinned_wooze(in: SkinnedVertex) -> @builtin(position) vec4<f32> {
    let skin_mat = skin_transform(in.joint_indices, in.joint_weights);
    return clip_of(drifted(world_of(skin_mat * vec4<f32>(in.position, 1.0))));
}
