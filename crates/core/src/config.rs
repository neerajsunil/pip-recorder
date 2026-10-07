//! User-facing recording configuration and its validation.
use crate::{Codec, EncoderPreference, recommended_bitrate_mbps};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct RecordingConfig {
    pub destination: PathBuf,
    pub fps: u32,
    /// Largest output height; 0 keeps the source resolution. See [`output_size`].
    pub max_height: u32,
    pub encoder: EncoderPreference,
    pub bitrate_mbps: u32,
    pub bitrate_mode: u32,
    pub gpu_index: Option<u32>,
    pub nvenc_preset: u32,
    pub keyframe_seconds: u32,
    pub constant_bitrate: bool,
    pub capture_cursor: bool,
    pub quality_qp: Option<u32>,
    /// With `EncoderPreference::Auto`, only use H.264 so the file plays everywhere.
    pub prefer_h264: bool,
    pub tuning: EncoderTuning,
    pub audio: AudioConfig,
}

/// NVENC multipass encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Multipass {
    Off,
    #[default]
    QuarterResolution,
    FullResolution,
}

/// Expert encoder controls (Settings → Video → Advanced). The defaults are a
/// quality-first balance for local recordings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderTuning {
    /// Maximum B-frames; `None` lets Pip choose (up to 2).
    pub b_frames: Option<u32>,
    pub lookahead: bool,
    pub spatial_aq: bool,
    pub temporal_aq: bool,
    pub multipass: Multipass,
    /// NVENC low-latency tuning instead of high quality.
    pub low_latency: bool,
    /// Peak bitrate for VBR in Mbps; 0 means twice the target.
    pub max_bitrate_mbps: u32,
    /// rav1e speed preset, 0 (slowest, best) to 10 (fastest).
    pub software_speed: u32,
}
impl Default for EncoderTuning {
    fn default() -> Self {
        Self {
            b_frames: None,
            lookahead: false,
            spatial_aq: true,
            temporal_aq: false,
            multipass: Multipass::QuarterResolution,
            low_latency: false,
            max_bitrate_mbps: 0,
            software_speed: 8,
        }
    }
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
    /// AAC-LC bitrate: one of [`AUDIO_BITRATES_KBPS`].
    pub bitrate_kbps: u32,
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
            bitrate_kbps: 192,
        }
    }
}
impl AudioConfig {
    pub fn enabled(&self) -> bool {
        self.desktop || self.microphone
    }
}

/// Frame rates the recorder offers.
pub const FRAME_RATES: [u32; 5] = [24, 25, 30, 50, 60];
/// Output height limits the recorder offers; 0 means "same as source".
pub const OUTPUT_HEIGHTS: [u32; 5] = [0, 2160, 1440, 1080, 720];
/// AAC-LC bitrates supported by the Windows AAC encoder.
pub const AUDIO_BITRATES_KBPS: [u32; 4] = [96, 128, 160, 192];

/// Encoded frame size for a source, scaled down (never up) to `max_height`
/// with its aspect ratio kept. Both dimensions are even for 4:2:0 video.
pub fn output_size(width: u32, height: u32, max_height: u32) -> (u32, u32) {
    let even = |value: u32| (value.max(2) + 1) & !1;
    if max_height == 0 || height <= max_height {
        return (even(width), even(height));
    }
    // Round the scaled width to the nearest even number to keep the aspect ratio.
    let scaled = f64::from(width) * f64::from(max_height) / f64::from(height);
    (((scaled / 2.0).round() as u32 * 2).max(2), even(max_height))
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
        if self.tuning.b_frames.is_some_and(|b| b > 4) {
            return Err("Choose at most 4 B-frames.".into());
        }
        if self.tuning.software_speed > 10 {
            return Err("Choose a software AV1 speed from 0 to 10.".into());
        }
        if self.tuning.max_bitrate_mbps > 200 {
            return Err("Choose a peak bitrate of at most 200 Mbps.".into());
        }
        if self.bitrate_mode > 3 {
            return Err("Choose High, Recommended, Low, or Custom bitrate.".into());
        }
        if !FRAME_RATES.contains(&self.fps) {
            return Err("Choose 24, 25, 30, 50 or 60 frames per second.".into());
        }
        if !OUTPUT_HEIGHTS.contains(&self.max_height) {
            return Err("Choose a supported output resolution.".into());
        }
        if !AUDIO_BITRATES_KBPS.contains(&self.audio.bitrate_kbps) {
            return Err("Choose an audio quality of 96, 128, 160 or 192 kbps.".into());
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
            max_height: 0,
            encoder: EncoderPreference::Auto,
            bitrate_mbps: 20,
            bitrate_mode: 3,
            gpu_index: None,
            nvenc_preset: 4,
            keyframe_seconds: 2,
            constant_bitrate: false,
            capture_cursor: true,
            quality_qp: None,
            prefer_h264: false,
            tuning: EncoderTuning::default(),
            audio: AudioConfig::default(),
        };
        assert!(config.validate().is_err());
        let config = RecordingConfig {
            destination: "capture.mp4".into(),
            fps: 120,
            max_height: 0,
            encoder: EncoderPreference::Auto,
            bitrate_mbps: 20,
            bitrate_mode: 3,
            gpu_index: None,
            nvenc_preset: 4,
            keyframe_seconds: 2,
            constant_bitrate: false,
            capture_cursor: true,
            quality_qp: None,
            prefer_h264: false,
            tuning: EncoderTuning::default(),
            audio: AudioConfig::default(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn output_size_scales_down_and_keeps_even_dimensions() {
        assert_eq!(output_size(1920, 1080, 0), (1920, 1080));
        assert_eq!(output_size(1919, 1079, 0), (1920, 1080));
        assert_eq!(output_size(1920, 1080, 1440), (1920, 1080));
        assert_eq!(output_size(3840, 2160, 1080), (1920, 1080));
        assert_eq!(output_size(2560, 1440, 720), (1280, 720));
        assert_eq!(output_size(3440, 1440, 1080), (2580, 1080));
        assert_eq!(output_size(1366, 768, 720), (1280, 720));
    }
}
