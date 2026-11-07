//! [`Previewer`]: how a caller lets [`crate::AnthropicAgent`] see what a spec
//! builds.
//!
//! This crate is tool-time and depends on `flint-procgen` only, so it can
//! show Claude a texture (the generator's pixels are the asset) but cannot
//! draw a mesh. A caller that can render, such as `flint-cli` with the
//! headless model renderer, implements this trait and hands it to
//! [`crate::AnthropicAgent::with_previewer`].

use flint_procgen::ProcGenSpec;

/// Renders a spec to an encoded image (PNG or JPEG) for the model to look at
/// and for the refine loop to measure against the reference.
///
/// Return `None` when the spec can't be previewed (a generator this
/// previewer doesn't draw, a GPU that isn't there). The agent then falls
/// back to what it can show by itself, which for a mesh is nothing.
///
/// Every closure `Fn(&ProcGenSpec) -> Option<Vec<u8>>` is a previewer.
pub trait Previewer: Send + Sync {
    /// The image `spec` builds, encoded.
    fn preview(&self, spec: &ProcGenSpec) -> Option<Vec<u8>>;
}

impl<F> Previewer for F
where
    F: Fn(&ProcGenSpec) -> Option<Vec<u8>> + Send + Sync,
{
    fn preview(&self, spec: &ProcGenSpec) -> Option<Vec<u8>> {
        self(spec)
    }
}
