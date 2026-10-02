//! Versioned, per-user preferences. GPU identity is a name, never a DXGI ordinal.
use crate::MainWindow;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    version: u32,
    pub save_directory: Option<PathBuf>,
    pub last_recording: Option<PathBuf>,
    pub encoder: i32,
    pub gpu: Option<String>,
    bitrate_mode: i32,
    custom_bitrate: i32,
    nvenc_preset: i32,
    keyframe_seconds: i32,
    constant_bitrate: bool,
    auto_minimize: bool,
    capture_cursor: bool,
    quality_mode: bool,
    quality_level: i32,
    desktop_audio: bool,
    microphone_audio: bool,
    pub desktop_device: Option<String>,
    pub microphone_device: Option<String>,
    desktop_volume: i32,
    microphone_volume: i32,
    start_shortcut: String,
    stop_shortcut: String,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            save_directory: None,
            last_recording: None,
            encoder: 0,
            gpu: None,
            bitrate_mode: 1,
            custom_bitrate: 8,
            nvenc_preset: 5,
            keyframe_seconds: 2,
            constant_bitrate: false,
            auto_minimize: true,
            capture_cursor: true,
            quality_mode: false,
            quality_level: 20,
            desktop_audio: true,
            microphone_audio: false,
            desktop_device: None,
            microphone_device: None,
            desktop_volume: 100,
            microphone_volume: 100,
            start_shortcut: "Ctrl+Shift+F9".into(),
            stop_shortcut: "Ctrl+Shift+F10".into(),
        }
    }
}
impl Preferences {
    pub fn load() -> Result<Self, String> {
        let path = fastrecorder_windows::preferences_path()?;
        match std::fs::read(&path) {
            Ok(bytes) => {
                let value: Self = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Could not read preferences: {e}"))?;
                if value.version != 1 {
                    return Err("Unsupported preferences version; using defaults.".into());
                }
                Ok(value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("Could not read preferences: {error}")),
        }
    }
    pub fn apply(&self, ui: &MainWindow) {
        // Frame rate intentionally starts at 30 fps on every launch.
        ui.set_bitrate_mode(self.bitrate_mode.clamp(0, 3));
        ui.set_custom_bitrate(self.custom_bitrate.clamp(1, 100));
        ui.set_nvenc_preset(self.nvenc_preset.clamp(1, 7));
        ui.set_keyframe_seconds(self.keyframe_seconds.clamp(1, 10));
        ui.set_constant_bitrate(self.constant_bitrate);
        ui.set_auto_minimize(self.auto_minimize);
        ui.set_capture_cursor(self.capture_cursor);
        ui.set_quality_mode(self.quality_mode);
        ui.set_quality_level(self.quality_level.clamp(1, 51));
        ui.set_desktop_audio(self.desktop_audio);
        ui.set_microphone_audio(self.microphone_audio);
        ui.set_desktop_volume(self.desktop_volume.clamp(0, 200));
        ui.set_microphone_volume(self.microphone_volume.clamp(0, 200));
        ui.set_start_shortcut(self.start_shortcut.clone().into());
        ui.set_stop_shortcut(self.stop_shortcut.clone().into());
        ui.set_active_start_shortcut(self.start_shortcut.clone().into());
        ui.set_active_stop_shortcut(self.stop_shortcut.clone().into());
    }
    pub fn capture(
        ui: &MainWindow,
        save_directory: Option<&Path>,
        last_recording: Option<&Path>,
        gpu: Option<String>,
        desktop_device: Option<String>,
        microphone_device: Option<String>,
    ) -> Self {
        Self {
            save_directory: save_directory.map(PathBuf::from),
            last_recording: last_recording.map(PathBuf::from),
            encoder: ui.get_encoder_choice(),
            gpu,
            bitrate_mode: ui.get_bitrate_mode(),
            custom_bitrate: ui.get_custom_bitrate(),
            nvenc_preset: ui.get_nvenc_preset(),
            keyframe_seconds: ui.get_keyframe_seconds(),
            constant_bitrate: ui.get_constant_bitrate(),
            auto_minimize: ui.get_auto_minimize(),
            capture_cursor: ui.get_capture_cursor(),
            quality_mode: ui.get_quality_mode(),
            quality_level: ui.get_quality_level(),
            desktop_audio: ui.get_desktop_audio(),
            microphone_audio: ui.get_microphone_audio(),
            desktop_device,
            microphone_device,
            desktop_volume: ui.get_desktop_volume(),
            microphone_volume: ui.get_microphone_volume(),
            start_shortcut: ui.get_active_start_shortcut().to_string(),
            stop_shortcut: ui.get_active_stop_shortcut().to_string(),
            ..Self::default()
        }
    }
    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        fastrecorder_windows::save_preferences(&bytes)
    }
}
