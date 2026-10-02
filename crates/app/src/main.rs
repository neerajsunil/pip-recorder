#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

slint::include_modules!();
use slint::ComponentHandle;

#[cfg(target_os = "windows")]
mod app;
#[cfg(feature = "diagnostics")]
mod docs;
#[cfg(target_os = "windows")]
mod logging;
#[cfg(target_os = "windows")]
mod preferences;
#[cfg(target_os = "windows")]
mod tray;

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "windows")]
    logging::install();
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            #[cfg(target_os = "windows")]
            logging::write(&format!("Startup/event-loop error: {error}"));
            #[cfg(target_os = "windows")]
            fastrecorder_windows::show_details(
                0,
                &format!("FastRecorder could not continue:\n\n{error}"),
            );
            eprintln!("FastRecorder: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(target_os = "windows", feature = "diagnostics"))]
    {
        let args: Vec<_> = std::env::args().collect();
        if let Some(path) = args
            .iter()
            .position(|a| a == "--validate")
            .and_then(|i| args.get(i + 1))
        {
            let report = fastrecorder_windows::validate_recording(std::path::Path::new(path))?;
            println!(
                "DECODED {} frames, {:.3}s, {}x{}",
                report.frames, report.seconds, report.width, report.height
            );
            if report.frames == 0 || report.first_rgba.is_empty() {
                return Err("No decoded frames".into());
            }
            let image_path = std::path::Path::new(path).with_extension("decoded.png");
            image::save_buffer(
                image_path,
                &report.first_rgba,
                report.width,
                report.height,
                image::ColorType::Rgba8,
            )?;
            return Ok(());
        }
    }
    let software_ui = std::env::args().any(|arg| arg == "--software-ui");
    let ui = match create_ui(software_ui) {
        Ok(ui) => ui,
        Err(error) if !software_ui => {
            eprintln!("GPU interface unavailable; retrying software UI: {error}");
            let mut command = std::process::Command::new(std::env::current_exe()?);
            command
                .args(std::env::args_os().skip(1))
                .arg("--software-ui")
                .arg("--gpu-fallback");
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000); // CREATE_NO_WINDOW for the console, not the UI.
            }
            // Leave one application process in Task Manager. The replacement
            // handles its own startup errors instead of keeping a wrapper alive.
            command.spawn()?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    if software_ui {
        ui.set_renderer_label("Software interface".into());
        if std::env::args().any(|arg| arg == "--gpu-fallback") {
            ui.set_notice("GPU rendering wasn't available. Using the software interface.".into());
        }
    }
    #[cfg(feature = "diagnostics")]
    {
        let args: Vec<_> = std::env::args().collect();
        if let Some(path) = args
            .iter()
            .position(|arg| arg == "--docs-snapshot")
            .and_then(|index| args.get(index + 1))
        {
            return docs::snapshot(
                ui,
                std::path::Path::new(path),
                args.iter().any(|arg| arg == "--docs-audio"),
            );
        }
    }
    #[cfg(target_os = "windows")]
    app::run(ui)?;
    #[cfg(not(target_os = "windows"))]
    {
        ui.set_capture_supported(false);
        ui.set_notice("Recording is currently available on Windows 11.".into());
        ui.run()?;
    }
    Ok(())
}

fn create_ui(software_ui: bool) -> Result<MainWindow, Box<dyn std::error::Error>> {
    let selector = slint::BackendSelector::new().backend_name("winit".into());
    #[cfg(target_os = "windows")]
    let selector =
        selector.with_winit_window_attributes_hook(|attributes| attributes.with_transparent(false));
    if software_ui {
        selector.renderer_name("software".into()).select()?;
    } else {
        #[cfg(target_os = "windows")]
        let selector = selector.require_d3d();
        selector.renderer_name("femtovg-wgpu".into()).select()?;
    }
    let ui = MainWindow::new()?;
    ui.show()?;
    Ok(ui)
}
