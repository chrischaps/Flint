// A skinned vertex's pose (#1244): its four bones' matrices, each weighted.
// Written once for every pass that poses a skinned mesh (the scene, its
// shadow, the style and selection masks, the wireframe, the woozy stages),
// so a pose drawn twice is posed the same way twice. Each shader declares
// `bone_matrices` itself, at the group its layout gives the bones.

fn skin_transform(joints: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    return bone_matrices[joints.x] * weights.x
         + bone_matrices[joints.y] * weights.y
         + bone_matrices[joints.z] * weights.z
         + bone_matrices[joints.w] * weights.w;
}
