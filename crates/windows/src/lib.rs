//! Native recording resources stay inside this backend; only bounded preview pixels reach the UI.
#[cfg(target_os = "windows")]
mod native;
#[cfg(target_os = "windows")]
pub use native::*;

#[cfg(target_os = "windows")]
mod sources;
#[cfg(target_os = "windows")]
pub use sources::*;

#[cfg(target_os = "windows")]
mod mp4;
#[cfg(target_os = "windows")]
mod nvenc;
#[cfg(target_os = "windows")]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    unnecessary_transmutes,
    unpredictable_function_pointer_comparisons,
    clippy::all
)]
mod nvenc_api;
#[cfg(target_os = "windows")]
mod preview;
#[cfg(target_os = "windows")]
pub use preview::{Preview, PreviewChannel, PreviewFrame};
#[cfg(target_os = "windows")]
mod hardware;
#[cfg(target_os = "windows")]
pub use hardware::*;

#[cfg(all(target_os = "windows", feature = "diagnostics"))]
mod validation;
#[cfg(all(target_os = "windows", feature = "diagnostics"))]
pub use validation::*;
#[cfg(target_os = "windows")]
mod intel;
#[cfg(target_os = "windows")]
mod shortcut;
#[cfg(target_os = "windows")]
mod software_av1;
#[cfg(target_os = "windows")]
pub use shortcut::{RecordingShortcut, ShortcutAction};
#[cfg(target_os = "windows")]
mod audio;
#[cfg(target_os = "windows")]
pub use audio::{AudioDevice, AudioInventory, audio_inventory};
