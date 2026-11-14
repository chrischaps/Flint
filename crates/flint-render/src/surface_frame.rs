//! Taking the next swapchain image (#944).
//!
//! Every window (the player, the web shims, the viewer, the CLI's editors
//! and previews) takes its frame through [`AcquireFrame::acquire`], never
//! `Surface::get_current_texture` (clippy's `disallowed_methods` holds it).
//! wgpu 29 turns that call's `Result<_, SurfaceError>` into a
//! `CurrentSurfaceTexture` enum; when Flint moves to it (#945) only this
//! body changes, and the callers keep matching on this module's
//! [`SurfaceError`].
//!
//! A suboptimal frame counts as a frame, as it does on wgpu 23
//! (`SurfaceTexture::suboptimal`) and as `CurrentSurfaceTexture::Suboptimal`
//! will.

/// Why no frame could be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceError {
    /// Acquiring the frame timed out.
    Timeout,
    /// The window is hidden or minimised. wgpu 29+; never on wgpu 23.
    Occluded,
    /// The surface changed (a resize): reconfigure it.
    Outdated,
    /// The swapchain is gone: reconfigure it.
    Lost,
    /// No memory for a frame. wgpu 23 only; wgpu 29 folds it into a lost device.
    OutOfMemory,
    /// A validation error. wgpu 29+; never on wgpu 23.
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
    fn acquire(&self) -> Result<wgpu::SurfaceTexture, SurfaceError> {
        #[allow(clippy::disallowed_methods)]
        self.get_current_texture().map_err(|e| match e {
            wgpu::SurfaceError::Timeout => SurfaceError::Timeout,
            wgpu::SurfaceError::Outdated => SurfaceError::Outdated,
            wgpu::SurfaceError::Lost => SurfaceError::Lost,
            wgpu::SurfaceError::OutOfMemory => SurfaceError::OutOfMemory,
        })
    }
}
