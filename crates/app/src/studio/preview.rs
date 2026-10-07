//! Places the native preview surface over the studio's preview area and turns
//! preview news into UI state. Frames themselves are presented by the capture
//! GPU; only the CPU fallback (no surface yet) carries pixels.
use super::{
    encoding::update_encoding_labels,
    notify::hwnd,
    state::{Lock, Shared},
};
use crate::MainWindow;
use fastrecorder_core::SessionState;
use fastrecorder_platform as native;
use slint::{ComponentHandle, Timer, TimerMode};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// Starts the preview pump. Returns its timer.
pub(super) fn install(ui: &MainWindow, state: &Shared) -> Timer {
    let surface: Rc<RefCell<Option<native::PreviewSurface>>> = Rc::new(RefCell::new(None));
    // Hide the native preview synchronously, before a panel or the countdown is
    // painted over it. The timer below shows it again once nothing covers it.
    let cover_surface = surface.clone();
    ui.on_cover_preview(move || {
        if let Some(surface) = cover_surface.borrow_mut().as_mut() {
            surface.hide();
        }
    });
    let timer = Timer::default();
    let weak = ui.as_weak();
    let preview_state = state.clone();
    timer.start(TimerMode::Repeated, Duration::from_millis(83), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let visible = !ui.window().is_minimized()
            && ui.get_preview_enabled()
            && !ui.get_settings_open()
            && !ui.get_source_open();
        if surface.borrow().is_none()
            && let Ok(owner) = hwnd(&ui)
        {
            match native::PreviewSurface::new(owner) {
                Ok(created) => *surface.borrow_mut() = Some(created),
                Err(error) => {
                    crate::logging::write(&format!("Native preview unavailable: {error}"))
                }
            }
        }
        if let Some(surface) = surface.borrow_mut().as_mut() {
            let scale = ui.window().scale_factor();
            let px = |length: f32| (length * scale).round() as i32;
            let color = ui.get_preview_well_color();
            surface.place(
                (
                    px(ui.get_preview_area_x()),
                    px(ui.get_preview_area_y()),
                    px(ui.get_preview_area_width()),
                    px(ui.get_preview_area_height()),
                ),
                px(ui.get_preview_radius()),
                [color.red(), color.green(), color.blue()],
                visible && ui.get_preview_native() && !ui.get_preview_covered(),
            );
        }
        show_latest(&ui, &preview_state, visible);
    });
    timer
}

fn show_latest(ui: &MainWindow, state: &Shared, visible: bool) {
    let channel = state.locked().preview_channel.clone();
    channel.set_enabled(visible);
    if !visible {
        return;
    }
    match channel.take() {
        Some(Ok(frame)) => {
            let mut locked = state.locked();
            if locked.session.state() == SessionState::Idle
                && let Some(source) = locked.source.as_mut()
                && (source.width != frame.source_width || source.height != frame.source_height)
            {
                source.width = frame.source_width;
                source.height = frame.source_height;
                ui.set_source_detail(format!("{} × {}", source.width, source.height).into());
                update_encoding_labels(ui, &locked);
            }
            drop(locked);
            #[cfg(feature = "diagnostics")]
            {
                static REPORTED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
                let kind = if frame.presented { 2 } else { 1 };
                if REPORTED.swap(kind, std::sync::atomic::Ordering::Relaxed) != kind {
                    let path = if frame.presented { "NATIVE" } else { "CPU" };
                    println!(
                        "PREVIEW {path} {}x{} from {}x{}",
                        frame.width, frame.height, frame.source_width, frame.source_height
                    );
                }
            }
            if !frame.presented {
                ui.set_preview_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                    slint::Rgba8Pixel,
                >::clone_from_slice(
                    &frame.rgba,
                    frame.width,
                    frame.height,
                )));
            }
            ui.set_preview_native(frame.presented);
            ui.set_preview_ready(true);
            ui.set_preview_error("".into());
        }
        Some(Err(error)) => {
            ui.set_preview_native(false);
            ui.set_preview_ready(false);
            ui.set_preview_error(error.into());
        }
        None => {}
    }
}
