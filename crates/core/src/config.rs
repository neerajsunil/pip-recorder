//! User-facing recording configuration and its validation.
use crate::{Codec, EncoderPreference, recommended_bitrate_mbps};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct RecordingConfig {
    pub destination: PathBuf,
    pub fps: u32,
    pub encoder: EncoderPreference,
    pub bitrate_mbps: u32,
    pub bitrate_mode: u32,
    pub gpu_index: Option<u32>,
    pub nvenc_preset: u32,
    pub keyframe_seconds: u32,
    pub constant_bitrate: bool,
    pub capture_cursor: bool,
    pub quality_qp: Option<u32>,
    pub audio: AudioConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    pub desktop: bool,
    pub microphone: bool,
    /// None selects the Windows default endpoint when recording starts.
    pub desktop_device: Option<String>,
    pub microphone_device: Option<String>,
    pub desktop_volume: u32,
    pub microphone_volume: u32,
}
impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            desktop: true,
            microphone: false,
            desktop_device: None,
            microphone_device: None,
            desktop_volume: 100,
            microphone_volume: 100,
        }
    }
}
impl AudioConfig {
    pub fn enabled(&self) -> bool {
        self.desktop || self.microphone
    }
}

impl RecordingConfig {
    pub fn bitrate_for(&self, codec: Codec, width: u32, height: u32) -> u32 {
        let recommended = recommended_bitrate_mbps(codec, width, height, self.fps);
        match self.bitrate_mode {
            0 => (recommended as f64 * 1.5).round() as u32,
            2 => (recommended as f64 * 0.65).round() as u32,
            3 => self.bitrate_mbps,
            _ => recommended,
        }
        .clamp(1, 100)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.audio.desktop_volume > 200 || self.audio.microphone_volume > 200 {
            return Err("Audio volume must be between 0 and 200%.".into());
        }
        if self.quality_qp.is_some_and(|qp| !(1..=51).contains(&qp)) {
            return Err("Choose a quality level between 1 and 51.".into());
        }
        if !(1..=7).contains(&self.nvenc_preset) || !(1..=10).contains(&self.keyframe_seconds) {
            return Err(
                "Choose NVENC preset P1–P7 and a keyframe interval of 1–10 seconds.".into(),
            );
        }
        if !(1..=100).contains(&self.bitrate_mbps) {
            return Err("Choose a bitrate between 1 and 100 Mbps.".into());
        }
        if self.bitrate_mode > 3 {
            return Err("Choose High, Recommended, Low, or Custom bitrate.".into());
        }
        if !matches!(self.fps, 30 | 60) {
            return Err("Choose 30 or 60 frames per second.".into());
        }
        if self
            .destination
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("mp4"))
        {
            return Err("Choose an MP4 destination.".into());
        }
        if self.destination.exists() {
            return Err("That file already exists. Choose a new filename.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_configuration_is_rejected() {
        let config = RecordingConfig {
            destination: "capture.mov".into(),
            fps: 30,
            encoder: EncoderPreference::Auto,
            bitrate_mbps: 20,
            bitrate_mode: 3,
            gpu_index: None,
            nvenc_preset: 4,
            keyframe_seconds: 2,
            constant_bitrate: false,
            capture_cursor: true,
            quality_qp: None,
            audio: AudioConfig::default(),
        };
        assert!(config.validate().is_err());
        let config = RecordingConfig {
            destination: "capture.mp4".into(),
            fps: 120,
            encoder: EncoderPreference::Auto,
            bitrate_mbps: 20,
            bitrate_mode: 3,
            gpu_index: None,
            nvenc_preset: 4,
            keyframe_seconds: 2,
            constant_bitrate: false,
            capture_cursor: true,
            quality_qp: None,
            audio: AudioConfig::default(),
        };
        assert!(config.validate().is_err());
    }
}
