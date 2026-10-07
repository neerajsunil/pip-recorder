//! Studio state shared between Slint callbacks.
use fastrecorder_core::Session;
use fastrecorder_platform::{self as native, Recording, Source};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Instant,
};

#[derive(Default)]
pub(super) struct AppState {
    pub(super) session: Session,
    pub(super) source: Option<Source>,
    pub(super) sources: Vec<native::SourceCandidate>,
    pub(super) selected_candidate: Option<native::SourceCandidate>,
    pub(super) destination: Option<PathBuf>,
    pub(super) custom_destination: bool,
    pub(super) recording: Option<Recording>,
    pub(super) preview: Option<native::Preview>,
    pub(super) preview_channel: native::PreviewChannel,
    pub(super) gpus: Vec<native::GpuInfo>,
    pub(super) started: Option<Instant>,
    pub(super) last_file: Option<PathBuf>,
    pub(super) close_after_save: bool,
    pub(super) preferred_encoder: Option<i32>,
    pub(super) preferred_gpu: Option<String>,
    pub(super) desktop_devices: Vec<native::AudioDevice>,
    pub(super) microphone_devices: Vec<native::AudioDevice>,
    pub(super) desktop_device: Option<String>,
    pub(super) microphone_device: Option<String>,
    pub(super) audio_refresh: u64,
    pub(super) audio_channel: native::AudioChannel,
    pub(super) audio_monitor: Option<native::AudioMonitor>,
    pub(super) meter_config: Option<fastrecorder_core::AudioConfig>,
    #[cfg(feature = "diagnostics")]
    pub(super) diagnostic: bool,
    #[cfg(feature = "diagnostics")]
    pub(super) diagnostic_failure: Option<String>,
}

pub(super) type Shared = Arc<Mutex<AppState>>;

/// UI state stays usable after a panic in another callback; a poisoned
/// lock must not cascade into every later event.
pub(super) trait Lock<T> {
    fn locked(&self) -> MutexGuard<'_, T>;
}
impl<T> Lock<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
