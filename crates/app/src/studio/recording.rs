//! Starting/stopping recordings and applying backend events to the studio.
use super::{
    audio::audio_config,
    destination::assign_destination,
    encoding::{preference, selected_gpu, update_encoding_labels},
    notify::{notify, notify_issue},
    settings::persist_preferences,
    state::{Lock, Shared},
    surface::restore_studio,
};
use crate::{MainWindow, cli, tray::TrayHandle};
use fastrecorder_core::{EncoderPreference, RecordingConfig, SessionState};
use fastrecorder_platform::{self as native, Recording, RecordingEvent};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

/// Wires Record/Stop and the backend event queue. Returns the elapsed-time timer.
pub(super) fn install(ui: &MainWindow, state: &Shared, tray: &TrayHandle) -> Rc<Timer> {
    let (event_sender, event_receiver) = std::sync::mpsc::sync_channel(32);
    let event_tray = tray.clone();
    let event_state = state.clone();
    let weak = ui.as_weak();
    ui.on_drain_recording_events(move || {
        if let Some(ui) = weak.upgrade() {
            while let Ok(event) = event_receiver.try_recv() {
                handle_event(&ui, &event_tray, &event_state, event);
            }
        }
    });
    let timer = Rc::new(Timer::default());
    let weak = ui.as_weak();
    let recording_state = state.clone();
    let recording_timer = timer.clone();
    let recording_tray = tray.clone();
    ui.on_toggle_recording(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = recording_state.locked();
        if matches!(
            state.session.state(),
            SessionState::Starting | SessionState::Recording
        ) {
            if state.session.stop() {
                if let Some(recording) = &state.recording {
                    recording.stop();
                }
                ui.set_session_state(3);
                if let Some(tray) = recording_tray.borrow().as_ref() {
                    tray.update(SessionState::Stopping, ui.get_elapsed().as_str());
                }
                notify(&ui, "Saving…", false);
            }
            return;
        }
        if state.session.state() != SessionState::Idle {
            return;
        }
        if (ui.get_desktop_audio() || ui.get_microphone_audio())
            && (!ui.get_audio_ready()
                || ui.get_desktop_audio() && ui.get_desktop_audio_device() < 0
                || ui.get_microphone_audio() && ui.get_microphone_audio_device() < 0)
        {
            notify(
                &ui,
                "Choose connected audio devices in Settings → Audio, or disable audio.",
                true,
            );
            return;
        }
        if !state.custom_destination {
            assign_destination(&ui, &mut state);
        }
        let (Some(source), Some(destination)) = (state.source.clone(), state.destination.clone())
        else {
            return;
        };
        let source = if let Some(candidate) = &state.selected_candidate {
            match candidate.open() {
                Ok(source) => source,
                Err(error) => {
                    notify_issue(&ui, "Choose another screen or window.", error);
                    return;
                }
            }
        } else {
            source
        };
        update_encoding_labels(&ui, &state);
        if !ui.get_encoder_available() {
            notify(
                &ui,
                "The selected encoder is unavailable on this GPU / driver.",
                true,
            );
            return;
        }
        let config = RecordingConfig {
            destination,
            fps: ui.get_fps() as u32,
            max_height: ui.get_output_height() as u32,
            bitrate_mbps: ui.get_bitrate_mbps() as u32,
            bitrate_mode: ui.get_bitrate_mode() as u32,
            gpu_index: selected_gpu(&ui, &state),
            nvenc_preset: ui.get_nvenc_preset() as u32,
            keyframe_seconds: ui.get_keyframe_seconds() as u32,
            constant_bitrate: ui.get_constant_bitrate(),
            capture_cursor: ui.get_capture_cursor(),
            quality_qp: (ui.get_quality_mode() && ui.get_nvenc_controls())
                .then_some(ui.get_quality_level() as u32),
            prefer_h264: ui.get_prefer_h264(),
            tuning: fastrecorder_core::EncoderTuning {
                b_frames: u32::try_from(ui.get_b_frames()).ok(),
                lookahead: ui.get_lookahead(),
                spatial_aq: ui.get_spatial_aq(),
                temporal_aq: ui.get_temporal_aq(),
                multipass: match ui.get_multipass() {
                    0 => fastrecorder_core::Multipass::Off,
                    2 => fastrecorder_core::Multipass::FullResolution,
                    _ => fastrecorder_core::Multipass::QuarterResolution,
                },
                low_latency: ui.get_low_latency(),
                max_bitrate_mbps: ui.get_max_bitrate().max(0) as u32,
                software_speed: ui.get_software_speed().clamp(0, 10) as u32,
            },
            audio: {
                let diagnostic = cfg!(feature = "diagnostics")
                    && cli::flag("--self-test-record")
                    && !cli::flag("--with-audio");
                let mut config = audio_config(&ui, &state);
                config.desktop &= !diagnostic;
                config.microphone &= !diagnostic;
                config
            },
            encoder: if ui.get_software_encoder()
                || cfg!(feature = "diagnostics") && cli::flag("--software-encoder")
            {
                EncoderPreference::SoftwareOnly
            } else {
                preference(ui.get_encoder_choice())
            },
        };
        if let Err(e) = config.validate() {
            notify(&ui, e.clone(), true);
            #[cfg(feature = "diagnostics")]
            if state.diagnostic {
                state.diagnostic_failure = Some(e);
                let _ = slint::quit_event_loop();
            }
            return;
        }
        // A terminal event means media finalization is complete. Retiring COM
        // resources must not block the studio while the next recording starts.
        state.recording.take();
        state.preview.take();
        // Separate channels prevent a retiring preview from publishing stale pixels.
        state.preview_channel = native::PreviewChannel::default();
        state.preview_channel.set_cursor(ui.get_capture_cursor());
        state
            .preview_channel
            .set_enabled(!ui.window().is_minimized() && ui.get_preview_enabled());
        ui.set_preview_ready(false);
        ui.set_source_open(false);
        ui.set_settings_open(false);
        ui.set_info_open(false);
        ui.set_source_detail(format!("{} × {}", source.width, source.height).into());
        state.source = Some(source.clone());
        ui.set_active_encoder("Starting…".into());
        ui.set_audio_details("Starting…".into());
        ui.set_recording_stats("0 frames skipped".into());
        state.session.begin();
        if let Some(tray) = recording_tray.borrow().as_ref() {
            tray.update(SessionState::Starting, "00:00");
        }
        ui.set_session_state(1);
        ui.set_elapsed("00:00".into());
        ui.set_window_title("Pip · Starting recording".into());
        ui.set_celebrate(false);
        notify(&ui, "", false);
        let event_weak = ui.as_weak();
        let preview_channel = state.preview_channel.clone();
        let audio_monitor = state.audio_monitor.take();
        state.meter_config = None;
        state.audio_channel = native::AudioChannel::default();
        ui.set_desktop_audio_error("".into());
        ui.set_microphone_audio_error("".into());
        let audio_channel = state.audio_channel.clone();
        let event_sender = event_sender.clone();
        match Recording::start(
            source,
            config,
            preview_channel,
            audio_channel,
            audio_monitor,
            move |event| {
                match event_sender.try_send(event) {
                    Ok(()) => {}
                    Err(std::sync::mpsc::TrySendError::Full(RecordingEvent::Statistics {
                        ..
                    })) => {
                        return;
                    }
                    Err(std::sync::mpsc::TrySendError::Full(event)) => {
                        if event_sender.send(event).is_err() {
                            return;
                        }
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
                }
                let _ = event_weak.upgrade_in_event_loop(|ui| ui.invoke_drain_recording_events());
            },
        ) {
            Ok(recording) => state.recording = Some(recording),
            Err(e) => {
                state.session.finished();
                if let Some(tray) = recording_tray.borrow().as_ref() {
                    tray.update(SessionState::Idle, "");
                }
                ui.set_session_state(0);
                ui.set_window_title("Pip".into());
                if let Some(source) = state.source.clone() {
                    state.preview =
                        native::Preview::start(source, state.preview_channel.clone()).ok();
                }
                notify_issue(&ui, "Couldn't start recording.", e);
            }
        }
        let clock_tray = recording_tray.clone();
        let timer_weak = ui.as_weak();
        let timer_state = recording_state.clone();
        let weak_timer = std::rc::Rc::downgrade(&recording_timer);
        recording_timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            let Some(ui) = timer_weak.upgrade() else {
                return;
            };
            let state = timer_state.locked();
            if state.session.state() == SessionState::Idle {
                if let Some(timer) = weak_timer.upgrade() {
                    timer.stop();
                }
            } else if let Some(started) = state.started {
                let seconds = started.elapsed().as_secs();
                let elapsed: slint::SharedString =
                    format!("{:02}:{:02}", seconds / 60, seconds % 60).into();
                ui.set_elapsed(elapsed.clone());
                ui.set_window_title(
                    format!(
                        "Pip · {} {elapsed}",
                        if state.session.state() == SessionState::Stopping {
                            "Saving"
                        } else {
                            "Recording"
                        }
                    )
                    .into(),
                );
                if let Some(tray) = clock_tray.borrow().as_ref() {
                    tray.update(state.session.state(), elapsed.as_str());
                }
                let limit = u64::try_from(ui.get_auto_stop_minutes()).unwrap_or(0) * 60;
                if limit > 0 && seconds >= limit && state.session.state() == SessionState::Recording
                {
                    // Stop outside this callback: toggling takes the state lock.
                    let weak = ui.as_weak();
                    Timer::single_shot(Duration::ZERO, move || {
                        if let Some(ui) = weak.upgrade()
                            && ui.get_session_state() == 2
                        {
                            ui.invoke_toggle_recording();
                        }
                    });
                }
            }
        });
    });
    timer
}

