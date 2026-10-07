//! Windows notification-area integration; native menus share Slint's event loop.
use crate::MainWindow;
use fastrecorder_core::SessionState;
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

pub type TrayHandle = Rc<RefCell<Option<RecordingTray>>>;
pub struct RecordingTray {
    icon: TrayIcon,
    stop: MenuItem,
    state: Cell<SessionState>,
    tooltip: RefCell<String>,
}
impl RecordingTray {
    pub fn new(ui: &MainWindow) -> Result<Self, Box<dyn std::error::Error>> {
        let pixels = ui
            .get_tray_image()
            .to_rgba8()
            .ok_or("Could not rasterize the tray icon")?;
        let icon = Icon::from_rgba(pixels.as_bytes().to_vec(), pixels.width(), pixels.height())?;
        let open = MenuItem::new("Open Pip", true, None);
        let stop = MenuItem::new("Stop recording", false, None);
        let menu = Menu::with_items(&[&open, &stop])?;
        let open_id = open.id().clone();
        let stop_id = stop.id().clone();
        let weak = ui.as_weak();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let open = event.id == open_id;
            let stop = event.id == stop_id;
            if open || stop {
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    if stop {
                        ui.invoke_tray_stop();
                    } else {
                        ui.invoke_tray_show();
                    }
                });
            }
        }));
        let weak = ui.as_weak();
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_tray_show());
            }
        }));
        let icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_tooltip("Pip")
            .with_menu_on_left_click(false)
            .build()?;
        icon.set_visible(false)?;
        Ok(Self {
            icon,
            stop,
            state: Cell::new(SessionState::Idle),
            tooltip: RefCell::new(String::new()),
        })
    }
    pub fn update(&self, state: SessionState, elapsed: &str) {
        let previous = self.state.replace(state);
        if previous != state {
            self.stop.set_enabled(matches!(
                state,
                SessionState::Starting | SessionState::Recording
            ));
            if (previous == SessionState::Idle) != (state == SessionState::Idle) {
                let _ = self.icon.set_visible(state != SessionState::Idle);
            }
        }
        let status = match state {
            SessionState::Starting => "Starting recording".to_string(),
            SessionState::Recording => format!("Recording · {elapsed}"),
            SessionState::Stopping => "Saving recording".to_string(),
            SessionState::Idle => "Ready".to_string(),
        };
        let text = format!("Pip · {status}");
        if *self.tooltip.borrow() != text {
            let _ = self.icon.set_tooltip(Some(&text));
            *self.tooltip.borrow_mut() = text;
        }
    }
}
impl Drop for RecordingTray {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    }
}
