// Bind group 2, the light group, as every lit shader binds it (#738): the
// lights, the shadow map array (the sun's cascades in layers 0-2, the casting
// spot's map in layer 3) with its comparison sampler, and the shadow
// uniforms. `scene_renderer/lights.rs` builds the group the scene's shaders
// take; the volumetric pass builds its own over the same slots. Needs
// `lights.wgsl`. The PBR surface adds `material_sampler` at binding 4.

@group(2) @binding(0)
var<uniform> lights: LightUniforms;

@group(2) @binding(1)
var shadow_maps: texture_depth_2d_array;
@group(2) @binding(2)
var shadow_sampler: sampler_comparison;

@group(2) @binding(3)
var<uniform> shadow: ShadowUniforms;
