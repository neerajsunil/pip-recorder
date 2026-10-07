//! Idle level metering for enabled sources; packets are discarded.
use super::{AudioChannel, devices::Apartment, endpoint::Endpoint, mixer::Mixer};
use fastrecorder_core::AudioConfig;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Metering only: packets are discarded, never recorded or played back.
pub struct AudioMonitor {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl AudioMonitor {
    pub fn start(config: AudioConfig, feedback: AudioChannel) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("fastrecorder-audio-meters".into())
            .spawn(move || {
                let run = || -> Result<(), String> {
                    let _apartment = Apartment::new().map_err(|e| e.to_string())?;
                    let mut endpoints = Vec::new();
                    for (enabled, desktop, id, volume) in [
                        (
                            config.desktop,
                            true,
                            config.desktop_device.as_deref(),
                            config.desktop_volume,
                        ),
                        (
                            config.microphone,
                            false,
                            config.microphone_device.as_deref(),
                            config.microphone_volume,
                        ),
                    ] {
                        if worker_stop.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        if enabled {
                            match Endpoint::open(id, desktop, volume) {
                                Ok(endpoint) => endpoints.push(endpoint),
                                Err(error) => feedback.error(desktop, error),
                            }
                        }
                    }
                    let mut ring = Mixer::new();
                    while !worker_stop.load(Ordering::Acquire) && !endpoints.is_empty() {
                        endpoints.retain(|endpoint| {
                            match endpoint.drain(0, &mut ring, &feedback) {
                                Ok(()) => true,
                                Err(error) => {
                                    feedback.error(endpoint.desktop, error);
                                    false
                                }
                            }
                        });
                        thread::sleep(Duration::from_millis(20));
                    }
                    Ok(())
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
                    .unwrap_or_else(|_| {
                        Err("Audio meters stopped. Refresh audio devices to try again.".into())
                    });
                if let Err(error) = result {
                    if config.desktop {
                        feedback.error(true, error.clone());
                    }
                    if config.microphone {
                        feedback.error(false, error);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
    pub fn join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
            // A broken endpoint driver must not hold recording startup or app exit hostage.
        }
    }
}
impl Drop for AudioMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
