//! The studio window controller. Each submodule owns one area of the UI and
//! wires its Slint callbacks in an `install` function; `run` composes them.
mod audio;
mod destination;
#[cfg(feature = "diagnostics")]
mod diagnostics;
mod encoding;
mod notify;
mod preview;
mod recording;
mod settings;
mod shortcuts;
mod sources;
mod state;
mod surface;
mod theme;

use crate::{
    MainWindow, cli,
    preferences::Preferences,
    tray::{RecordingTray, TrayHandle},
};
use destination::assign_destination;
use fastrecorder_core::SessionState;
use fastrecorder_platform as native;
use notify::{hwnd, notify, notify_issue};
use settings::persist_preferences;
use slint::{CloseRequestResponse, ComponentHandle, Timer};
use sources::refresh_sources;
use state::{AppState, Lock};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
use surface::{install_surface_recovery, restore_studio};

pub fn run(ui: MainWindow) -> Result<(), Box<dyn std::error::Error>> {
    let _apartment = native::initialize_ui()?;
    install_surface_recovery(&ui);
    let state = Arc::new(Mutex::new(AppState::default()));
    match Preferences::load() {
        Ok(preferences) => {
            preferences.apply(&ui);
            let mut state = state.locked();
            state.preferred_encoder = Some(preferences.encoder.clamp(0, 8));
            state.preferred_gpu = preferences.gpu;
            state.desktop_device = preferences.desktop_device;
            state.microphone_device = preferences.microphone_device;
            if let Some(directory) = preferences.save_directory {
                if directory.is_dir() {
                    // A placeholder in the folder; assign_destination names the file.
                    state.destination = Some(directory.join("pending.mp4"));
                } else {
                    notify(
                        &ui,
                        "Your saved recording folder is unavailable. Using Videos/FastRecorder.",
                        false,
                    );
                }
            }
            if let Some(path) = preferences.last_recording.filter(|path| path.is_file()) {
                ui.set_has_recording(true);
                ui.set_last_recording(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                        .into(),
                );
                state.last_file = Some(path);
            }
        }
        Err(error) => notify_issue(&ui, "Couldn't load your saved settings.", error),
    }
    native::set_process_priority(ui.get_process_priority());
    let tray: TrayHandle = Rc::new(RefCell::new(None));
    let timer = recording::install(&ui, &state, &tray);
    ui.set_capture_supported(native::capture_supported());
    if !ui.get_capture_supported() {
        notify(
            &ui,
            "Windows screen capture isn't available on this device.",
            true,
        );
    }
    let audio_timer = audio::install(&ui, &state);

    let weak = ui.as_weak();
    ui.on_open_link(move |url| {
        if let Err(error) = native::open_url(&url)
            && let Some(ui) = weak.upgrade()
        {
            notify_issue(&ui, "Couldn't open the link.", error);
        }
    });

    let weak = ui.as_weak();
    ui.on_show_details(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_source_open(false);
            ui.invoke_cover_preview();
            ui.set_settings_open(true);
            ui.set_info_open(true);
        }
    });

    sources::install(&ui, &state);
    destination::install(&ui, &state);
    shortcuts::install(&ui, &state);
    let close_state = state.clone();
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        let mut state = close_state.locked();
        if state.session.state() == SessionState::Idle {
            if let Some(ui) = weak.upgrade() {
                persist_preferences(&ui, &state);
            }
            return CloseRequestResponse::HideWindow;
        }
        state.close_after_save = true;
        state.session.stop();
        if let Some(recording) = &state.recording {
            recording.stop();
        }
        if let Some(ui) = weak.upgrade() {
            ui.set_session_state(3);
            notify(&ui, "Saving before closing…", false);
        }
        CloseRequestResponse::KeepWindowShown
    });

    ui.show()?;
    assign_destination(&ui, &mut state.locked());
    let startup_state = state.clone();
    let startup_weak = ui.as_weak();
    let startup_tray = tray.clone();
    Timer::single_shot(Duration::from_millis(100), move || {
        let Some(ui) = startup_weak.upgrade() else {
            return;
        };
        match RecordingTray::new(&ui) {
            Ok(tray) => *startup_tray.borrow_mut() = Some(tray),
            Err(error) => {
                ui.set_auto_minimize(false);
                notify_issue(&ui, "Tray controls aren't available.", error);
            }
        }
        let state = startup_state;
        refresh_sources(&ui, &state);
        if let Ok(owner) = hwnd(&ui) {
            let own_window_diagnostic = cfg!(feature = "diagnostics")
                && (cli::flag("--self-test-record") || cli::flag("--profile-visible"));
            if !own_window_diagnostic && let Err(error) = native::exclude_from_capture(owner) {
                notify_issue(
                    &ui,
                    "The studio may appear in your recording.",
                    error.to_string(),
                );
            }
        }

        let primary = {
            let state = state.locked();
            state
                .sources
                .iter()
                .position(|source| source.primary)
                .or_else(|| state.sources.iter().position(|source| source.display))
        };
        if let Some(index) = primary {
            ui.invoke_select_source(index as i32);
        }
    });
    let weak = ui.as_weak();
    ui.on_tray_show(move || {
        if let Some(ui) = weak.upgrade() {
            restore_studio(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_tray_stop(move || {
        if let Some(ui) = weak.upgrade()
            && matches!(ui.get_session_state(), 1 | 2)
        {
            ui.invoke_toggle_recording();
        }
    });
    encoding::install(&ui, &state)?;
    let settings_timer = settings::install(&ui, &state);
    let preview_timer = preview::install(&ui, &state);
    let theme_timer = theme::install(&ui);
    #[cfg(feature = "diagnostics")]
    diagnostics::install(&ui, state.clone());
    slint::run_event_loop()?;
    timer.stop();
    settings_timer.stop();
    preview_timer.stop();
    theme_timer.stop();
    audio_timer.stop();
    if let Some(mut monitor) = state.locked().audio_monitor.take() {
        monitor.join();
    }
    if let Some(mut preview) = state.locked().preview.take() {
        preview.join();
    }
    if let Some(mut recording) = state.locked().recording.take() {
        recording.join();
    }
    #[cfg(feature = "diagnostics")]
    if let Some(error) = state.locked().diagnostic_failure.take() {
        return Err(error.into());
    }
    Ok(())
}
