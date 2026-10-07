//! Where recordings are saved, and opening/revealing the last one.
use super::{
    notify::{hwnd, notify, notify_issue},
    settings::persist_preferences,
    state::{AppState, Lock, Shared},
};
use crate::MainWindow;
use fastrecorder_core::SessionState;
use fastrecorder_platform as native;
use slint::ComponentHandle;
use std::path::PathBuf;

pub(super) fn assign_destination(ui: &MainWindow, state: &mut AppState) {
    let directory = state
        .destination
        .as_ref()
        .or(state.last_file.as_ref())
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(native::default_recording_directory);
    match directory {
        Ok(directory) => {
            let path = native::timestamped_destination(&directory);
            state.custom_destination = false;
            ui.set_destination_label(path.to_string_lossy().into_owned().into());
            ui.set_destination_selected(true);
            state.destination = Some(path);
        }
        Err(error) => notify_issue(ui, "Couldn't prepare the recording folder.", error),
    }
}

pub(super) fn install(ui: &MainWindow, state: &Shared) {
    let weak = ui.as_weak();
    let destination_state = state.clone();
    ui.on_choose_destination(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if destination_state.locked().session.state() != SessionState::Idle {
            return;
        }
        let owner = match hwnd(&ui) {
            Ok(owner) => owner,
            Err(e) => {
                notify_issue(&ui, "Couldn't open the save dialog.", e);
                return;
            }
        };
        let suggested = native::timestamped_destination(&PathBuf::new());
        let suggested = suggested.file_name().unwrap().to_string_lossy();
        match native::choose_destination(owner, &suggested) {
            Ok(Some(path)) => {
                if path.exists() {
                    notify(
                        &ui,
                        "Choose a new filename to keep your existing recording.",
                        true,
                    );
                    return;
                }
                ui.set_destination_label(path.to_string_lossy().into_owned().into());
                ui.set_destination_selected(true);
                let mut state = destination_state.locked();
                state.destination = Some(path);
                state.custom_destination = true;
                persist_preferences(&ui, &state);
                notify(&ui, "", false);
            }
            Ok(None) => {}
            Err(e) => notify_issue(&ui, "Couldn't choose a save location.", e),
        }
    });

    let reveal_state = state.clone();
    let weak = ui.as_weak();
    ui.on_reveal_recording(move || {
        let file = reveal_state.locked().last_file.clone();
        if let Some(file) = file
            && let Err(e) = native::reveal_recording(&file)
            && let Some(ui) = weak.upgrade()
        {
            notify_issue(&ui, "Couldn't open the recording folder.", e);
        }
    });
    let open_state = state.clone();
    let weak = ui.as_weak();
    ui.on_open_recording(move || {
        let file = open_state.locked().last_file.clone();
        if let Some(file) = file
            && let Err(error) = native::open_recording(&file)
            && let Some(ui) = weak.upgrade()
        {
            notify_issue(&ui, "Couldn't open the video player.", error);
        }
    });
}
