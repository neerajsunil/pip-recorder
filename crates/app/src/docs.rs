//! Privacy-safe README screenshots. No capture, audio, shortcuts or preferences.
use crate::MainWindow;
use slint::ComponentHandle;
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};

pub fn snapshot(
    ui: MainWindow,
    path: &Path,
    audio_page: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    ui.set_hardware_ready(true);
    ui.set_audio_ready(true);
    ui.set_source_selected(true);
    ui.set_destination_selected(true);
    ui.set_source_name("Display 1".into());
    ui.set_source_detail("1920 × 1080".into());
    ui.set_codec_label("AV1".into());
    ui.set_encoder_label("Automatic · best available".into());
    let preview =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/images/sample-desktop.png");
    ui.set_preview_image(slint::Image::load_from_path(&preview)?);
    ui.set_preview_ready(true);
    ui.set_audio_status("".into());
    let model = |value: &str| {
        Rc::new(slint::VecModel::from(vec![slint::SharedString::from(
            value,
        )]))
        .into()
    };
    ui.set_desktop_audio_devices(model("System default playback device"));
    ui.set_microphone_audio_devices(model("System default microphone"));
    ui.set_shortcut_status("Start: Ctrl+Shift+F9\nStop: Ctrl+Shift+F10".into());
    if audio_page {
        ui.set_settings_page(1);
        ui.set_settings_open(true);
    }
    ui.window().set_size(slint::LogicalSize::new(1100., 760.));
    let path = path.to_path_buf();
    let result: Rc<RefCell<Option<Result<(), String>>>> = Rc::new(RefCell::new(None));
    let outcome = result.clone();
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1200), move || {
        let saved = (|| -> Result<(), String> {
            let ui = weak.upgrade().ok_or("Documentation window closed")?;
            let pixels = ui.window().take_snapshot().map_err(|e| e.to_string())?;
            let bytes: Vec<_> = pixels
                .as_slice()
                .iter()
                .flat_map(|p| [p.r, p.g, p.b, p.a])
                .collect();
            image::save_buffer(
                path,
                &bytes,
                pixels.width(),
                pixels.height(),
                image::ColorType::Rgba8,
            )
            .map_err(|e| e.to_string())
        })();
        *outcome.borrow_mut() = Some(saved);
        let _ = slint::quit_event_loop();
    });
    slint::run_event_loop()?;
    result
        .borrow_mut()
        .take()
        .ok_or("Documentation snapshot did not finish")?
        .map_err(Into::into)
}
