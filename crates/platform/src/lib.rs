//! Compile-time platform facade.
//!
//! The application depends only on this crate. Each backend crate
//! (`fastrecorder-windows` today; `fastrecorder-macos` and
//! `fastrecorder-linux` later) exposes the same set of names, and this crate
//! re-exports the one matching `target_os`. See `docs/ARCHITECTURE.md` for the
//! contract a new backend must satisfy.

/// Whether this build has a native recording backend.
pub const RECORDING_SUPPORTED: bool = cfg!(target_os = "windows");

/// Human-readable name of the active backend.
pub const BACKEND: &str = if cfg!(target_os = "windows") {
    "Windows Graphics Capture"
} else {
    "none"
};

#[cfg(target_os = "windows")]
pub use fastrecorder_windows::*;
