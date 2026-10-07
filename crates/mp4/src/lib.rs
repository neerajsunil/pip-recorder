//! Portable MP4 muxing. Pure Rust and platform-neutral: every backend feeds
//! encoded packets here, and the crate is tested on every CI host.
//!
//! - [`Mp4Writer`] writes AV1 (`av01`), HEVC (`hvc1`) or H.264 (`avc1`) video,
//!   optionally interleaving a spooled [`AacTrack`] on finalization.
//! - [`attach_audio`] adds an AAC track to an MP4 produced by another muxer
//!   without re-encoding or relocating its video.
mod audio;
mod av1;
mod boxes;
mod nal;
mod writer;

pub use audio::{AAC_AUDIO_SPECIFIC_CONFIG, AAC_SAMPLE_RATE, AacTrack, attach_audio};
pub use writer::Mp4Writer;
