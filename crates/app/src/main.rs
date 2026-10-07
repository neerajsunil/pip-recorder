#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

slint::include_modules!();
use slint::ComponentHandle;

mod cli;
#[cfg(feature = "diagnostics")]
mod docs;
#[cfg(target_os = "windows")]
mod logging;
#[cfg(target_os = "windows")]
mod preferences;
#[cfg(target_os = "windows")]
mod studio;
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
            fastrecorder_platform::show_details(0, &format!("Pip could not continue:\n\n{error}"));
            eprintln!("Pip: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(target_os = "windows", feature = "diagnostics"))]
    {
        if let Some(path) = cli::value("--validate") {
            let report = fastrecorder_platform::validate_recording(std::path::Path::new(&path))?;
            println!(
                "DECODED {} frames, {:.3}s, {}x{}",
                report.frames, report.seconds, report.width, report.height
            );
            if report.frames == 0 || report.first_rgba.is_empty() {
                return Err("No decoded frames".into());
            }
            let image_path = std::path::Path::new(&path).with_extension("decoded.png");
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
    let ui = create_ui()?;
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    #[cfg(feature = "diagnostics")]
    {
        if let Some(path) = cli::value("--docs-snapshot") {
            let view = cli::value("--docs-view").unwrap_or_else(|| {
                if cli::flag("--docs-audio") {
                    "audio"
                } else {
                    "studio"
                }
                .into()
            });
            return docs::snapshot(ui, std::path::Path::new(&path), &view);
        }
    }
    #[cfg(target_os = "windows")]
    studio::run(ui)?;
    #[cfg(not(target_os = "windows"))]
    {
        ui.set_capture_supported(false);
        ui.set_notice("Recording is currently available on Windows 11.".into());
        ui.run()?;
    }
    Ok(())
}

/// The studio uses Slint's software renderer: it redraws only what changed and
/// needs no GPU device of its own (the live preview is presented natively by
/// the capture GPU; see `fastrecorder_platform::PreviewSurface`).
fn create_ui() -> Result<MainWindow, Box<dyn std::error::Error>> {
    let selector = slint::BackendSelector::new().backend_name("winit".into());
    #[cfg(target_os = "windows")]
    let selector =
        selector.with_winit_window_attributes_hook(|attributes| attributes.with_transparent(false));
    selector.renderer_name("software".into()).select()?;
    let ui = MainWindow::new()?;
    ui.show()?;
    Ok(ui)
}
