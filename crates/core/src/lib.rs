use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionState {
    #[default]
    Idle,
    Starting,
    Recording,
    Stopping,
}

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

#[derive(Clone, Debug)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncoderPreference {
    #[default]
    Auto,
    SoftwareOnly,
    NvencAv1,
    HardwareH264,
    NvencHevc,
    NvencH264,
    IntelAv1,
    IntelHevc,
    IntelH264,
    SoftwareAv1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Av1,
    Hevc,
    H264,
}
impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Self::Av1 => "AV1",
            Self::Hevc => "HEVC",
            Self::H264 => "H.264",
        }
    }
}
impl EncoderPreference {
    pub fn codec(self) -> Codec {
        match self {
            Self::NvencAv1 | Self::IntelAv1 | Self::SoftwareAv1 => Codec::Av1,
            Self::NvencHevc | Self::IntelHevc => Codec::Hevc,
            _ => Codec::H264,
        }
    }
}

/// Screen-content starting points at 30 fps. Interpolate in pixel area between
/// common resolution tiers; extrapolate gently outside them. These are product
/// defaults, not guaranteed quality or a claim about codec equivalence.
pub fn recommended_bitrate_mbps(codec: Codec, width: u32, height: u32, fps: u32) -> u32 {
    let pixels = f64::from(width.max(1)) * f64::from(height.max(1));
    let rates = match codec {
        Codec::Av1 => [4.0, 6.0, 12.0],
        Codec::Hevc => [5.0, 8.0, 16.0],
        Codec::H264 => [8.0, 12.0, 24.0],
    };
    let areas = [1920.0 * 1080.0, 2560.0 * 1440.0, 3840.0 * 2160.0];
    let rate = if pixels < areas[0] {
        rates[0] * (pixels / areas[0]).powf(0.7)
    } else if pixels > areas[2] {
        rates[2] * (pixels / areas[2]).powf(0.7)
    } else {
        let i = if pixels <= areas[1] { 0 } else { 1 };
        rates[i] + (rates[i + 1] - rates[i]) * (pixels - areas[i]) / (areas[i + 1] - areas[i])
    };
    (rate * (f64::from(fps.max(1)) / 30.0).powf(0.7))
        .round()
        .clamp(1.0, 100.0) as u32
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

#[derive(Default, Debug)]
pub struct Session {
    state: SessionState,
}

impl Session {
    pub fn state(&self) -> SessionState {
        self.state
    }
    pub fn begin(&mut self) -> bool {
        if self.state != SessionState::Idle {
            return false;
        }
        self.state = SessionState::Starting;
        true
    }
    pub fn started(&mut self) {
        if self.state == SessionState::Starting {
            self.state = SessionState::Recording;
        }
    }
    pub fn stop(&mut self) -> bool {
        if !matches!(self.state, SessionState::Starting | SessionState::Recording) {
            return false;
        }
        self.state = SessionState::Stopping;
        true
    }
    pub fn finished(&mut self) {
        self.state = SessionState::Idle;
    }
}

/// Frame pacing uses a wall-clock grid, never frame arrival counts.
pub fn frame_time(index: u64, fps: u32) -> i64 {
    ((u128::from(index) * 10_000_000) / u128::from(fps)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_during_startup_cannot_return_to_recording() {
        let mut session = Session::default();
        assert!(session.begin());
        assert!(!session.begin());
        assert!(session.stop());
        session.started();
        assert_eq!(session.state(), SessionState::Stopping);
        assert!(!session.stop());
        session.finished();
        assert!(session.begin());
    }
    #[test]
    fn frame_clock_has_no_accumulating_rounding_drift() {
        assert_eq!(frame_time(60 * 3600, 60), 36_000_000_000);
        assert_eq!(frame_time(30 * 3600, 30), 36_000_000_000);
        assert!(frame_time(1, 60) < frame_time(2, 60));
    }
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
