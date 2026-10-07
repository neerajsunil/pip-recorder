//! Privacy-safe README screenshots. No capture, audio, shortcuts or preferences.
use crate::MainWindow;
use slint::ComponentHandle;
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};

pub fn snapshot(ui: MainWindow, path: &Path, view: &str) -> Result<(), Box<dyn std::error::Error>> {
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
    ui.set_save_folder("Videos › FastRecorder".into());
    ui.set_save_folder_path(r"C:\Users\you\Videos\FastRecorder".into());
    ui.set_bitrate_detail("5 Mbps · AV1 · 1920 × 1080 · 30 fps".into());
    ui.set_bitrate_mbps(5);
    ui.set_destination_label("2026-10-07-14-32-08.mp4".into());
    if crate::cli::flag("--docs-dark") {
        ui.set_theme_mode(2);
    }
    // Views: studio, video, video-advanced, audio, recording-settings, appearance,
    // community, sources, recording, countdown, saved, napping, oops.
    let settings = |page: i32, advanced: bool| {
        ui.set_settings_page(page);
        ui.set_show_advanced(advanced);
        ui.set_settings_open(true);
    };
    match view {
        "video" => settings(0, false),
        "video-advanced" => {
            let model = |values: &[&str]| {
                Rc::new(slint::VecModel::from(
                    values
                        .iter()
                        .map(|v| slint::SharedString::from(*v))
                        .collect::<Vec<_>>(),
                ))
                .into()
            };
            ui.set_gpu_names(model(&["Automatic", "NVIDIA GeForce RTX 4070"]));
            ui.set_codec_names(model(&[
                "Automatic · smallest",
                "AV1",
                "HEVC · H.265",
                "H.264 · plays everywhere",
            ]));
            ui.set_encoder_label("NVIDIA NVENC · AV1".into());
            ui.set_nvenc_controls(true);
            ui.set_keyframe_controls(true);
            settings(0, true);
        }
        "recording-settings" => settings(2, true),
        "appearance" => settings(4, false),
        "countdown" => {
            ui.set_countdown_seconds(3);
            ui.invoke_record_pressed();
        }
        "audio" => settings(1, crate::cli::flag("--docs-advanced")),
        "community" => ui.set_community_open(true),
        "sources" => {
            let entry = |name: &str, detail: &str, index| crate::SourceEntry {
                name: name.into(),
                detail: detail.into(),
                index,
            };
            ui.set_sources(
                Rc::new(slint::VecModel::from(vec![
                    entry("Display 1", "1920 × 1080 · Primary", 0),
                    entry("Display 2", "2560 × 1440", 1),
                ]))
                .into(),
            );
            ui.set_selected_source(0);
            ui.set_source_open(true);
        }
        "recording" => {
            ui.set_session_state(2);
            ui.set_elapsed("01:24".into());
            ui.set_microphone_audio(true);
            ui.set_desktop_level(0.42);
            ui.set_microphone_level(0.63);
        }
        "saved" => {
            ui.set_has_recording(true);
            ui.set_last_recording("2026-10-06-14-32-08.mp4".into());
            ui.set_notice("Saved! Nice recording.".into());
            ui.set_celebrate(true);
        }
        "napping" => ui.set_preview_enabled(false),
        "oops" => {
            ui.set_preview_ready(false);
            ui.set_preview_error("The source closed. Choose another source.".into());
            ui.set_notice("Couldn't start recording.".into());
            ui.set_notice_error(true);
        }
        _ => {}
    }
    let height = crate::cli::value("--docs-height")
        .and_then(|value| value.parse().ok())
        .unwrap_or(760.);
    ui.window().set_size(slint::LogicalSize::new(1100., height));
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
