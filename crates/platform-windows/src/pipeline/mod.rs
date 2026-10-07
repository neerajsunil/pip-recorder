//! The recording session: a worker thread that captures, converts and encodes.
mod convert;
mod worker;

use crate::Source;

use fastrecorder_core::RecordingConfig;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};
use windows::Win32::{Media::MediaFoundation::*, System::WinRT::*};

#[derive(Debug)]
pub enum RecordingEvent {
    Started {
        hardware: bool,
        bitrate_mbps: u32,
        encoder: String,
        gpu: String,
        codec: String,
        fallback: Option<String>,
        audio: String,
    },
    Statistics {
        dropped: u64,
    },
    Finished {
        file: Option<PathBuf>,
        error: Option<String>,
    },
}

pub struct Recording {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Recording {
    pub fn start(
        source: Source,
        config: RecordingConfig,
        preview: crate::PreviewChannel,
        audio_feedback: crate::AudioChannel,
        mut audio_monitor: Option<crate::AudioMonitor>,
        events: impl Fn(RecordingEvent) + Send + 'static,
    ) -> Result<Self, String> {
        config.validate()?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("fastrecorder-recording".into())
            .spawn(move || {
                if let Some(monitor) = &mut audio_monitor { monitor.join(); }
                // Keep initialization and cleanup on the same MTA thread.
                let result =
                    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string());
                match result {
                    Ok(()) => {
                        let result = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) };
                        if let Err(e) = result {
                            events(RecordingEvent::Finished {
                                file: None,
                                error: Some(e.to_string()),
                            });
                        } else {
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                worker::record(&source, &config, &worker_stop, &preview, &audio_feedback, &events)
                            })).unwrap_or_else(|_| Err(format!(
                                "Recording stopped after an internal error. An incomplete file may remain in {}. See the local diagnostic log for details.",
                                config.destination.parent().unwrap_or(Path::new(".")).display()
                            )));
                            if let Err(e) = result {
                                events(RecordingEvent::Finished {
                                    file: None,
                                    error: Some(e),
                                });
                            }
                            unsafe {
                                let _ = MFShutdown();
                            }
                        }
                        unsafe {
                            RoUninitialize();
                        }
                    }
                    Err(e) => events(RecordingEvent::Finished {
                        file: None,
                        error: Some(e),
                    }),
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn join(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Recording {
    fn drop(&mut self) {
        self.stop();
    }
}
