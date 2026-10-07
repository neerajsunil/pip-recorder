//! Audio recording worker: endpoints → mixer → AAC spool, aligned to the video clock.
use super::{
    AudioChannel, BLOCK, RATE, RING_FRAMES,
    aac::AacEncoder,
    devices::{Apartment, MediaFoundation},
    endpoint::Endpoint,
    mixer::Mixer,
    qpc_time,
};
use fastrecorder_core::AudioConfig;
use fastrecorder_mp4::AacTrack;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(crate) struct AudioCompletion {
    pub track: Option<AacTrack>,
    pub error: Option<String>,
}
pub(crate) struct AudioRecording {
    epoch: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    end: Arc<AtomicU64>,
    failure: Arc<Mutex<Option<String>>>,
    worker: Option<JoinHandle<AudioCompletion>>,
    path: PathBuf,
    pub description: String,
}
impl AudioRecording {
    pub fn start(
        config: AudioConfig,
        path: PathBuf,
        feedback: AudioChannel,
        cancelled: &AtomicBool,
    ) -> Result<Self, String> {
        let epoch = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let end = Arc::new(AtomicU64::new(0));
        let failure = Arc::new(Mutex::new(None));
        let (worker_epoch, worker_stop, worker_end, worker_failure) =
            (epoch.clone(), stop.clone(), end.clone(), failure.clone());
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let spool_path = path.clone();
        let worker = thread::Builder::new()
            .name("fastrecorder-audio".into())
            .spawn(move || {
                let setup = (|| -> Result<_, String> {
                    let apartment = Apartment::new().map_err(|e| e.to_string())?;
                    let media = MediaFoundation::new().map_err(|e| e.to_string())?;
                    let mut endpoints = Vec::new();
                    if config.desktop {
                        endpoints.push(Endpoint::open(
                            config.desktop_device.as_deref(),
                            true,
                            config.desktop_volume,
                        ).inspect_err(|error| feedback.error(true, error.clone()))?);
                    }
                    if config.microphone {
                        endpoints.push(Endpoint::open(
                            config.microphone_device.as_deref(),
                            false,
                            config.microphone_volume,
                        ).inspect_err(|error| feedback.error(false, error.clone()))?);
                    }
                    if worker_stop.load(Ordering::Acquire) { return Err("Audio startup was cancelled.".into()); }
                    let encoder = AacEncoder::new(&path)
                        .map_err(|e| format!("Could not initialize AAC audio: {e}"))?;
                    Ok((apartment, media, endpoints, encoder))
                })();
                let (apartment, media, endpoints, mut encoder) = match setup {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = sender.send(Err(error.clone()));
                        return AudioCompletion {
                            track: None,
                            error: Some(error),
                        };
                    }
                };
                let description = format!(
                    "{} → stereo AAC-LC · 48 kHz · 192 kbps",
                    endpoints
                        .iter()
                        .map(|endpoint| endpoint.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" + ")
                );
                if sender.send(Ok(description)).is_err() {
                    worker_stop.store(true, Ordering::Release);
                }
                let mut ring = Mixer::new();
                let result = (|| -> Result<(), String> {
                    while !worker_stop.load(Ordering::Acquire) {
                        let epoch = worker_epoch.load(Ordering::Acquire);
                        for endpoint in &endpoints {
                            endpoint.drain(epoch, &mut ring, &feedback).inspect_err(|error| feedback.error(endpoint.desktop, error.clone()))?;
                        }
                        if epoch != 0 {
                            let due = qpc_time().map_err(|e| e.to_string())?.saturating_sub(epoch)
                                * u64::from(RATE)
                                / 10_000_000;
                            let ready = due.saturating_sub(4_800); // 100 ms margin for late endpoint packets.
                            if ready.saturating_sub(ring.cursor) > RING_FRAMES as u64 {
                                return Err("Recording stopped after an audio timing interruption. The recorded portion will be saved. Avoid putting the computer to sleep while recording.".into());
                            }
                            while ring.cursor + BLOCK <= ready {
                                let first = ring.cursor;
                                encoder.encode(&ring.take(BLOCK), first, BLOCK)?;
                            }
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    let epoch = worker_epoch.load(Ordering::Acquire);
                    for endpoint in &endpoints {
                        // A lost endpoint must not prevent already-buffered audio from being flushed.
                        let _ = endpoint.drain(epoch, &mut ring, &feedback);
                    }
                    let end = worker_end.load(Ordering::Acquire);
                    if end.saturating_sub(ring.cursor) > RING_FRAMES as u64 {
                        return Err("Audio ended after a timing interruption; the recorded portion has been retained.".into());
                    }
                    while ring.cursor < end {
                        let first = ring.cursor;
                        let count = BLOCK.min(end - first);
                        encoder.encode(&ring.take(count), first, count)?;
                    }
                    Ok(())
                })();
                let mut error = result.err();
                if let Some(message) = &error {
                    *worker_failure.lock().unwrap() = Some(message.clone());
                }
                if let Err(message) = encoder.finish() {
                    error =
                        Some(error.map_or(message.clone(), |prior| format!("{prior}; {message}")));
                    *worker_failure.lock().unwrap() = error.clone();
                }
                let priming = encoder
                    .first_time
                    .filter(|time| *time < 0)
                    .map_or(0, |time| {
                        ((-i128::from(time)) * i128::from(RATE) / 10_000_000) as u64
                    });
                let sizes = std::mem::take(&mut encoder.sizes);
                drop(encoder);
                drop(endpoints);
                drop(media);
                drop(apartment);
                let track = if sizes.is_empty() {
                    if worker_end.load(Ordering::Acquire) > 0 && error.is_none() {
                        error = Some("AAC encoder produced no audio packets.".into());
                    }
                    let _ = std::fs::remove_file(&path);
                    None
                } else {
                    Some(AacTrack {
                        path,
                        sizes,
                        frames: ring.cursor,
                        priming,
                    })
                };
                AudioCompletion { track, error }
            })
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let setup = loop {
            if cancelled.load(Ordering::Acquire) {
                stop.store(true, Ordering::Release);
                return Err("Audio startup was cancelled.".into());
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(setup) => break setup,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {
                    continue;
                }
                Err(error) => {
                    stop.store(true, Ordering::Release);
                    return Err(format!(
                        "Audio could not start: {error}. Refresh audio devices or disable the affected audio source."
                    ));
                }
            }
        };
        match setup {
            Ok(description) => Ok(Self {
                epoch,
                stop,
                end,
                failure,
                worker: Some(worker),
                path: spool_path,
                description,
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
    pub fn begin(&self) -> Result<(), String> {
        let time = qpc_time().map_err(|e| e.to_string())?;
        let _ = self
            .epoch
            .compare_exchange(0, time, Ordering::Release, Ordering::Relaxed);
        Ok(())
    }
    pub fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap().clone().or_else(|| {
            self.worker
                .as_ref()
                .filter(|worker| worker.is_finished())
                .map(|_| "Audio capture stopped unexpectedly.".into())
        })
    }
    pub fn finish(&mut self, frames: u64) -> AudioCompletion {
        self.end.store(frames, Ordering::Release);
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap_or_else(|_| {
            let _ = std::fs::remove_file(&self.path);
            AudioCompletion {
                track: None,
                error: Some("Audio capture thread failed.".into()),
            }
        })
    }
}
impl Drop for AudioRecording {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.finish(0);
        }
    }
}
