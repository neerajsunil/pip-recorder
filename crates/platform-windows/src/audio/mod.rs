//! WASAPI endpoints, a bounded timestamp-aligned stereo mixer, and AAC-LC.
//! Only compressed audio is spooled; video retains its GPU input path.
mod aac;
mod devices;
mod endpoint;
mod mixer;
mod monitor;
mod recording;

pub use devices::{AudioDevice, AudioInventory, audio_inventory};
pub use monitor::AudioMonitor;
pub(crate) use recording::AudioRecording;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};
use windows::{
    Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
    core::Result as WinResult,
};

pub(crate) const RATE: u32 = fastrecorder_mp4::AAC_SAMPLE_RATE;
const BLOCK: u64 = 1024;
const RING_FRAMES: usize = 96_000;

/// One small shared channel; no PCM or device handles cross into the UI.
#[derive(Clone, Default)]
pub struct AudioChannel(Arc<AudioFeedback>);
#[derive(Default)]
struct AudioFeedback {
    peaks: [AtomicU32; 2],
    errors: Mutex<[Option<String>; 2]>,
}
impl AudioChannel {
    pub(super) fn peak(&self, desktop: bool, value: f32) {
        self.0.peaks[usize::from(!desktop)].fetch_max(value.to_bits(), Ordering::Relaxed);
    }
    pub(super) fn error(&self, desktop: bool, error: String) {
        if let Ok(mut errors) = self.0.errors.lock() {
            errors[usize::from(!desktop)] = Some(error);
        }
    }
    pub fn take_peaks(&self) -> [f32; 2] {
        std::array::from_fn(|index| f32::from_bits(self.0.peaks[index].swap(0, Ordering::Relaxed)))
    }
    pub fn errors(&self) -> [Option<String>; 2] {
        self.0
            .errors
            .lock()
            .map(|errors| errors.clone())
            .unwrap_or_default()
    }
}

pub(crate) fn qpc_time() -> WinResult<u64> {
    static FREQUENCY: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let frequency = if let Some(frequency) = FREQUENCY.get() {
        *frequency
    } else {
        let mut frequency = 0;
        unsafe {
            QueryPerformanceFrequency(&mut frequency)?;
        }
        let _ = FREQUENCY.set(frequency);
        frequency
    };
    let mut ticks = 0;
    unsafe {
        QueryPerformanceCounter(&mut ticks)?;
    }
    Ok((i128::from(ticks) * 10_000_000 / i128::from(frequency)) as u64)
}
