//! Persisting preferences and reacting to settings edits.
use super::{
    encoding::update_encoding_labels,
    notify::notify_issue,
    state::{AppState, Lock, Shared},
};
use crate::{MainWindow, cli};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{rc::Rc, time::Duration};

pub(super) fn persist_preferences(ui: &MainWindow, state: &AppState) {
    if !ui.get_hardware_ready()
        || (cli::flag("--snapshot")
            || cli::flag("--self-test-record")
            || cli::flag("--profile-record"))
    {
        return;
    }
    let gpu = (ui.get_gpu_choice() > 0)
        .then(|| {
            state
                .gpus
                .get((ui.get_gpu_choice() - 1) as usize)
                .map(|gpu| gpu.name.clone())
        })
        .flatten();
    let prefs = crate::preferences::Preferences::capture(
        ui,
        state.destination.as_ref().and_then(|p| p.parent()),
        state.last_file.as_deref(),
        gpu,
        state.desktop_device.clone(),
        state.microphone_device.clone(),
    );
    if let Err(error) = prefs.save() {
        notify_issue(ui, "Couldn't save your settings.", error);
    }
}

/// Debounces saves while the user edits settings. Returns the save timer.
pub(super) fn install(ui: &MainWindow, state: &Shared) -> Rc<Timer> {
    let weak = ui.as_weak();
    let settings_state = state.clone();
    let settings_timer = std::rc::Rc::new(Timer::default());
    let settings_save_timer = settings_timer.clone();
    ui.on_settings_changed(move || {
        if let Some(ui) = weak.upgrade() {
            if ui.get_session_state() != 0 {
                return;
            }
            let mut state = settings_state.locked();
            if ui.get_audio_ready() && ui.get_desktop_audio_device() == 0 {
                state.desktop_device = None;
            } else if let Some(device) = usize::try_from(ui.get_desktop_audio_device() - 1)
                .ok()
                .and_then(|index| state.desktop_devices.get(index))
            {
                state.desktop_device = Some(device.id.clone());
            }
            if ui.get_audio_ready() && ui.get_microphone_audio_device() == 0 {
                state.microphone_device = None;
            } else if let Some(device) = usize::try_from(ui.get_microphone_audio_device() - 1)
                .ok()
                .and_then(|index| state.microphone_devices.get(index))
            {
                state.microphone_device = Some(device.id.clone());
            }
            state.preview_channel.set_cursor(ui.get_capture_cursor());
            fastrecorder_platform::set_process_priority(ui.get_process_priority());
            update_encoding_labels(&ui, &state);
            if !state.custom_destination {
                super::destination::assign_destination(&ui, &mut state);
            }
            drop(state);
            let save_weak = ui.as_weak();
            let save_state = settings_state.clone();
            settings_save_timer.start(
                TimerMode::SingleShot,
                Duration::from_millis(500),
                move || {
                    if let Some(ui) = save_weak.upgrade() {
                        persist_preferences(&ui, &save_state.locked());
                    }
                },
            );
        }
    });
    settings_timer
}
