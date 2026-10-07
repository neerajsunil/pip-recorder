//! Pulls preview frames from the backend while the studio is visible.
use super::{
    encoding::update_encoding_labels,
    state::{Lock, Shared},
};
use crate::MainWindow;
use fastrecorder_core::SessionState;
use slint::{ComponentHandle, Timer, TimerMode};
use std::time::Duration;

/// Starts the ~12 fps preview pump. Returns its timer.
pub(super) fn install(ui: &MainWindow, state: &Shared) -> Timer {
    let preview_timer = Timer::default();
    let weak = ui.as_weak();
    let preview_state = state.clone();
    preview_timer.start(TimerMode::Repeated, Duration::from_millis(83), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let channel = preview_state.locked().preview_channel.clone();
        let visible = !ui.window().is_minimized()
            && ui.get_preview_enabled()
            && !ui.get_settings_open()
            && !ui.get_source_open();
        channel.set_enabled(visible);
        if !visible {
            return;
        }
        match channel.take() {
            Some(Ok(frame)) => {
                let mut state = preview_state.locked();
                if state.session.state() == SessionState::Idle
                    && let Some(source) = state.source.as_mut()
                    && (source.width != frame.source_width || source.height != frame.source_height)
                {
                    source.width = frame.source_width;
                    source.height = frame.source_height;
                    ui.set_source_detail(format!("{} × {}", source.width, source.height).into());
                    update_encoding_labels(&ui, &state);
                }
                drop(state);
                let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &frame.rgba,
                    frame.width,
                    frame.height,
                );
                ui.set_preview_image(slint::Image::from_rgba8(pixels));
                ui.set_preview_ready(true);
                ui.set_preview_error("".into());
            }
            Some(Err(error)) => {
                ui.set_preview_ready(false);
                ui.set_preview_error(error.into());
            }
            None => {}
        }
    });
    preview_timer
}
