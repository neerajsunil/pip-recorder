//! Development-only UI snapshots and self-test recordings (`--features diagnostics`).
use super::{
    notify::hwnd,
    state::{Lock, Shared},
};
use crate::{MainWindow, cli};
use fastrecorder_platform as native;
use slint::{ComponentHandle, Timer};
use std::{path::PathBuf, time::Duration};

pub(super) fn install(ui: &MainWindow, state: Shared) {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--compact-ui") {
        ui.window().set_size(slint::LogicalSize::new(780.0, 790.0));
    }
    if args.iter().any(|arg| arg == "--info-ui") {
        ui.set_settings_open(true);
        ui.set_info_open(true);
    }
    if args.iter().any(|arg| arg == "--settings-ui") {
        ui.set_settings_open(true);
    }
    if args.iter().any(|arg| arg == "--source-ui") {
        let weak = ui.as_weak();
        Timer::single_shot(Duration::from_millis(800), move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_source_open(true);
                ui.invoke_refresh_sources();
            }
        });
    }
    let snapshot = args
        .iter()
        .position(|arg| arg == "--snapshot")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let record = args
        .iter()
        .position(|arg| arg == "--self-test-record")
        .and_then(|i| args.get(i + 1))
        .cloned();
    if snapshot.is_none() && record.is_none() {
        return;
    }
    if record.is_some() {
        ui.set_auto_minimize(false);
        ui.set_preview_enabled(false);
        if !args.iter().any(|arg| arg == "--with-audio") {
            ui.set_desktop_audio(false);
            ui.set_microphone_audio(false);
        }
    }
    let weak = ui.as_weak();
    Timer::single_shot(Duration::from_millis(4000), move || {
        let ui = weak.upgrade().unwrap();
        if let Some(path) = snapshot {
            match ui.window().take_snapshot() {
                Ok(pixels) => {
                    let bytes: Vec<u8> = pixels
                        .as_slice()
                        .iter()
                        .flat_map(|p| [p.r, p.g, p.b, p.a])
                        .collect();
                    image::save_buffer(
                        &path,
                        &bytes,
                        pixels.width(),
                        pixels.height(),
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                    println!("SNAPSHOT {path}");
                }
                Err(e) => eprintln!("SNAPSHOT FAILED: {e}"),
            }
        }
        if let Some(path) = record {
            let source = match hwnd(&ui)
                .and_then(|hwnd| native::own_window_source(hwnd).map_err(|e| e.to_string()))
            {
                Ok(source) => source,
                Err(e) => {
                    eprintln!("SOURCE FAILED: {e}");
                    let _ = slint::quit_event_loop();
                    return;
                }
            };
            ui.set_source_name(source.name.clone().into());
            ui.set_source_detail(
                format!(
                    "{} × {}  ·  Original resolution",
                    source.width, source.height
                )
                .into(),
            );
            ui.set_source_selected(true);
            ui.set_destination_selected(true);
            ui.set_destination_label(
                PathBuf::from(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
                    .into(),
            );
            if cli::flag("--60fps") {
                ui.set_fps(60);
            }
            let mut app_state = state.locked();
            app_state.source = Some(source);
            app_state.selected_candidate = None;
            app_state.destination = Some(path.into());
            app_state.custom_destination = true;
            app_state.diagnostic = true;
            drop(app_state);
            ui.invoke_toggle_recording();
            if cli::flag("--resize-source") {
                let weak = ui.as_weak();
                Timer::single_shot(Duration::from_millis(1500), move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.window().set_size(slint::LogicalSize::new(780.0, 790.0));
                    }
                });
            }
            let weak = ui.as_weak();
            Timer::single_shot(Duration::from_secs(4), move || {
                if let Some(ui) = weak.upgrade() {
                    ui.invoke_toggle_recording();
                }
            });
            Timer::single_shot(Duration::from_secs(20), move || {
                eprintln!("DIAGNOSTIC TIMEOUT");
                state.locked().diagnostic_failure = Some("Diagnostic recording timed out.".into());
                let _ = slint::quit_event_loop();
            });
        } else {
            let _ = slint::quit_event_loop();
        }
    });
}
