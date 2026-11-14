//! The one place Flint's own WGSL becomes a shader module (#944).
//!
//! Every built-in `create_shader_module` (the renderer's pipelines, the
//! style kit, the player's world text, the CLI's windows) calls
//! [`TrustedShader::create_trusted_shader_module`] instead. On wgpu 23 it is
//! the plain call. The wgpu 30 bump (#945) makes it
//! `create_shader_module_trusted` with `force_loop_bounding: false`: naga 25+
//! bounds every loop in HLSL and SPIR-V, which made the ASCII style 2.6x
//! slower on the GPU in spike #934, and Flint's built-in loops all end.
//!
//! User styles (`stylize/custom.rs`) never come here: their WGSL is a game's
//! or a player's, so it keeps every check wgpu has. Clippy's
//! `disallowed_methods` (the workspace `clippy.toml`, denied in each crate
//! that links wgpu) refuses a bare `create_shader_module` anywhere else; the
//! two allowed calls are this one and the user-style path.

/// `create_shader_module` for WGSL that ships inside Flint.
pub trait TrustedShader {
    /// A shader module from built-in WGSL. Never for user style text.
    fn create_trusted_shader_module(
        &self,
        desc: wgpu::ShaderModuleDescriptor<'_>,
    ) -> wgpu::ShaderModule;
}

impl TrustedShader for wgpu::Device {
    fn create_trusted_shader_module(
        &self,
        desc: wgpu::ShaderModuleDescriptor<'_>,
    ) -> wgpu::ShaderModule {
        // The one sanctioned call. #945 swaps it for
        // `create_shader_module_trusted(desc, checks)` with loop bounding off.
        #[allow(clippy::disallowed_methods)]
        self.create_shader_module(desc)
    }
}
