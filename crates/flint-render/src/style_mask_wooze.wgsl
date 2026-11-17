// The style mask's vertex stages for a woozy draw (#737), appended to the
// woozy module only (style_mask.rs, `mask_source`).
//
// Shallow wooze (#215) moves a mesh's vertices in the main and shadow
// passes' own vertex stages (wooze_vertex.wgsl). The mask tests each of its
// fragments against the scene's depth, so it has to put the vertices where
// the scene drew them: these repeat wooze_vertex.wgsl's products in the
// same order, and the drift rides in the same transform slot. Masked at its
// rest pose, a drifting mesh had holes where it had moved off and spill
// where it had moved from.

@vertex
fn vs_static_wooze(in: StaticVertex) -> @builtin(position) vec4<f32> {
    let rest = transform.model * vec4<f32>(in.position, 1.0);
    let world_pos = rest.xyz + wooze_offset(rest.xyz, transform.wooze);
    return transform.view_proj * vec4<f32>(world_pos, 1.0);
}

@vertex
fn vs_skinned_wooze(in: SkinnedVertex) -> @builtin(position) vec4<f32> {
    let skin_mat = bone_matrices[in.joint_indices.x] * in.joint_weights.x
                 + bone_matrices[in.joint_indices.y] * in.joint_weights.y
                 + bone_matrices[in.joint_indices.z] * in.joint_weights.z
                 + bone_matrices[in.joint_indices.w] * in.joint_weights.w;
    let skinned_pos = skin_mat * vec4<f32>(in.position, 1.0);
    let rest = transform.model * skinned_pos;
    let world_pos = rest.xyz + wooze_offset(rest.xyz, transform.wooze);
    return transform.view_proj * vec4<f32>(world_pos, 1.0);
}
