//! Taking the next swapchain image (#944).
//!
//! Every window (the player, the web shims, the viewer, the CLI's editors
//! and previews) takes its frame through [`AcquireFrame::acquire`], never
//! `Surface::get_current_texture` (clippy's `disallowed_methods` holds it).
//! Since wgpu 29 that call returns a `CurrentSurfaceTexture` enum rather
//! than wgpu 23's `Result<_, SurfaceError>` (#945); this module turns it back
//! into a `Result`, so the callers match on this module's [`SurfaceError`].
//!
//! A suboptimal frame (`CurrentSurfaceTexture::Suboptimal`) counts as a
//! frame, as `SurfaceTexture::suboptimal` did on wgpu 23.

/// Why no frame could be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceError {
    /// Acquiring the frame timed out.
    Timeout,
    /// The window is hidden or minimised.
    Occluded,
    /// The surface changed (a resize): reconfigure it.
    Outdated,
    /// The swapchain is gone: reconfigure it.
    Lost,
    /// No memory for a frame. Never since wgpu 29, which folds it into a
    /// lost device; kept so the callers' fatal arm stays until #948.
    OutOfMemory,
    /// A validation error, caught by an error scope.
    Validation,
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Timeout => "timed out acquiring the next frame",
            Self::Occluded => "the surface is occluded",
            Self::Outdated => "the surface is outdated and must be reconfigured",
            Self::Lost => "the swapchain was lost and must be recreated",
            Self::OutOfMemory => "out of memory acquiring the next frame",
            Self::Validation => "a validation error acquiring the next frame",
        })
    }
}

impl std::error::Error for SurfaceError {}

/// The next swapchain image, or why there is none.
pub trait AcquireFrame {
    fn acquire(&self) -> Result<wgpu::SurfaceTexture, SurfaceError>;
}

impl AcquireFrame for wgpu::Surface<'_> {
    #[allow(clippy::disallowed_methods)] // the one sanctioned call
    fn acquire(&self) -> Result<wgpu::SurfaceTexture, SurfaceError> {
        use wgpu::CurrentSurfaceTexture as Current;
        match self.get_current_texture() {
            Current::Success(frame) | Current::Suboptimal(frame) => Ok(frame),
            Current::Timeout => Err(SurfaceError::Timeout),
            Current::Occluded => Err(SurfaceError::Occluded),
            Current::Outdated => Err(SurfaceError::Outdated),
            Current::Lost => Err(SurfaceError::Lost),
            Current::Validation => Err(SurfaceError::Validation),
        }
    }
}
