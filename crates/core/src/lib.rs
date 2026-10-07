//! Platform-neutral recording domain: configuration, validation, the session
//! state machine, codec choices and media timing. No OS APIs live here, so
//! everything in this crate builds and is tested on every target.
mod bitrate;
mod codec;
mod config;
mod filename;
mod session;
mod timing;

pub use bitrate::recommended_bitrate_mbps;
pub use codec::{Codec, EncoderPreference};
pub use config::{
    AUDIO_BITRATES_KBPS, AudioConfig, EncoderTuning, FRAME_RATES, Multipass, OUTPUT_HEIGHTS,
    RecordingConfig, output_size,
};
pub use filename::{DEFAULT_FILE_NAME, LocalTime, file_name};
pub use session::{Session, SessionState};
pub use timing::frame_time;
