//! Follows the Windows light/dark setting and keeps the title bar in step.
use super::notify::hwnd;
use crate::MainWindow;
use fastrecorder_platform as native;
use slint::{ComponentHandle, Timer, TimerMode};
use std::{cell::Cell, rc::Rc, time::Duration};

/// Polls the system theme (a registry read) and applies the effective theme to
/// the native title bar. Returns the polling timer.
pub(super) fn install(ui: &MainWindow) -> Rc<Timer> {
    ui.set_system_dark(native::system_prefers_dark());
    let timer = Rc::new(Timer::default());
    let applied: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
    let weak = ui.as_weak();
    let sync = move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let system = native::system_prefers_dark();
        if ui.get_system_dark() != system {
            ui.set_system_dark(system);
        }
        let dark = ui.get_dark_mode();
        if applied.get() != Some(dark)
            && let Ok(window) = hwnd(&ui)
        {
            native::set_dark_titlebar(window, dark);
            applied.set(Some(dark));
        }
    };
    sync();
    timer.start(TimerMode::Repeated, Duration::from_millis(750), sync);
    timer
}
