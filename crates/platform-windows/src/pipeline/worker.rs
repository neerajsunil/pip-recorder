//! Recording worker: temporary file lifecycle and the capture/encode loop.
use super::{RecordingEvent, convert::Converter};
use crate::{
    Source,
    capture::{Capture, CapturedFrame, configure_capture_session},
    encode::{
        VideoEncoder,
        media_foundation::{attributes, media_type},
    },
    files::{available_disk_space, move_without_overwrite},
};
use fastrecorder_core::{RecordingConfig, frame_time};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::*,
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Graphics::{Direct3D11::*, Dxgi::*},
        Media::MediaFoundation::*,
        System::WinRT::Direct3D11::*,
    },
    core::{IInspectable, Interface, Result as WinResult},
};

pub(super) fn record(
    source: &Source,
    config: &RecordingConfig,
    stop: &AtomicBool,
    preview: &crate::PreviewChannel,
    audio_feedback: &crate::AudioChannel,
    events: &impl Fn(RecordingEvent),
) -> Result<(), String> {
    if stop.load(Ordering::Acquire) {
        events(RecordingEvent::Finished {
            file: None,
            error: None,
        });
        return Ok(());
    }
    let directory = config
        .destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if available_disk_space(directory)? < 256 * 1024 * 1024 {
        return Err("Less than 256 MB is available. Choose a drive with more space.".into());
    }
    if source.width == 0 || source.height == 0 {
        return Err("The selected source has no visible content.".into());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = config.destination.with_file_name(format!(
        ".{}.{}.partial.mp4",
        config
            .destination
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy(),
        stamp
    ));
    // Reserve only our own temporary path; destination files are never overwritten.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| format!("Cannot write here: {e}"))?;
    let result = record_inner(
        source,
        config,
        stop,
        preview,
        audio_feedback,
        events,
        &temporary,
    );
    match result {
        Ok((frames, warning)) if frames > 0 => {
            let (file, warning) = match move_without_overwrite(&temporary, &config.destination) {
                Ok(()) => (config.destination.clone(), warning),
                Err(error) => (
                    temporary.clone(),
                    Some(format!(
                        "{}The video is saved here because its destination could not be used: {error}",
                        warning
                            .map(|warning| format!("{warning} "))
                            .unwrap_or_default()
                    )),
                ),
            };
            events(RecordingEvent::Finished {
                file: Some(file),
                error: warning,
            });
        }
        Ok((_, warning)) => {
            let _ = std::fs::remove_file(&temporary);
            events(RecordingEvent::Finished {
                file: None,
                error: warning.or_else(|| {
                    (!stop.load(Ordering::Acquire))
                        .then(|| "Recording stopped before any frames arrived.".into())
                }),
            });
        }
        Err(e) => {
            if std::fs::metadata(&temporary).is_ok_and(|metadata| metadata.len() == 0) {
                let _ = std::fs::remove_file(&temporary);
                return Err(e);
            }
            return Err(format!(
                "{e} Incomplete recording retained at {}",
                temporary.display()
            ));
        }
    }
    Ok(())
}

fn record_inner(
    source: &Source,
    config: &RecordingConfig,
    stop: &AtomicBool,
    preview: &crate::PreviewChannel,
    audio_feedback: &crate::AudioChannel,
    events: &impl Fn(RecordingEvent),
    temporary: &Path,
) -> Result<(u64, Option<String>), String> {
    let mut audio = if config.audio.enabled() {
        match crate::audio::AudioRecording::start(
            config.audio.clone(),
            temporary.with_extension("aac.partial"),
            audio_feedback.clone(),
            stop,
        ) {
            Ok(audio) => Some(audio),
            Err(_) if stop.load(Ordering::Acquire) => return Ok((0, None)),
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    if stop.load(Ordering::Acquire) {
        return Ok((0, None));
    }
    let native = || -> WinResult<_> {
        unsafe {
            let (device, context) = crate::gpu::create_device(config.gpu_index)?;
            let dxgi: IDXGIDevice = device.cast()?;
            #[cfg(feature = "diagnostics")]
            {
                let desc = dxgi.GetAdapter()?.GetDesc()?;
                let length = desc
                    .Description
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(desc.Description.len());
                println!(
                    "CAPTURE ADAPTER {}",
                    String::from_utf16_lossy(&desc.Description[..length])
                );
            }
            let winrt: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let mut manager = None;
            let mut token = 0;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(&device, token)?;
            Ok((device, context, winrt, manager))
        }
    };
    let (device, context, winrt, manager) =
        native().map_err(|e| format!("Could not initialize graphics: {e}"))?;
    let width = (source.width + 1) & !1;
    let height = (source.height + 1) & !1;
    let input_type =
        media_type(width, height, config.fps, &MFVideoFormat_NV12).map_err(|e| e.to_string())?;
    let mut encoder = VideoEncoder::new(
        &device,
        &context,
        &manager,
        temporary,
        &input_type,
        width,
        height,
        config,
    )?;
    let hardware = encoder.hardware();
    let mut preview_renderer = crate::preview::PreviewRenderer::default();
    let allocator = unsafe {
        let mut raw = std::ptr::null_mut();
        MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut raw)
            .map_err(|e| e.to_string())?;
        let allocator = IMFVideoSampleAllocatorEx::from_raw(raw);
        allocator
            .SetDirectXManager(&manager)
            .map_err(|e| e.to_string())?;
        let attrs = attributes(1).map_err(|e| e.to_string())?;
        attrs
            .SetUINT32(&MF_SA_D3D11_BINDFLAGS, D3D11_BIND_RENDER_TARGET.0 as u32)
            .map_err(|e| e.to_string())?;
        allocator
            .InitializeSampleAllocatorEx(4, 8, &attrs, &input_type)
            .map_err(|e| e.to_string())?;
        allocator
    };
    let mut converter = Converter::new(
        &device,
        &context,
        source.width,
        source.height,
        width,
        height,
        config.fps,
    )
    .map_err(|e| format!("GPU color conversion is unavailable: {e}"))?;
    let latest = Arc::new(std::sync::Mutex::new(None::<CapturedFrame>));
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        DirectXPixelFormat::R16G16B16A16Float,
        3,
        source.item.Size().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let session = pool
        .CreateCaptureSession(&source.item)
        .map_err(|e| e.to_string())?;
    let mut capture = Capture {
        pool,
        session,
        item: source.item.clone(),
        frame_token: None,
        closed_token: None,
    };
    let callback_latest = latest.clone();
    let frame_token = capture
        .pool
        .FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                if let Ok(frame) = pool.ok()?.TryGetNextFrame() {
                    let frame = CapturedFrame(frame);
                    if let Ok(mut latest) = callback_latest.lock() {
                        *latest = Some(frame);
                    }
                    let _ = wake_tx.try_send(());
                }
                Ok(())
            }),
        )
        .map_err(|e| e.to_string())?;
    capture.frame_token = Some(frame_token);
    let closed = Arc::new(AtomicBool::new(false));
    let callback_closed = closed.clone();
    let closed_token = source
        .item
        .Closed(
            &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                callback_closed.store(true, Ordering::Release);
                Ok(())
            }),
        )
        .map_err(|e| e.to_string())?;
    capture.closed_token = Some(closed_token);
    capture
        .session
        .SetIsCursorCaptureEnabled(config.capture_cursor)
        .map_err(|e| e.to_string())?;
    configure_capture_session(&capture.session)
        .map_err(|e| format!("Could not configure borderless recording: {e}"))?;
    capture.session.StartCapture().map_err(|e| e.to_string())?;
    let mut frame_wait_deadline = Instant::now() + Duration::from_secs(8);
    let mut clock: Option<Instant> = None;
    let mut index = 0;
    let mut written = 0;
    let mut video_end = 0;
    let mut dropped = 0;
    let mut last_statistics = Instant::now();
    let mut capture_size = source.item.Size().map_err(|e| e.to_string())?;
    let mut frame_valid = false;
    let mut color = crate::capture::DisplayColor::for_source(source);
    let mut last_tick = Instant::now();
    let gpu = unsafe {
        device
            .cast::<IDXGIDevice>()
            .and_then(|d| d.GetAdapter())
            .and_then(|a| a.GetDesc())
    }
    .map(|d| {
        String::from_utf16_lossy(&d.Description)
            .trim_end_matches('\0')
            .to_owned()
    })
    .unwrap_or_else(|_| "Unknown adapter".into());
    let mut started_event = Some(RecordingEvent::Started {
        hardware,
        bitrate_mbps: config.bitrate_for(
            match encoder.codec() {
                "AV1" => fastrecorder_core::Codec::Av1,
                "HEVC" => fastrecorder_core::Codec::Hevc,
                _ => fastrecorder_core::Codec::H264,
            },
            width,
            height,
        ),
        encoder: encoder.label().into(),
        codec: encoder.codec().into(),
        gpu,
        fallback: encoder.fallback().map(str::to_owned),
        audio: audio.as_ref().map_or_else(
            || "Audio disabled".into(),
            |audio| audio.description.clone(),
        ),
    });
    let loop_result = (|| -> Result<(), String> {
        while !stop.load(Ordering::Acquire) && !closed.load(Ordering::Acquire) {
            if written == 0 && Instant::now() > frame_wait_deadline {
                return Err(
                    "Recording could not start. Try another capture source or encoder.".into(),
                );
            }
            // Don't synthesize minutes of catch-up audio/video after sleep or a stalled driver.
            if last_tick.elapsed() > Duration::from_secs(2) {
                return Err("Recording stopped after the computer slept or capture was interrupted. The recorded portion will be saved.".into());
            }
            last_tick = Instant::now();
            if let Some(error) = audio.as_ref().and_then(|audio| audio.failure()) {
                return Err(error);
            }
            if frame_valid {
                let target = Duration::from_nanos(
                    (u128::from(index) * 1_000_000_000 / u128::from(config.fps)) as u64,
                );
                let delay = target.saturating_sub(clock.unwrap().elapsed());
                if !delay.is_zero() {
                    // High-refresh sources can wake us more often than output
                    // FPS. Keep their newest frame, but copy it only when due.
                    let _ = wake_rx.recv_timeout(delay.min(Duration::from_millis(20)));
                    continue;
                }
            }
            let frame = latest
                .lock()
                .map_err(|_| "Capture synchronization failed.".to_string())?
                .take();
            if let Some(frame) = frame {
                let content = frame.ContentSize().map_err(|e| e.to_string())?;
                if content.Width > 0 && content.Height > 0 {
                    if content != capture_size {
                        drop(frame);
                        capture
                            .pool
                            .Recreate(&winrt, DirectXPixelFormat::R16G16B16A16Float, 3, content)
                            .map_err(|e| e.to_string())?;
                        capture_size = content;
                        converter = Converter::new(
                            &device,
                            &context,
                            content.Width as u32,
                            content.Height as u32,
                            width,
                            height,
                            config.fps,
                        )
                        .map_err(|e| e.to_string())?;
                        frame_valid = false;
                        frame_wait_deadline = Instant::now() + Duration::from_secs(8);
                        continue;
                    }
                    if !converter
                        .copy_frame(&frame, color)
                        .map_err(|e| e.to_string())?
                    {
                        continue;
                    }
                    preview_renderer.update(
                        &device,
                        &context,
                        &converter.input,
                        content.Width as u32,
                        content.Height as u32,
                        preview,
                    );
                    frame_valid = true;
                    if clock.is_none() {
                        if let Some(audio) = &audio {
                            audio.begin()?;
                        }
                        clock = Some(Instant::now());
                    }
                }
            }
            if !frame_valid {
                if Instant::now() > frame_wait_deadline {
                    return Err(
                        "No frames arrived. The source may be minimized or unavailable.".into(),
                    );
                }
                let _ = wake_rx.recv_timeout(Duration::from_millis(20));
                continue;
            }
            let elapsed = clock.unwrap().elapsed();
            let due = frame_time(index, config.fps);
            if elapsed.as_nanos() / 100 >= due as u128 {
                // Never build a catch-up queue: skip missed ticks while preserving wall time.
                let now_index =
                    (elapsed.as_nanos() * u128::from(config.fps) / 1_000_000_000) as u64;
                if now_index > index && written > 0 {
                    dropped += now_index - index;
                    index = now_index;
                }
                let sample = unsafe { allocator.AllocateSample() };
                match sample {
                    Ok(sample) => {
                        converter.convert(&sample).map_err(|e| e.to_string())?;
                        let sample = if !encoder.cpu_input() {
                            sample
                        } else {
                            converter.cpu_sample(&sample).map_err(|e| e.to_string())?
                        };
                        unsafe {
                            sample
                                .SetSampleTime(frame_time(index, config.fps))
                                .map_err(|e| e.to_string())?;
                            sample
                                .SetSampleDuration(
                                    frame_time(index + 1, config.fps)
                                        - frame_time(index, config.fps),
                                )
                                .map_err(|e| e.to_string())?;
                            encoder.write(&sample, index, config.fps)?;
                        }
                        written += 1;
                        video_end = index + 1;
                        // Don't minimize the studio or report recording before
                        // capture supplies a frame and encoding accepts it.
                        if let Some(event) = started_event.take() {
                            events(event);
                        }
                    }
                    Err(e) if e.code() == MF_E_SAMPLEALLOCATOR_EMPTY => {
                        dropped += 1;
                    }
                    Err(e) => return Err(e.to_string()),
                }
                index += 1;
            }
            if last_statistics.elapsed() >= Duration::from_secs(2) {
                let directory = temporary
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                if available_disk_space(directory)? < 64 * 1024 * 1024 {
                    return Err(
                        "Recording stopped because the drive is running out of space.".into(),
                    );
                }
                unsafe { device.GetDeviceRemovedReason() }
                    .map_err(|e| format!("The graphics device was disconnected or reset: {e}"))?;
                color = crate::capture::DisplayColor::for_source(source);
                events(RecordingEvent::Statistics { dropped });
                last_statistics = Instant::now();
            }
            let target = Duration::from_nanos(
                (u128::from(index) * 1_000_000_000 / u128::from(config.fps)) as u64,
            );
            let sleep = target
                .saturating_sub(clock.unwrap().elapsed())
                .min(Duration::from_millis(20));
            match wake_rx.recv_timeout(sleep) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("Capture unexpectedly disconnected.".into());
                }
            }
        }
        if closed.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
            return Err("The capture source closed.".into());
        }
        Ok(())
    })();
    drop(capture);
    // Drop checked-out WGC frames before finalization and device shutdown.
    if let Ok(mut latest) = latest.lock() {
        *latest = None;
    }
    // Conversion and preview resources are no longer needed. Release them
    // before buffered encoders allocate/process their final tail at Stop.
    drop(converter);
    drop(preview_renderer);
    let audio = audio.as_mut().map(|audio| {
        audio.finish(video_end * u64::from(crate::audio::RATE) / u64::from(config.fps))
    });
    let audio_error = audio.as_ref().and_then(|audio| audio.error.clone());
    let warning = match (loop_result.err(), audio_error) {
        (Some(video), Some(audio)) if video != audio => Some(format!("{video}; {audio}")),
        (video, audio) => video.or(audio),
    };
    if written == 0 {
        return Ok((0, warning));
    }
    let finalization_warning = encoder
        .finalize(
            temporary,
            audio.as_ref().and_then(|audio| audio.track.as_ref()),
        )
        .map_err(|e| {
            format!(
                "{}Could not finish the MP4: {e}",
                warning
                    .as_ref()
                    .map(|e| format!("{e} "))
                    .unwrap_or_default()
            )
        })?;
    let warning = match (warning, finalization_warning) {
        (Some(recording), Some(finalization)) => Some(format!("{recording} {finalization}")),
        (recording, finalization) => recording.or(finalization),
    };
    Ok((written, warning))
}
