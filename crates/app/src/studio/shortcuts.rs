//! Global start/stop shortcuts.
use super::{
    settings::persist_preferences,
    state::{Lock, Shared},
};
use crate::{MainWindow, cli};
use fastrecorder_platform as native;
use slint::ComponentHandle;

pub(super) fn register_shortcuts(
    ui: &MainWindow,
    start: &str,
    stop: &str,
) -> Result<native::RecordingShortcut, String> {
    let weak = ui.as_weak();
    native::RecordingShortcut::register(start, stop, move |action| {
        let _ = weak.upgrade_in_event_loop(move |ui| {
            // Counting down counts as "starting": Stop cancels it, Start is ignored.
            let can_stop = matches!(ui.get_session_state(), 1 | 2) || ui.get_counting_down();
            let apply = match action {
                native::ShortcutAction::Start => {
                    ui.get_can_start_recording() && !ui.get_counting_down()
                }
                native::ShortcutAction::Stop => can_stop,
                native::ShortcutAction::Toggle => can_stop || ui.get_can_start_recording(),
            };
            if apply {
                ui.invoke_record_pressed();
            }
        });
    })
}
pub(super) fn shortcut_status(start: &str, stop: &str) -> String {
    let name = |text: &str| {
        if text.trim().is_empty() {
            "Disabled".to_string()
        } else {
            text.to_string()
        }
    };
    format!("Start: {}\nStop: {}", name(start), name(stop))
}

/// Registers the saved shortcuts and wires Apply.
pub(super) fn install(ui: &MainWindow, state: &Shared) {
    let shortcuts = std::rc::Rc::new(std::cell::RefCell::new(None));
    let diagnostic_shortcuts = cli::flag("--snapshot") || cli::flag("--self-test-record");
    let start = ui.get_active_start_shortcut().to_string();
    let stop = ui.get_active_stop_shortcut().to_string();
    if !diagnostic_shortcuts {
        match register_shortcuts(ui, &start, &stop) {
            Ok(shortcut) => {
                *shortcuts.borrow_mut() = Some(shortcut);
                ui.set_shortcut_status(shortcut_status(&start, &stop).into());
            }
            Err(error) => ui.set_shortcut_status(format!("Shortcuts inactive: {error}").into()),
        }
    }
    let shortcut_holder = shortcuts.clone();
    let shortcut_state = state.clone();
    let weak = ui.as_weak();
    ui.on_apply_shortcuts(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_session_state() != 0 || diagnostic_shortcuts {
            return;
        }
        let start = ui.get_start_shortcut().to_string();
        let stop = ui.get_stop_shortcut().to_string();
        if let Err(error) = native::RecordingShortcut::validate(&start, &stop) {
            ui.set_shortcut_status(error.into());
            return;
        }
        let previous_start = ui.get_active_start_shortcut().to_string();
        let previous_stop = ui.get_active_stop_shortcut().to_string();
        let had_shortcuts = shortcut_holder.borrow_mut().take().is_some();
        match register_shortcuts(&ui, &start, &stop) {
            Ok(shortcut) => {
                *shortcut_holder.borrow_mut() = Some(shortcut);
                ui.set_active_start_shortcut(start.clone().into());
                ui.set_active_stop_shortcut(stop.clone().into());
                ui.set_shortcut_status(shortcut_status(&start, &stop).into());
                persist_preferences(&ui, &shortcut_state.locked());
            }
            Err(error) => {
                let restored = if had_shortcuts {
                    match register_shortcuts(&ui, &previous_start, &previous_stop) {
                        Ok(shortcut) => {
                            *shortcut_holder.borrow_mut() = Some(shortcut);
                            format!(
                                "Previous shortcuts remain active.\n{}",
                                shortcut_status(&previous_start, &previous_stop)
                            )
                        }
                        Err(restore_error) => format!("Shortcuts inactive: {restore_error}"),
                    }
                } else {
                    "Shortcuts inactive.".into()
                };
                ui.set_shortcut_status(format!("{error}\n{restored}").into());
            }
        }
    });
}
