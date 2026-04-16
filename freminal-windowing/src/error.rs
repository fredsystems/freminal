//! Error types for the `freminal-windowing` crate.

/// Errors that can occur during windowing operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed to create the winit event loop.
    #[error("failed to create event loop: {0}")]
    EventLoopCreation(String),

    /// Failed to create the OpenGL display or context.
    #[error("failed to create GL context: {0}")]
    GlContextCreation(String),

    /// Failed to create the OpenGL surface.
    #[error("failed to create GL surface: {0}")]
    SurfaceCreation(String),

    /// Failed to create a window.
    #[error("failed to create window: {0}")]
    WindowCreation(String),

    /// Failed to make the GL context current.
    #[error("failed to make GL context current: {0}")]
    MakeCurrent(String),

    /// Failed to swap buffers.
    #[error("failed to swap buffers: {0}")]
    SwapBuffers(String),
}
