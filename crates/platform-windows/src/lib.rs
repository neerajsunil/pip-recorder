//! FastRecorder's Windows backend.
//!
//! Native recording resources (D3D11 devices, WGC sessions, encoder handles)
//! stay inside this crate; only bounded preview pixels, events and plain data
//! reach the UI. The application reaches this crate through
//! `fastrecorder-platform`, never directly.
//!
//! | Module      | Responsibility                                              |
//! |-------------|-------------------------------------------------------------|
//! | `capture`   | Windows Graphics Capture sources, sessions, HDR → SDR color  |
//! | `pipeline`  | The recording worker: capture → convert → encode → MP4       |
//! | `encode`    | Encoder selection: NVENC, Intel VPL, Media Foundation, rav1e |
//! | `audio`     | WASAPI capture/loopback, mixing, AAC-LC                      |
//! | `preview`   | Low-rate studio preview off the capture device               |
//! | `gpu`       | Adapter enumeration, device creation, texture helpers        |
//! | `shell`     | Dialogs, Explorer, window styling, COM apartment             |
//! | `files`     | Known folders, atomic preference writes, disk checks         |
//! | `shortcut`  | Global hotkeys on a dedicated message thread                 |
#![cfg(target_os = "windows")]

mod audio;
mod capture;
mod encode;
mod files;
mod gpu;
mod pipeline;
mod preview;
mod shell;
mod shortcut;
#[cfg(feature = "diagnostics")]
mod validation;

pub use audio::{AudioChannel, AudioDevice, AudioInventory, AudioMonitor, audio_inventory};
pub use capture::{
    Source, SourceCandidate, capture_supported, enumerate_sources, own_window_source,
};
pub use files::{default_recording_directory, preferences_path, save_preferences};
pub use gpu::{GpuInfo, gpu_inventory};
pub use pipeline::{Recording, RecordingEvent};
pub use preview::{Preview, PreviewChannel, PreviewFrame};
pub use shell::{
    UiApartment, choose_destination, dark_titlebar, exclude_from_capture, initialize_ui,
    open_recording, reveal_recording, show_details, timestamped_destination,
};
pub use shortcut::{RecordingShortcut, ShortcutAction};
#[cfg(feature = "diagnostics")]
pub use validation::{Validation, validate_recording};
