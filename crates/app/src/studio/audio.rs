//! Audio device inventory, idle level meters and audio configuration.
use super::{
    notify::notify_issue,
    state::{AppState, Lock, Shared},
};
use crate::MainWindow;
use fastrecorder_core::SessionState;
use fastrecorder_platform as native;
use slint::{ComponentHandle, Timer, TimerMode};
use std::time::Duration;

pub(super) fn refresh_audio_devices(ui: &MainWindow, state: &Shared) {
    if ui.get_session_state() != 0 {
        return;
    }
    let generation = {
        let mut state = state.locked();
        state.audio_refresh += 1;
        state.audio_monitor.take();
        state.audio_channel = native::AudioChannel::default();
        state.meter_config = None;
        state.audio_refresh
    };
    ui.set_audio_ready(false);
    ui.set_desktop_audio_error("".into());
    ui.set_microphone_audio_error("".into());
    ui.set_audio_status("Refreshing audio devices…".into());
    let weak = ui.as_weak();
    let state = state.clone();
    let result = std::thread::Builder::new()
        .name("fastrecorder-audio-devices".into())
        .spawn(move || {
            let result = native::audio_inventory();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if state.locked().audio_refresh != generation {
                    return;
                }
                match result {
                    Ok(inventory) => {
                        let mut state = state.locked();
                        let names = |default: &str, devices: &[native::AudioDevice]| {
                            std::rc::Rc::new(slint::VecModel::from(
                                std::iter::once(slint::SharedString::from(default))
                                    .chain(devices.iter().map(|device| device.name.clone().into()))
                                    .collect::<Vec<_>>(),
                            ))
                            .into()
                        };
                        ui.set_desktop_audio_devices(names(
                            "System default playback device",
                            &inventory.desktop,
                        ));
                        ui.set_microphone_audio_devices(names(
                            "System default microphone",
                            &inventory.microphones,
                        ));
                        let index = |id: &Option<String>, devices: &[native::AudioDevice]| {
                            id.as_ref().map_or(0, |id| {
                                devices
                                    .iter()
                                    .position(|device| &device.id == id)
                                    .map_or(-1, |index| index as i32 + 1)
                            })
                        };
                        ui.set_desktop_audio_device(index(
                            &state.desktop_device,
                            &inventory.desktop,
                        ));
                        ui.set_microphone_audio_device(index(
                            &state.microphone_device,
                            &inventory.microphones,
                        ));
                        state.desktop_devices = inventory.desktop;
                        state.microphone_devices = inventory.microphones;
                        let mut messages = Vec::new();
                        if ui.get_desktop_audio_device() < 0 {
                            messages
                                .push("Saved playback device disconnected. Select another device.");
                        }
                        if ui.get_microphone_audio_device() < 0 {
                            messages.push("Saved microphone disconnected. Select another device.");
                        }
                        if state.desktop_devices.is_empty() {
                            messages.push("No connected playback devices.");
                        }
                        if state.microphone_devices.is_empty() {
                            messages.push("No connected microphones.");
                        }
                        ui.set_audio_status(messages.join("\n").into());
                        ui.set_audio_ready(true);
                    }
                    Err(error) => {
                        ui.set_audio_status(error.clone().into());
                        notify_issue(&ui, "Couldn't find audio devices.", error);
                    }
                }
            });
        });
    if let Err(error) = result {
        ui.set_audio_status(format!("Could not refresh audio devices: {error}").into());
    }
}

pub(super) fn audio_config(ui: &MainWindow, state: &AppState) -> fastrecorder_core::AudioConfig {
    fastrecorder_core::AudioConfig {
        desktop: ui.get_desktop_audio(),
        microphone: ui.get_microphone_audio(),
        desktop_device: state.desktop_device.clone(),
        microphone_device: state.microphone_device.clone(),
        desktop_volume: ui.get_desktop_volume() as u32,
        microphone_volume: ui.get_microphone_volume() as u32,
    }
}

pub(super) fn update_audio_feedback(ui: &MainWindow, state: &mut AppState) {
    let idle = state.session.state() == SessionState::Idle;
    if idle {
        let visible = !ui.window().is_minimized() && ui.window().is_visible();
        let config = audio_config(ui, state);
        let requested = (visible && ui.get_audio_ready() && config.enabled()).then_some(config);
        if requested != state.meter_config {
            state.audio_monitor.take(); // Signal stop; device teardown stays off the UI thread.
            state.audio_channel = native::AudioChannel::default();
            state.meter_config = requested.clone();
            ui.set_desktop_audio_error("".into());
            ui.set_microphone_audio_error("".into());
            if let Some(mut config) = requested {
                // A missing saved endpoint must never silently meter the default device.
                if config.desktop && ui.get_desktop_audio_device() < 0 {
                    config.desktop = false;
                    ui.set_desktop_audio_error(
                        "Saved playback device is disconnected. Choose another device.".into(),
                    );
                }
                if config.microphone && ui.get_microphone_audio_device() < 0 {
                    config.microphone = false;
                    ui.set_microphone_audio_error(
                        "Saved microphone is disconnected. Choose another device.".into(),
                    );
                }
                match native::AudioMonitor::start(config, state.audio_channel.clone()) {
                    Ok(monitor) => state.audio_monitor = Some(monitor),
                    Err(error) => ui.set_audio_status(error.into()),
                }
            }
        }
    }
    let peaks = state.audio_channel.take_peaks();
    let level = |peak: f32, previous: f32, enabled: bool| -> f32 {
        if !enabled {
            return 0.;
        }
        let measured = if peak > 0.001 {
            ((20. * peak.log10() + 60.) / 60.).clamp(0., 1.)
        } else {
            0.
        };
        measured.max(previous * 0.82)
    };
    ui.set_desktop_level(level(
        peaks[0],
        ui.get_desktop_level(),
        ui.get_desktop_audio(),
    ));
    ui.set_microphone_level(level(
        peaks[1],
        ui.get_microphone_level(),
        ui.get_microphone_audio(),
    ));
    let errors = state.audio_channel.errors();
    if let Some(error) = &errors[0] {
        ui.set_desktop_audio_error(error.clone().into());
    }
    if let Some(error) = &errors[1] {
        ui.set_microphone_audio_error(error.clone().into());
    }
}

/// Wires device refresh and starts the 10 Hz meter timer.
pub(super) fn install(ui: &MainWindow, state: &Shared) -> Timer {
    let audio_state = state.clone();
    let weak = ui.as_weak();
    ui.on_refresh_audio_devices(move || {
        if let Some(ui) = weak.upgrade() {
            refresh_audio_devices(&ui, &audio_state);
        }
    });
    refresh_audio_devices(ui, state);
    let audio_timer = Timer::default();
    let audio_weak = ui.as_weak();
    let audio_state = state.clone();
    audio_timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
        if let Some(ui) = audio_weak.upgrade() {
            update_audio_feedback(&ui, &mut audio_state.locked());
        }
    });
    audio_timer
}
