//! Swapchain recovery after minimize/restore, occlusion and DPI changes.
use crate::MainWindow;
use slint::{ComponentHandle, Timer};
use std::time::Duration;

pub(super) fn restore_studio(ui: &MainWindow) {
    use slint::winit_030::WinitWindowAccessor;
    ui.window().set_minimized(false);
    let _ = ui.show();
    ui.window()
        .with_winit_window(|window| window.focus_window());
    refresh_studio_surface(ui);
    let weak = ui.as_weak();
    Timer::single_shot(Duration::from_millis(100), move || {
        if let Some(ui) = weak.upgrade() {
            refresh_studio_surface(&ui);
        }
    });
}

pub(super) fn refresh_studio_surface(ui: &MainWindow) {
    use slint::winit_030::WinitWindowAccessor;
    let size = ui
        .window()
        .with_winit_window(|window| {
            if window.is_minimized() == Some(true) {
                return None;
            }
            let size = window.inner_size();
            (size.width > 0 && size.height > 0).then(|| {
                slint::LogicalSize::new(
                    size.width as f32 / window.scale_factor() as f32,
                    size.height as f32 / window.scale_factor() as f32,
                )
            })
        })
        .flatten();
    if let Some(size) = size {
        // Restoring through the taskbar bypasses tray callbacks. Reconfigure
        // the swapchain from the current native size, even if it is unchanged.
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::Resized { size });
        ui.window().request_redraw();
    }
}

pub(super) fn install_surface_recovery(ui: &MainWindow) {
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
    let weak = ui.as_weak();
    let pending = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut minimized = false;
    ui.window().on_winit_window_event(move |window, event| {
        match event {
            WindowEvent::Resized(size) if size.width == 0 || size.height == 0 => minimized = true,
            WindowEvent::RedrawRequested
                if window
                    .with_winit_window(|window| window.is_minimized())
                    .flatten()
                    == Some(true) =>
            {
                // A minimized swapchain cannot present. Let the next restore
                // event request a frame instead of repeatedly acquiring it.
                return EventResult::PreventDefault;
            }
            WindowEvent::Resized(_) if minimized => {
                minimized = false;
                schedule_surface_refresh(&weak, &pending);
            }
            WindowEvent::Focused(true) => schedule_surface_refresh(&weak, &pending),
            WindowEvent::Occluded(false) | WindowEvent::ScaleFactorChanged { .. } => {
                schedule_surface_refresh(&weak, &pending)
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

pub(super) fn schedule_surface_refresh(
    weak: &slint::Weak<MainWindow>,
    pending: &std::rc::Rc<std::cell::Cell<bool>>,
) {
    if pending.replace(true) {
        return;
    }
    let weak = weak.clone();
    let pending = pending.clone();
    Timer::single_shot(Duration::from_millis(50), move || {
        pending.set(false);
        if let Some(ui) = weak.upgrade() {
            refresh_studio_surface(&ui);
        }
    });
}
