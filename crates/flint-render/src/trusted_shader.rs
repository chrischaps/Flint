//! Shader modules without some of wgpu's runtime checks (#934 spike).
//!
//! Since wgpu 25, naga's HLSL and SPIR-V backends bound every loop against
//! infinite-loop UB, and wgpu 30 guards integer division too. Both cost GPU
//! time on Flint's loop-heavy styles (ASCII, Kuwahara, pencil). Flint's own
//! shaders are trusted code. `FLINT_SHADER_CHECKS` picks what the built-in
//! modules keep, for the A/B: `all` (wgpu's default), `noloop` (no loop
//! bounding; the default here), `none` (also no bounds or division checks).

/// `create_shader_module` with the checks `FLINT_SHADER_CHECKS` names.
pub(crate) trait TrustedShader {
    fn create_module(&self, desc: wgpu::ShaderModuleDescriptor<'_>) -> wgpu::ShaderModule;
}

pub(crate) fn checks() -> wgpu::ShaderRuntimeChecks {
    let mut c = wgpu::ShaderRuntimeChecks::checked();
    match std::env::var("FLINT_SHADER_CHECKS").as_deref() {
        Ok("all") => {}
        Ok("none") => {
            c.bounds_checks = false;
            c.force_loop_bounding = false;
            c.int_div_checks = false;
        }
        Ok("nodiv") => c.int_div_checks = false,
        _ => c.force_loop_bounding = false,
    }
    c
}

impl TrustedShader for wgpu::Device {
    fn create_module(&self, desc: wgpu::ShaderModuleDescriptor<'_>) -> wgpu::ShaderModule {
        // SAFETY: Flint's built-in WGSL; its loops all end (the spike's
        // claim to check before any real use).
        unsafe { self.create_shader_module_trusted(desc, checks()) }
    }
}
