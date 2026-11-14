//! Taking the next swapchain image (#934 spike).
//!
//! wgpu 29 made `Surface::get_current_texture` return a
//! [`wgpu::CurrentSurfaceTexture`] instead of `Result<_, SurfaceError>`.
//! [`AcquireFrame::acquire`] gives the callers back the `Result` shape they
//! match on, with a suboptimal frame counted as a frame.

/// Why no frame could be taken: wgpu 23's `SurfaceError`, less the
/// out-of-memory case wgpu 29 folded into a lost device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceError {
    Timeout,
    Occluded,
    Outdated,
    Lost,
    /// Never returned since wgpu 29; kept so the old match arms compile.
    OutOfMemory,
    Validation,
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for SurfaceError {}

/// [`wgpu::Surface::get_current_texture`] as a `Result`.
pub trait AcquireFrame {
    fn acquire(&self) -> Result<wgpu::SurfaceTexture, SurfaceError>;
}

impl AcquireFrame for wgpu::Surface<'_> {
    fn acquire(&self) -> Result<wgpu::SurfaceTexture, SurfaceError> {
        use wgpu::CurrentSurfaceTexture as C;
        match self.get_current_texture() {
            C::Success(t) | C::Suboptimal(t) => Ok(t),
            C::Timeout => Err(SurfaceError::Timeout),
            C::Occluded => Err(SurfaceError::Occluded),
            C::Outdated => Err(SurfaceError::Outdated),
            C::Lost => Err(SurfaceError::Lost),
            C::Validation => Err(SurfaceError::Validation),
        }
    }
}
