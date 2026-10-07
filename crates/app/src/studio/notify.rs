//! Notices, issue details and the native window handle.
use crate::MainWindow;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;

pub(super) fn hwnd(ui: &MainWindow) -> Result<usize, String> {
    let handle = ui.window().window_handle();
    match handle.window_handle().map_err(|e| e.to_string())?.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as usize),
        _ => Err("The native window is unavailable.".into()),
    }
}

pub(super) fn notify(ui: &MainWindow, message: impl Into<slint::SharedString>, error: bool) {
    let message = message.into();
    if error {
        ui.set_issue_detail(message.clone());
    }
    ui.set_notice(message);
    ui.set_notice_error(error);
}

pub(super) fn notify_issue(ui: &MainWindow, summary: &str, detail: impl std::fmt::Display) {
    let detail = detail.to_string();
    crate::logging::write(&format!("{summary} {detail}"));
    notify(ui, summary, true);
    ui.set_issue_detail(detail.into());
}