fn handle_event(ui: &MainWindow, tray: &TrayHandle, event_state: &Shared, event: RecordingEvent) {
    let mut state = event_state.locked();
    match event {
        RecordingEvent::Started {
            hardware: _,
            encoder,
            gpu,
            codec,
            fallback,
            bitrate_mbps,
            audio,
        } => {
            ui.set_bitrate_mbps(bitrate_mbps as i32);
            state.session.started();
            if matches!(
                state.session.state(),
                SessionState::Starting | SessionState::Recording
            ) {
                ui.set_session_state(2);
                if ui.get_auto_minimize() && tray.borrow().is_some() {
                    ui.window().set_minimized(true);
                }
            }
            if let Some(tray) = tray.borrow().as_ref() {
                tray.update(state.session.state(), "00:00");
            }
            state.started = Some(Instant::now());
            ui.set_encoder_label(encoder.clone().into());
            ui.set_gpu_label(gpu.clone().into());
            ui.set_codec_label(codec.clone().into());
            ui.set_audio_details(audio.into());
            let rate = if encoder.starts_with("NVIDIA") && ui.get_quality_mode() {
                format!("CQP {} · variable bitrate", ui.get_quality_level())
            } else {
                format!("{bitrate_mbps} Mbps target")
            };
            ui.set_active_encoder(
                format!(
                    "{encoder}\nGPU: {gpu}\nCodec: {codec} · {rate} · {} fps",
                    ui.get_fps()
                )
                .into(),
            );
            ui.set_fallback_detail(fallback.clone().unwrap_or_else(|| "None".into()).into());
            notify(ui, "", false);
            #[cfg(feature = "diagnostics")]
            println!("ENCODER {encoder}");
        }
        RecordingEvent::Statistics { dropped } => {
            ui.set_recording_stats(format!("{dropped} frames skipped").into());
            if dropped > 0 && state.session.state() == SessionState::Recording {
                notify(
                    ui,
                    "Recording is struggling to keep up. Try 30 fps or a smaller capture area.",
                    false,
                );
            }
        }
        RecordingEvent::Finished { file, error } => {
            let never_started = state.started.is_none();
            let cancelled = state.session.state() == SessionState::Stopping
                && file.is_none()
                && error.is_none();
            state.session.finished();
            state.recording.take();
            state.started = None;
            ui.set_session_state(0);
            ui.set_window_title("Pip".into());
            if let Some(tray) = tray.borrow().as_ref() {
                tray.update(SessionState::Idle, "");
            }
            ui.set_preview_ready(false);
            ui.set_preview_image(slint::Image::default());
            if !state.close_after_save {
                restore_studio(ui);
            }
            state.preview_channel = native::PreviewChannel::default();
            state.preview_channel.set_cursor(ui.get_capture_cursor());
            if let Some(candidate) = &state.selected_candidate {
                match candidate.open() {
                    Ok(source) => state.source = Some(source),
                    Err(_) => {
                        state.source = None;
                        state.selected_candidate = None;
                        ui.set_source_selected(false);
                        ui.set_selected_source(-1);
                        ui.set_preview_error("The source closed. Choose another source.".into());
                    }
                }
            }
            if !state.close_after_save
                && let Some(source) = state.source.clone()
            {
                match native::Preview::start(source, state.preview_channel.clone()) {
                    Ok(preview) => state.preview = Some(preview),
                    Err(error) => ui.set_preview_error(error.into()),
                }
            }
            if let Some(file) = file {
                ui.set_last_recording(
                    file.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                        .into(),
                );
                ui.set_has_recording(true);
                state.last_file = Some(file.clone());
                assign_destination(ui, &mut state);
                persist_preferences(ui, &state);
                if let Some(error) = &error {
                    notify_issue(ui, "Recording saved with an issue.", error.as_str());
                } else {
                    notify(ui, "Saved! Nice recording.", false);
                    ui.set_celebrate(true);
                    if !state.close_after_save {
                        let after = match ui.get_after_save() {
                            1 => native::reveal_recording(&file),
                            2 => native::open_recording(&file),
                            _ => Ok(()),
                        };
                        if let Err(error) = after {
                            notify_issue(ui, "Saved, but couldn't open it.", error);
                        }
                    }
                }
                #[cfg(feature = "diagnostics")]
                println!("SAVED {}", file.display());
            } else {
                if let Some(error) = &error {
                    notify_issue(
                        ui,
                        if never_started {
                            "Couldn't start recording."
                        } else {
                            "Couldn't save the recording."
                        },
                        error.as_str(),
                    );
                } else {
                    notify(
                        ui,
                        if cancelled {
                            "Recording cancelled."
                        } else {
                            "No recording was saved."
                        },
                        !cancelled,
                    );
                }
                #[cfg(feature = "diagnostics")]
                eprintln!(
                    "RECORDING FAILED: {}",
                    error.as_deref().unwrap_or("No file")
                );
                #[cfg(feature = "diagnostics")]
                if state.diagnostic {
                    state.diagnostic_failure = error;
                }
            }
            #[cfg(feature = "diagnostics")]
            if state.diagnostic {
                let _ = slint::quit_event_loop();
            }
            if state.close_after_save {
                let _ = slint::quit_event_loop();
            }
        }
    }
}
