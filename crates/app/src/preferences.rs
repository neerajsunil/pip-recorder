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
    fps: i32,
    max_height: i32,
    audio_bitrate: i32,
    countdown_seconds: i32,
    auto_stop_minutes: i32,
    after_save: i32,
    theme_mode: i32,
    show_advanced: bool,
    prefer_h264: bool,
    b_frames: i32,
    lookahead: bool,
    spatial_aq: bool,
    temporal_aq: bool,
    multipass: i32,
    low_latency: bool,
    max_bitrate: i32,
    software_speed: i32,
    file_name_pattern: String,
    process_priority: i32,
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
            fps: 30,
            max_height: 0,
            audio_bitrate: 192,
            countdown_seconds: 3,
            auto_stop_minutes: 0,
            after_save: 0,
            theme_mode: 0,
            show_advanced: false,
            prefer_h264: false,
            b_frames: -1,
            lookahead: false,
            spatial_aq: true,
            temporal_aq: false,
            multipass: 1,
            low_latency: false,
            max_bitrate: 0,
            software_speed: 8,
            file_name_pattern: fastrecorder_core::DEFAULT_FILE_NAME.into(),
            process_priority: 0,
        }
    }
}
impl Preferences {
    pub fn load() -> Result<Self, String> {
        let path = fastrecorder_platform::preferences_path()?;
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
        let pick = |value: i32, allowed: &[u32], fallback: i32| {
            u32::try_from(value)
                .ok()
                .filter(|value| allowed.contains(value))
                .map_or(fallback, |value| value as i32)
        };
        ui.set_fps(pick(self.fps, &fastrecorder_core::FRAME_RATES, 30));
        ui.set_output_height(pick(self.max_height, &fastrecorder_core::OUTPUT_HEIGHTS, 0));
        ui.set_audio_bitrate(pick(
            self.audio_bitrate,
            &fastrecorder_core::AUDIO_BITRATES_KBPS,
            192,
        ));
        ui.set_countdown_seconds(pick(self.countdown_seconds, &[0, 3, 5, 10], 0));
        ui.set_auto_stop_minutes(pick(
            self.auto_stop_minutes,
            &[0, 5, 10, 15, 30, 60, 120],
            0,
        ));
        ui.set_after_save(self.after_save.clamp(0, 2));
        ui.set_theme_mode(self.theme_mode.clamp(0, 2));
        ui.set_show_advanced(self.show_advanced);
        ui.set_prefer_h264(self.prefer_h264);
        ui.set_b_frames(self.b_frames.clamp(-1, 4));
        ui.set_lookahead(self.lookahead);
        ui.set_spatial_aq(self.spatial_aq);
        ui.set_temporal_aq(self.temporal_aq);
        ui.set_multipass(self.multipass.clamp(0, 2));
        ui.set_low_latency(self.low_latency);
        ui.set_max_bitrate(self.max_bitrate.clamp(0, 200));
        ui.set_software_speed(self.software_speed.clamp(0, 10));
        ui.set_file_name_pattern(
            if self.file_name_pattern.trim().is_empty() {
                fastrecorder_core::DEFAULT_FILE_NAME
            } else {
                self.file_name_pattern.as_str()
            }
            .into(),
        );
        ui.set_process_priority(self.process_priority.clamp(0, 2));
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
            fps: ui.get_fps(),
            max_height: ui.get_output_height(),
            audio_bitrate: ui.get_audio_bitrate(),
            countdown_seconds: ui.get_countdown_seconds(),
            auto_stop_minutes: ui.get_auto_stop_minutes(),
            after_save: ui.get_after_save(),
            theme_mode: ui.get_theme_mode(),
            show_advanced: ui.get_show_advanced(),
            prefer_h264: ui.get_prefer_h264(),
            b_frames: ui.get_b_frames(),
            lookahead: ui.get_lookahead(),
            spatial_aq: ui.get_spatial_aq(),
            temporal_aq: ui.get_temporal_aq(),
            multipass: ui.get_multipass(),
            low_latency: ui.get_low_latency(),
            max_bitrate: ui.get_max_bitrate(),
            software_speed: ui.get_software_speed(),
            file_name_pattern: ui.get_file_name_pattern().to_string(),
            process_priority: ui.get_process_priority(),
            ..Self::default()
        }
    }
    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        fastrecorder_platform::save_preferences(&bytes)
    }
}
