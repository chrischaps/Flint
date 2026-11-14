//! The one place Flint's own WGSL becomes a shader module (#944, #945).
//!
//! Every built-in `create_shader_module` (the renderer's pipelines, the
//! style kit, the player's world text, the CLI's windows) calls
//! [`TrustedShader::create_trusted_shader_module`] instead. It is
//! `create_shader_module_trusted` with `force_loop_bounding: false` and every
//! other runtime check on (bounds, integer division, ray queries, mesh
//! shaders).
//!
//! Why: since wgpu 25, naga bounds every loop it writes to HLSL and SPIR-V,
//! so that a loop that never ends can't be treated as unreachable by the
//! driver's compiler. On Flint's loop-heavy styles that cost a lot of GPU
//! time (spike #934, Vulkan, 1280x720: ASCII 2.6x slower, Kuwahara and
//! pencil about 25%). Flint's built-in loops all end; #945's PR lists each
//! one and why. **A new loop in a built-in shader must end on every input**
//! (a constant or clamped bound): with bounding off, one that doesn't can
//! hang the GPU.
//!
//! User styles (`stylize/custom.rs`) never come here: their WGSL is a game's
//! or a player's, so it keeps every check wgpu has, and Flint guards their
//! loops itself (#411). `kit::shader` is public, so a game's own Rust style
//! built on the kit is trusted code too.
//!
//! `FLINT_SHADER_CHECKS=all` puts loop bounding back on for the built-in
//! modules: the A/B against wgpu's default, and a first test when a GPU
//! hangs. Unset, or any other value, is the default above.
//!
//! Clippy's `disallowed_methods` (the workspace `clippy.toml`, denied in each
//! crate that links wgpu) refuses a bare `create_shader_module` or
//! `create_shader_module_trusted` anywhere else; the allowed calls are this
//! one and the user-style path.

use std::sync::OnceLock;

/// `create_shader_module` for WGSL that ships inside Flint.
pub trait TrustedShader {
    /// A shader module from built-in WGSL. Never for user style text.
    fn create_trusted_shader_module(
        &self,
        desc: wgpu::ShaderModuleDescriptor<'_>,
    ) -> wgpu::ShaderModule;
}

/// The runtime checks a built-in module keeps: all of them but loop
/// bounding, unless `FLINT_SHADER_CHECKS=all`.
pub(crate) fn built_in_checks() -> wgpu::ShaderRuntimeChecks {
    static CHECKS: OnceLock<wgpu::ShaderRuntimeChecks> = OnceLock::new();
    *CHECKS.get_or_init(|| {
        let mut checks = wgpu::ShaderRuntimeChecks::checked();
        if std::env::var("FLINT_SHADER_CHECKS").as_deref() != Ok("all") {
            checks.force_loop_bounding = false;
        }
        checks
    })
}

impl TrustedShader for wgpu::Device {
    #[allow(clippy::disallowed_methods)] // the one sanctioned call
    fn create_trusted_shader_module(
        &self,
        desc: wgpu::ShaderModuleDescriptor<'_>,
    ) -> wgpu::ShaderModule {
        // SAFETY: `create_shader_module_trusted` asks that a module with
        // loop bounding off has no loop that never ends. This takes only
        // Flint's own WGSL, whose loops #945 audited (each has a constant or
        // clamped bound); bounds and division checks stay on.
        unsafe { self.create_shader_module_trusted(desc, built_in_checks()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_keep_every_check_but_loop_bounding() {
        let c = built_in_checks();
        let loop_bounding = std::env::var("FLINT_SHADER_CHECKS").as_deref() == Ok("all");
        assert_eq!(c.force_loop_bounding, loop_bounding);
        assert!(c.bounds_checks, "bounds checks stay on");
        assert!(c.int_div_checks, "division checks stay on");
        assert!(c.ray_query_initialization_tracking);
        assert!(c.task_shader_dispatch_tracking);
        assert!(c.mesh_shader_primitive_indices_clamp);
    }
}
