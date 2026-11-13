### Changing MSAA at run time no longer breaks woozy meshes, lighten echoes or soft edges (#735)
**What changed.** Turning MSAA on or off while a scene runs (`--quality msaa=4`,
the web's `?quality=`, a script's `set_quality`) used to leave four pipelines
at the old sample count: shallow wooze's (`shallow_wooze`, plain and skinned),
a living prop's `echo_blend = "lighten"` echo, and a soft-edged skinned part
(`set_entity_part_softness`). The next frame was a wgpu validation error, a
panic natively. Every scene pipeline is now rebuilt together, and frames are
otherwise unchanged. For code that builds on `flint_render` directly:
`RenderPipeline::new` takes a `SceneLayouts`, which now holds the three bind
group layouts and the material sampler, and `ParticlePipeline`'s buffers moved
to `ParticleBuffers`.
**What to do.** Nothing, unless you call those two types yourself: then make a
`SceneLayouts::new(device)` once and pass it in.
