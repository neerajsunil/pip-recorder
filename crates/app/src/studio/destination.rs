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
use std::path::{Path, PathBuf};

/// A short folder name for the studio, e.g. "Videos › FastRecorder" inside the
/// user's profile; other locations keep their full path.
fn friendly_folder(directory: &Path) -> String {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| directory.strip_prefix(home).ok())
    {
        Some(relative) if relative.components().next().is_some() => relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" › "),
        _ => directory.display().to_string(),
    }
}

fn show_folder(ui: &MainWindow, directory: &Path) {
    ui.set_save_folder(friendly_folder(directory).into());
    ui.set_save_folder_path(directory.display().to_string().into());
}

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
            let path = native::timestamped_destination(
                &directory,
                &ui.get_file_name_pattern(),
                &ui.get_source_name(),
            );
            show_folder(ui, &directory);
            state.custom_destination = false;
            ui.set_destination_label(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
                    .into(),
            );
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
                notify_issue(&ui, "Couldn't open the folder picker.", e);
                return;
            }
        };
        let current = destination_state
            .locked()
            .destination
            .as_ref()
            .and_then(|path| path.parent())
            .map(PathBuf::from);
        match native::choose_folder(owner, current.as_deref()) {
            Ok(Some(directory)) => {
                let mut state = destination_state.locked();
                state.destination = Some(directory.join("pending.mp4"));
                assign_destination(&ui, &mut state);
                persist_preferences(&ui, &state);
                notify(&ui, "New recordings will be saved here.", false);
            }
            Ok(None) => {}
            Err(e) => notify_issue(&ui, "Couldn't choose a save folder.", e),
        }
    });

    let folder_state = state.clone();
    let weak = ui.as_weak();
    ui.on_open_folder(move || {
        let directory = folder_state
            .locked()
            .destination
            .as_ref()
            .and_then(|path| path.parent())
            .map(PathBuf::from);
        if let Some(directory) = directory
            && let Err(e) = native::open_folder(&directory)
            && let Some(ui) = weak.upgrade()
        {
            notify_issue(&ui, "Couldn't open the save folder.", e);
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
