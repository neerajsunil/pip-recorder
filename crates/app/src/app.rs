use crate::{
    MainWindow, SourceEntry,
    tray::{RecordingTray, TrayHandle},
};
use fastrecorder_core::{EncoderPreference, RecordingConfig, Session, SessionState};
use fastrecorder_windows::{self as native, Recording, RecordingEvent, Source};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{CloseRequestResponse, ComponentHandle, Timer, TimerMode};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct AppState {
    session: Session,
    source: Option<Source>,
    sources: Vec<native::SourceCandidate>,
    selected_candidate: Option<native::SourceCandidate>,
    destination: Option<PathBuf>,
    custom_destination: bool,
    recording: Option<Recording>,
    preview: Option<native::Preview>,
    preview_channel: native::PreviewChannel,
    gpus: Vec<native::GpuInfo>,
    started: Option<Instant>,
    last_file: Option<PathBuf>,
    close_after_save: bool,
    preferred_encoder: Option<i32>,
    preferred_gpu: Option<String>,
    desktop_devices: Vec<native::AudioDevice>,
    microphone_devices: Vec<native::AudioDevice>,
    desktop_device: Option<String>,
    microphone_device: Option<String>,
    audio_refresh: u64,
    #[cfg(feature = "diagnostics")]
    diagnostic: bool,
    #[cfg(feature = "diagnostics")]
    diagnostic_failure: Option<String>,
}

fn persist_preferences(ui: &MainWindow, state: &AppState) {
    if !ui.get_hardware_ready()
        || std::env::args().any(|arg| arg == "--snapshot" || arg == "--self-test-record")
    {
        return;
    }
    let gpu = (ui.get_gpu_choice() > 0)
        .then(|| {
            state
                .gpus
                .get((ui.get_gpu_choice() - 1) as usize)
                .map(|gpu| gpu.name.clone())
        })
        .flatten();
    let prefs = crate::preferences::Preferences::capture(
        ui,
        state.destination.as_ref().and_then(|p| p.parent()),
        state.last_file.as_deref(),
        gpu,
        state.desktop_device.clone(),
        state.microphone_device.clone(),
    );
    if let Err(error) = prefs.save() {
        notify(ui, format!("Preferences could not be saved: {error}"), true);
    }
}

fn refresh_audio_devices(ui: &MainWindow, state: &Arc<Mutex<AppState>>) {
    if ui.get_session_state() != 0 {
        return;
    }
    let generation = {
        let mut state = state.lock().unwrap();
        state.audio_refresh += 1;
        state.audio_refresh
    };
    ui.set_audio_ready(false);
    ui.set_audio_status("Refreshing audio devices…".into());
    let weak = ui.as_weak();
    let state = state.clone();
    let result = std::thread::Builder::new()
        .name("fastrecorder-audio-devices".into())
        .spawn(move || {
            let result = native::audio_inventory();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if state.lock().unwrap().audio_refresh != generation {
                    return;
                }
                match result {
                    Ok(inventory) => {
                        let mut state = state.lock().unwrap();
                        let names = |default: &str, devices: &[native::AudioDevice]| {
                            std::rc::Rc::new(slint::VecModel::from(
                                std::iter::once(slint::SharedString::from(default))
                                    .chain(devices.iter().map(|device| device.name.clone().into()))
                                    .collect::<Vec<_>>(),
                            ))
                            .into()
                        };
                        ui.set_desktop_audio_devices(names(
                            "System default playback device",
                            &inventory.desktop,
                        ));
                        ui.set_microphone_audio_devices(names(
                            "System default microphone",
                            &inventory.microphones,
                        ));
                        let index = |id: &Option<String>, devices: &[native::AudioDevice]| {
                            id.as_ref().map_or(0, |id| {
                                devices
                                    .iter()
                                    .position(|device| &device.id == id)
                                    .map_or(-1, |index| index as i32 + 1)
                            })
                        };
                        ui.set_desktop_audio_device(index(
                            &state.desktop_device,
                            &inventory.desktop,
                        ));
                        ui.set_microphone_audio_device(index(
                            &state.microphone_device,
                            &inventory.microphones,
                        ));
                        state.desktop_devices = inventory.desktop;
                        state.microphone_devices = inventory.microphones;
                        let mut messages = Vec::new();
                        if ui.get_desktop_audio_device() < 0 {
                            messages
                                .push("Saved playback device disconnected. Select another device.");
                        }
                        if ui.get_microphone_audio_device() < 0 {
                            messages.push("Saved microphone disconnected. Select another device.");
                        }
                        if state.desktop_devices.is_empty() {
                            messages.push("No connected playback devices.");
                        }
                        if state.microphone_devices.is_empty() {
                            messages.push("No connected microphones.");
                        }
                        ui.set_audio_status(messages.join("\n").into());
                        ui.set_audio_ready(true);
                    }
                    Err(error) => {
                        ui.set_audio_status(error.clone().into());
                        notify(&ui, error, true);
                    }
                }
            });
        });
    if let Err(error) = result {
        ui.set_audio_status(format!("Could not refresh audio devices: {error}").into());
    }
}

fn register_shortcuts(
    ui: &MainWindow,
    start: &str,
    stop: &str,
) -> Result<native::RecordingShortcut, String> {
    let weak = ui.as_weak();
    native::RecordingShortcut::register(start, stop, move |action| {
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let can_stop = matches!(ui.get_session_state(), 1 | 2);
            let apply = match action {
                native::ShortcutAction::Start => ui.get_can_start_recording(),
                native::ShortcutAction::Stop => can_stop,
                native::ShortcutAction::Toggle => can_stop || ui.get_can_start_recording(),
            };
            if apply {
                ui.invoke_toggle_recording();
            }
        });
    })
}
fn shortcut_status(start: &str, stop: &str) -> String {
    let name = |text: &str| {
        if text.trim().is_empty() {
            "Disabled".to_string()
        } else {
            text.to_string()
        }
    };
    format!("Start: {}\nStop: {}", name(start), name(stop))
}

fn hwnd(ui: &MainWindow) -> Result<usize, String> {
    let handle = ui.window().window_handle();
    match handle.window_handle().map_err(|e| e.to_string())?.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as usize),
        _ => Err("The native window is unavailable.".into()),
    }
}

fn notify(ui: &MainWindow, message: impl Into<slint::SharedString>, error: bool) {
    ui.set_notice(message.into());
    ui.set_notice_error(error);
}

fn restore_studio(ui: &MainWindow) {
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

fn refresh_studio_surface(ui: &MainWindow) {
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

fn install_surface_recovery(ui: &MainWindow) {
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
            _ => {}
        }
        EventResult::Propagate
    });
}

fn schedule_surface_refresh(
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
fn preference(choice: i32) -> EncoderPreference {
    match choice {
        1 => EncoderPreference::NvencAv1,
        2 => EncoderPreference::NvencHevc,
        3 => EncoderPreference::NvencH264,
        4 => EncoderPreference::IntelAv1,
        5 => EncoderPreference::IntelHevc,
        6 => EncoderPreference::IntelH264,
        7 => EncoderPreference::SoftwareOnly,
        8 => EncoderPreference::SoftwareAv1,
        _ => EncoderPreference::Auto,
    }
}
fn selected_gpu(ui: &MainWindow, state: &AppState) -> Option<u32> {
    if ui.get_gpu_choice() > 0 {
        return state
            .gpus
            .get((ui.get_gpu_choice() - 1) as usize)
            .map(|gpu| gpu.index);
    }
    let choice = ui.get_encoder_choice();
    if (1..=6).contains(&choice) {
        let vendor = if choice <= 3 { 0x10de } else { 0x8086 };
        let codec = ((choice - 1) % 3) as usize;
        return state
            .gpus
            .iter()
            .find(|gpu| gpu.vendor == vendor && gpu.codecs[codec])
            .or_else(|| state.gpus.iter().find(|gpu| gpu.vendor == vendor))
            .map(|gpu| gpu.index);
    }
    state
        .gpus
        .iter()
        .find(|gpu| gpu.vendor == 0x10de && gpu.av1)
        .or_else(|| state.gpus.iter().find(|gpu| gpu.av1))
        .or_else(|| state.gpus.iter().find(|gpu| gpu.codecs[1]))
        .or_else(|| state.gpus.iter().find(|gpu| gpu.codecs[2]))
        .map(|gpu| gpu.index)
}
const ENCODER_NAMES: [&str; 9] = [
    "Automatic · best available",
    "NVIDIA NVENC · AV1",
    "NVIDIA NVENC · HEVC",
    "NVIDIA NVENC · H.264",
    "Intel Quick Sync · AV1",
    "Intel Quick Sync · HEVC",
    "Intel Quick Sync · H.264",
    "Windows software · H.264",
    "rav1e software · AV1",
];
fn encoder_choices(ui: &MainWindow, state: &AppState) -> Vec<usize> {
    let explicit_gpu = if ui.get_gpu_choice() > 0 {
        state.gpus.get((ui.get_gpu_choice() - 1) as usize)
    } else {
        None
    };
    (0..ENCODER_NAMES.len())
        .filter(|&i| {
            if !(1..=6).contains(&i) {
                return true;
            }
            let supported = |gpu: &native::GpuInfo| {
                gpu.vendor == if i <= 3 { 0x10de } else { 0x8086 } && gpu.codecs[(i - 1) % 3]
            };
            explicit_gpu.map_or_else(|| state.gpus.iter().any(supported), supported)
        })
        .collect()
}
fn update_encoding_labels(ui: &MainWindow, state: &AppState) {
    let choices = encoder_choices(ui, state);
    if !choices.contains(&(ui.get_encoder_choice() as usize)) {
        ui.set_encoder_choice(0);
    }
    let gpu =
        selected_gpu(ui, state).and_then(|index| state.gpus.iter().find(|gpu| gpu.index == index));
    let choice = ui.get_encoder_choice().clamp(0, 8) as usize;
    let (encoder, codec) = if choice == 0 {
        if let Some(gpu) = gpu {
            let c = gpu.codecs.iter().position(|supported| *supported);
            match (gpu.vendor, c) {
                (0x10de, Some(i)) => (
                    ENCODER_NAMES[1 + i],
                    [
                        fastrecorder_core::Codec::Av1,
                        fastrecorder_core::Codec::Hevc,
                        fastrecorder_core::Codec::H264,
                    ][i],
                ),
                (0x8086, Some(i)) => (
                    ENCODER_NAMES[4 + i],
                    [
                        fastrecorder_core::Codec::Av1,
                        fastrecorder_core::Codec::Hevc,
                        fastrecorder_core::Codec::H264,
                    ][i],
                ),
                _ => ("Media Foundation · H.264", fastrecorder_core::Codec::H264),
            }
        } else {
            ("Media Foundation · H.264", fastrecorder_core::Codec::H264)
        }
    } else {
        (ENCODER_NAMES[choice], preference(choice as i32).codec())
    };
    let available = if (1..=6).contains(&choice) {
        gpu.is_some_and(|gpu| {
            gpu.vendor == if choice <= 3 { 0x10de } else { 0x8086 } && gpu.codecs[(choice - 1) % 3]
        })
    } else {
        true
    };
    ui.set_encoder_available(available);
    ui.set_encoder_status(
        if !available {
            "Unavailable on the selected GPU / installed driver. Choose another encoder or GPU."
        } else if choice == 8 {
            "CPU encoding · rav1e speed 8 · may skip frames if the CPU cannot keep up."
        } else if (4..=6).contains(&choice) {
            "Hardware encoding · installed Intel oneVPL / Media SDK runtime · NV12 readback."
        } else {
            ""
        }
        .into(),
    );
    ui.set_encoder_names(
        std::rc::Rc::new(slint::VecModel::from(
            choices
                .iter()
                .map(|&i| slint::SharedString::from(ENCODER_NAMES[i]))
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    ui.set_encoder_list_index(choices.iter().position(|&i| i == choice).unwrap_or(0) as i32);
    ui.set_gpu_label(if choice >= 7 {
        "CPU".into()
    } else {
        gpu.map(|g| g.name.clone())
            .unwrap_or_else(|| "System default adapter".into())
            .into()
    });
    ui.set_nvenc_controls(encoder.starts_with("NVIDIA"));
    ui.set_keyframe_controls(
        encoder.starts_with("NVIDIA")
            || encoder.starts_with("Intel")
            || codec == fastrecorder_core::Codec::Av1,
    );
    ui.set_encoder_tech(if encoder.starts_with("NVIDIA") { format!("NVENC API 12.1 ABI · P{} · high-quality tuning · spatial AQ · {} · P-only / no lookahead · {}s keyframes", ui.get_nvenc_preset(), if ui.get_quality_mode() { format!("CQP {} · single pass", ui.get_quality_level()) } else { format!("{} · two-pass quarter resolution", if ui.get_constant_bitrate() { "CBR" } else { "VBR" }) }, ui.get_keyframe_seconds()) }
        else if encoder.starts_with("Intel") { format!("Intel oneVPL / Media SDK · hardware · TU1 best-quality request · {} · no B-frames · {}s keyframes · NV12 readback", if ui.get_constant_bitrate() { "CBR" } else { "VBR" }, ui.get_keyframe_seconds()) }
        else if codec == fastrecorder_core::Codec::Av1 { "rav1e 0.8.1 · Rust · speed 8 · P-only · 8-frame lookahead · CPU NV12 input".into() }
        else { "Windows Media Foundation · H.264 · driver / Windows rate control".into() }.into());
    ui.set_encoder_label(encoder.into());
    ui.set_codec_label(codec.name().into());
    let (width, height) = state
        .source
        .as_ref()
        .map(|s| (s.width, s.height))
        .unwrap_or((1920, 1080));
    let recommended =
        fastrecorder_core::recommended_bitrate_mbps(codec, width, height, ui.get_fps() as u32);
    let bitrate = match ui.get_bitrate_mode() {
        0 => (recommended as f64 * 1.5).round() as u32,
        2 => (recommended as f64 * 0.65).round() as u32,
        3 => ui.get_custom_bitrate() as u32,
        _ => recommended,
    }
    .clamp(1, 100);
    ui.set_bitrate_mbps(bitrate as i32);
    ui.set_bitrate_detail(
        if encoder.starts_with("NVIDIA") && ui.get_quality_mode() {
            format!(
                "CQP {} · {} · {width} × {height} · {} fps · variable bitrate / file size",
                ui.get_quality_level(),
                codec.name(),
                ui.get_fps()
            )
        } else {
            format!(
                "{bitrate} Mbps · {} · {width} × {height} · {} fps",
                codec.name(),
                ui.get_fps()
            )
        }
        .into(),
    );
    ui.set_fallback_detail(if choice == 0 { "Prefer AV1 → HEVC → H.264 on the selected GPU; then Media Foundation hardware H.264 → Windows software H.264. Startup failures are reported." }
        else { "Explicit selection; codec / vendor failures are reported without switching encoders." }.into());
}
fn assign_destination(ui: &MainWindow, state: &mut AppState) {
    let directory = state
        .destination
        .as_ref()
        .or(state.last_file.as_ref())
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(native::default_recording_directory);
    match directory {
        Ok(directory) => {
            let path = native::timestamped_destination(&directory);
            state.custom_destination = false;
            ui.set_destination_label(path.to_string_lossy().into_owned().into());
            ui.set_destination_selected(true);
            state.destination = Some(path);
        }
        Err(error) => notify(
            ui,
            format!("Could not prepare a save location: {error}"),
            true,
        ),
    }
}

fn refresh_sources(ui: &MainWindow, state: &Arc<Mutex<AppState>>) {
    let mut state = state.lock().unwrap();
    if state.session.state() != SessionState::Idle || !ui.get_capture_supported() {
        return;
    }
    match hwnd(ui).and_then(native::enumerate_sources) {
        Ok(sources) => {
            let selected = state.selected_candidate.as_ref().and_then(|selected| {
                sources
                    .iter()
                    .position(|candidate| candidate.same_target(selected))
            });
            ui.set_selected_source(selected.map_or(-1, |index| index as i32));
            if state.selected_candidate.is_some() && selected.is_none() {
                state.preview.take(); // Drop requests stop without blocking the UI.
                state.preview_channel = native::PreviewChannel::default();
                ui.set_preview_ready(false);
                ui.set_preview_image(slint::Image::default());
                ui.set_preview_error("Choose an available source.".into());
                state.source = None;
                state.selected_candidate = None;
                ui.set_source_selected(false);
                ui.set_source_name("".into());
                notify(ui, "The selected source is no longer available.", false);
            }
            let rows: Vec<SourceEntry> = sources
                .iter()
                .enumerate()
                .filter(|(_, source)| source.display == ui.get_display_tab())
                .map(|(index, source)| SourceEntry {
                    name: source.name.clone().into(),
                    detail: source.detail.clone().into(),
                    index: index as i32,
                })
                .collect();
            ui.set_sources(std::rc::Rc::new(slint::VecModel::from(rows)).into());
            state.sources = sources;
        }
        Err(error) => notify(ui, format!("Could not list sources: {error}"), true),
    }
}

pub fn run(ui: MainWindow) -> Result<(), Box<dyn std::error::Error>> {
    let _apartment = native::initialize_ui()?;
    install_surface_recovery(&ui);
    let state = Arc::new(Mutex::new(AppState::default()));
    match crate::preferences::Preferences::load() {
        Ok(preferences) => {
            preferences.apply(&ui);
            let mut state = state.lock().unwrap();
            state.preferred_encoder = Some(preferences.encoder.clamp(0, 8));
            state.preferred_gpu = preferences.gpu;
            state.desktop_device = preferences.desktop_device;
            state.microphone_device = preferences.microphone_device;
            if let Some(directory) = preferences.save_directory {
                if directory.is_dir() {
                    state.destination = Some(native::timestamped_destination(&directory));
                } else {
                    notify(
                        &ui,
                        "Your saved recording folder is unavailable. Using Videos/FastRecorder.",
                        false,
                    );
                }
            }
            if let Some(path) = preferences.last_recording.filter(|path| path.is_file()) {
                ui.set_has_recording(true);
                ui.set_last_recording(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                        .into(),
                );
                state.last_file = Some(path);
            }
        }
        Err(error) => notify(&ui, error, true),
    }
    let tray: TrayHandle = std::rc::Rc::new(std::cell::RefCell::new(None));
    let (event_sender, event_receiver) = std::sync::mpsc::sync_channel(32);
    let event_tray = tray.clone();
    let event_state = state.clone();
    let weak = ui.as_weak();
    ui.on_drain_recording_events(move || {
        if let Some(ui) = weak.upgrade() {
            while let Ok(event) = event_receiver.try_recv() {
                handle_event(&ui, &event_tray, &event_state, event);
            }
        }
    });
    ui.set_capture_supported(native::capture_supported());
    if !ui.get_capture_supported() {
        notify(
            &ui,
            "Windows screen capture isn't available on this device.",
            true,
        );
    }
    let timer = std::rc::Rc::new(Timer::default());
    let audio_state = state.clone();
    let weak = ui.as_weak();
    ui.on_refresh_audio_devices(move || {
        if let Some(ui) = weak.upgrade() {
            refresh_audio_devices(&ui, &audio_state);
        }
    });
    refresh_audio_devices(&ui, &state);

    let weak = ui.as_weak();
    ui.on_show_details(move || {
        if let Some(ui) = weak.upgrade()
            && let Ok(owner) = hwnd(&ui)
        {
            native::show_details(owner, ui.get_notice().as_str());
        }
    });

    let weak = ui.as_weak();
    let source_state = state.clone();
    ui.on_refresh_sources(move || {
        if let Some(ui) = weak.upgrade() {
            refresh_sources(&ui, &source_state);
        }
    });

    let weak = ui.as_weak();
    let source_state = state.clone();
    ui.on_select_source(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = source_state.lock().unwrap();
        if state.session.state() != SessionState::Idle {
            return;
        }
        let Some(candidate) = usize::try_from(index)
            .ok()
            .and_then(|i| state.sources.get(i))
            .cloned()
        else {
            return;
        };
        match candidate.open() {
            Ok(source) => {
                ui.set_source_name(source.name.clone().into());
                ui.set_source_detail(format!("{} × {}", source.width, source.height).into());
                ui.set_source_selected(true);
                ui.set_source_is_display(candidate.display);
                ui.set_selected_source(index);
                state.preview.take();
                state.preview_channel = native::PreviewChannel::default();
                state.preview_channel.set_cursor(ui.get_capture_cursor());
                ui.set_preview_ready(false);
                ui.set_preview_error("".into());
                state.preview =
                    match native::Preview::start(source.clone(), state.preview_channel.clone()) {
                        Ok(preview) => Some(preview),
                        Err(error) => {
                            ui.set_preview_error(error.into());
                            None
                        }
                    };
                state.source = Some(source);
                update_encoding_labels(&ui, &state);
                ui.set_source_open(false);
                state.selected_candidate = Some(candidate);
                notify(&ui, "", false);
            }
            Err(error) => {
                state.preview.take();
                state.preview_channel = native::PreviewChannel::default();
                ui.set_preview_ready(false);
                ui.set_preview_error("Choose an available source.".into());
                state.source = None;
                state.selected_candidate = None;
                ui.set_source_selected(false);
                ui.set_selected_source(-1);
                notify(&ui, format!("Could not select source: {error}"), true);
            }
        }
    });
    let weak = ui.as_weak();
    let destination_state = state.clone();
    ui.on_choose_destination(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if destination_state.lock().unwrap().session.state() != SessionState::Idle {
            return;
        }
        let owner = match hwnd(&ui) {
            Ok(owner) => owner,
            Err(e) => {
                notify(&ui, e, true);
                return;
            }
        };
        let suggested = native::timestamped_destination(&PathBuf::new());
        let suggested = suggested.file_name().unwrap().to_string_lossy();
        match native::choose_destination(owner, &suggested) {
            Ok(Some(path)) => {
                if path.exists() {
                    notify(
                        &ui,
                        "Choose a new filename to keep your existing recording.",
                        true,
                    );
                    return;
                }
                ui.set_destination_label(path.to_string_lossy().into_owned().into());
                ui.set_destination_selected(true);
                let mut state = destination_state.lock().unwrap();
                state.destination = Some(path);
                state.custom_destination = true;
                persist_preferences(&ui, &state);
                notify(&ui, "", false);
            }
            Ok(None) => {}
            Err(e) => notify(&ui, format!("Could not choose a save location: {e}"), true),
        }
    });

    let weak = ui.as_weak();
    let recording_state = state.clone();
    let recording_timer = timer.clone();
    let recording_tray = tray.clone();
    ui.on_toggle_recording(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = recording_state.lock().unwrap();
        if matches!(
            state.session.state(),
            SessionState::Starting | SessionState::Recording
        ) {
            if state.session.stop() {
                if let Some(recording) = &state.recording {
                    recording.stop();
                }
                ui.set_session_state(3);
                if let Some(tray) = recording_tray.borrow().as_ref() {
                    tray.update(SessionState::Stopping, ui.get_elapsed().as_str());
                }
                notify(&ui, "Saving…", false);
            }
            return;
        }
        if state.session.state() != SessionState::Idle {
            return;
        }
        if (ui.get_desktop_audio() || ui.get_microphone_audio())
            && (!ui.get_audio_ready()
                || ui.get_desktop_audio() && ui.get_desktop_audio_device() < 0
                || ui.get_microphone_audio() && ui.get_microphone_audio_device() < 0)
        {
            notify(
                &ui,
                "Choose connected audio devices in Settings → Audio, or disable audio.",
                true,
            );
            return;
        }
        if !state.custom_destination {
            assign_destination(&ui, &mut state);
        }
        let (Some(source), Some(destination)) = (state.source.clone(), state.destination.clone())
        else {
            return;
        };
        let source = if let Some(candidate) = &state.selected_candidate {
            match candidate.open() {
                Ok(source) => source,
                Err(error) => {
                    notify(&ui, error, true);
                    return;
                }
            }
        } else {
            source
        };
        update_encoding_labels(&ui, &state);
        if !ui.get_encoder_available() {
            notify(
                &ui,
                "The selected encoder is unavailable on this GPU / driver.",
                true,
            );
            return;
        }
        let config = RecordingConfig {
            destination,
            fps: ui.get_fps() as u32,
            bitrate_mbps: ui.get_bitrate_mbps() as u32,
            bitrate_mode: ui.get_bitrate_mode() as u32,
            gpu_index: selected_gpu(&ui, &state),
            nvenc_preset: ui.get_nvenc_preset() as u32,
            keyframe_seconds: ui.get_keyframe_seconds() as u32,
            constant_bitrate: ui.get_constant_bitrate(),
            capture_cursor: ui.get_capture_cursor(),
            quality_qp: (ui.get_quality_mode() && ui.get_nvenc_controls())
                .then_some(ui.get_quality_level() as u32),
            audio: {
                let diagnostic = cfg!(feature = "diagnostics")
                    && std::env::args().any(|arg| arg == "--self-test-record")
                    && !std::env::args().any(|arg| arg == "--with-audio");
                fastrecorder_core::AudioConfig {
                    desktop: ui.get_desktop_audio() && !diagnostic,
                    microphone: ui.get_microphone_audio() && !diagnostic,
                    desktop_device: state.desktop_device.clone(),
                    microphone_device: state.microphone_device.clone(),
                    desktop_volume: ui.get_desktop_volume() as u32,
                    microphone_volume: ui.get_microphone_volume() as u32,
                }
            },
            encoder: if ui.get_software_encoder()
                || cfg!(feature = "diagnostics")
                    && std::env::args().any(|arg| arg == "--software-encoder")
            {
                EncoderPreference::SoftwareOnly
            } else {
                preference(ui.get_encoder_choice())
            },
        };
        if let Err(e) = config.validate() {
            notify(&ui, e.clone(), true);
            #[cfg(feature = "diagnostics")]
            if state.diagnostic {
                state.diagnostic_failure = Some(e);
                let _ = slint::quit_event_loop();
            }
            return;
        }
        if let Some(mut old) = state.recording.take() {
            old.join();
        }
        state.preview.take();
        // Separate channels prevent a retiring preview from publishing stale pixels.
        state.preview_channel = native::PreviewChannel::default();
        state.preview_channel.set_cursor(ui.get_capture_cursor());
        state
            .preview_channel
            .set_enabled(!ui.window().is_minimized() && ui.get_preview_enabled());
        ui.set_preview_ready(false);
        ui.set_source_open(false);
        ui.set_settings_open(false);
        ui.set_info_open(false);
        ui.set_source_detail(format!("{} × {}", source.width, source.height).into());
        state.source = Some(source.clone());
        ui.set_active_encoder("Starting…".into());
        ui.set_audio_details("Starting…".into());
        ui.set_recording_stats("0 frames skipped".into());
        state.session.begin();
        if let Some(tray) = recording_tray.borrow().as_ref() {
            tray.update(SessionState::Starting, "00:00");
        }
        ui.set_session_state(1);
        ui.set_elapsed("00:00".into());
        ui.set_window_title("FastRecorder · Starting recording".into());
        notify(&ui, "", false);
        let event_weak = ui.as_weak();
        let preview_channel = state.preview_channel.clone();
        let event_sender = event_sender.clone();
        match Recording::start(source, config, preview_channel, move |event| {
            match event_sender.try_send(event) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(RecordingEvent::Statistics { .. })) => {
                    return;
                }
                Err(std::sync::mpsc::TrySendError::Full(event)) => {
                    if event_sender.send(event).is_err() {
                        return;
                    }
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
            }
            let _ = event_weak.upgrade_in_event_loop(|ui| ui.invoke_drain_recording_events());
        }) {
            Ok(recording) => state.recording = Some(recording),
            Err(e) => {
                state.session.finished();
                if let Some(tray) = recording_tray.borrow().as_ref() {
                    tray.update(SessionState::Idle, "");
                }
                ui.set_session_state(0);
                ui.set_window_title("FastRecorder".into());
                if let Some(source) = state.source.clone() {
                    state.preview =
                        native::Preview::start(source, state.preview_channel.clone()).ok();
                }
                notify(&ui, e, true);
            }
        }
        let clock_tray = recording_tray.clone();
        let timer_weak = ui.as_weak();
        let timer_state = recording_state.clone();
        let weak_timer = std::rc::Rc::downgrade(&recording_timer);
        recording_timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            let Some(ui) = timer_weak.upgrade() else {
                return;
            };
            let state = timer_state.lock().unwrap();
            if state.session.state() == SessionState::Idle {
                if let Some(timer) = weak_timer.upgrade() {
                    timer.stop();
                }
            } else if let Some(started) = state.started {
                let seconds = started.elapsed().as_secs();
                let elapsed: slint::SharedString =
                    format!("{:02}:{:02}", seconds / 60, seconds % 60).into();
                ui.set_elapsed(elapsed.clone());
                ui.set_window_title(format!("FastRecorder · Recording {elapsed}").into());
                if let Some(tray) = clock_tray.borrow().as_ref() {
                    tray.update(state.session.state(), elapsed.as_str());
                }
            }
        });
    });

    let reveal_state = state.clone();
    let weak = ui.as_weak();
    ui.on_reveal_recording(move || {
        let file = reveal_state.lock().unwrap().last_file.clone();
        if let Some(file) = file
            && let Err(e) = native::reveal_recording(&file)
            && let Some(ui) = weak.upgrade()
        {
            notify(
                &ui,
                format!("Could not open the recording folder: {e}"),
                true,
            );
        }
    });
    let open_state = state.clone();
    let weak = ui.as_weak();
    ui.on_open_recording(move || {
        let file = open_state.lock().unwrap().last_file.clone();
        if let Some(file) = file
            && let Err(error) = native::open_recording(&file)
            && let Some(ui) = weak.upgrade()
        {
            notify(&ui, error, true);
        }
    });

    let shortcuts = std::rc::Rc::new(std::cell::RefCell::new(None));
    let diagnostic_shortcuts =
        std::env::args().any(|arg| arg == "--snapshot" || arg == "--self-test-record");
    let start = ui.get_active_start_shortcut().to_string();
    let stop = ui.get_active_stop_shortcut().to_string();
    if !diagnostic_shortcuts {
        match register_shortcuts(&ui, &start, &stop) {
            Ok(shortcut) => {
                *shortcuts.borrow_mut() = Some(shortcut);
                ui.set_shortcut_status(shortcut_status(&start, &stop).into());
            }
            Err(error) => ui.set_shortcut_status(format!("Shortcuts inactive: {error}").into()),
        }
    }
    let shortcut_holder = shortcuts.clone();
    let shortcut_state = state.clone();
    let weak = ui.as_weak();
    ui.on_apply_shortcuts(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_session_state() != 0 || diagnostic_shortcuts {
            return;
        }
        let start = ui.get_start_shortcut().to_string();
        let stop = ui.get_stop_shortcut().to_string();
        if let Err(error) = native::RecordingShortcut::validate(&start, &stop) {
            ui.set_shortcut_status(error.into());
            return;
        }
        let previous_start = ui.get_active_start_shortcut().to_string();
        let previous_stop = ui.get_active_stop_shortcut().to_string();
        let had_shortcuts = shortcut_holder.borrow_mut().take().is_some();
        match register_shortcuts(&ui, &start, &stop) {
            Ok(shortcut) => {
                *shortcut_holder.borrow_mut() = Some(shortcut);
                ui.set_active_start_shortcut(start.clone().into());
                ui.set_active_stop_shortcut(stop.clone().into());
                ui.set_shortcut_status(shortcut_status(&start, &stop).into());
                persist_preferences(&ui, &shortcut_state.lock().unwrap());
            }
            Err(error) => {
                let restored = if had_shortcuts {
                    match register_shortcuts(&ui, &previous_start, &previous_stop) {
                        Ok(shortcut) => {
                            *shortcut_holder.borrow_mut() = Some(shortcut);
                            format!(
                                "Previous shortcuts remain active.\n{}",
                                shortcut_status(&previous_start, &previous_stop)
                            )
                        }
                        Err(restore_error) => format!("Shortcuts inactive: {restore_error}"),
                    }
                } else {
                    "Shortcuts inactive.".into()
                };
                ui.set_shortcut_status(format!("{error}\n{restored}").into());
            }
        }
    });
    let close_state = state.clone();
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        let mut state = close_state.lock().unwrap();
        if state.session.state() == SessionState::Idle {
            if let Some(ui) = weak.upgrade() {
                persist_preferences(&ui, &state);
            }
            return CloseRequestResponse::HideWindow;
        }
        state.close_after_save = true;
        state.session.stop();
        if let Some(recording) = &state.recording {
            recording.stop();
        }
        if let Some(ui) = weak.upgrade() {
            ui.set_session_state(3);
            notify(&ui, "Saving before closing…", false);
        }
        CloseRequestResponse::KeepWindowShown
    });

    ui.show()?;
    assign_destination(&ui, &mut state.lock().unwrap());
    let startup_state = state.clone();
    let startup_weak = ui.as_weak();
    let startup_tray = tray.clone();
    Timer::single_shot(Duration::from_millis(100), move || {
        let Some(ui) = startup_weak.upgrade() else {
            return;
        };
        match RecordingTray::new(&ui) {
            Ok(tray) => *startup_tray.borrow_mut() = Some(tray),
            Err(error) => {
                ui.set_auto_minimize(false);
                notify(
                    &ui,
                    format!("Could not create recording tray controls: {error}"),
                    true,
                );
            }
        }
        let state = startup_state;
        refresh_sources(&ui, &state);
        if let Ok(owner) = hwnd(&ui) {
            native::dark_titlebar(owner);
            let own_window_diagnostic = cfg!(feature = "diagnostics")
                && std::env::args().any(|arg| arg == "--self-test-record");
            if !own_window_diagnostic && let Err(error) = native::exclude_from_capture(owner) {
                notify(
                    &ui,
                    format!("Could not exclude studio from display capture: {error}"),
                    true,
                );
            }
        }

        let primary = {
            let state = state.lock().unwrap();
            state
                .sources
                .iter()
                .position(|source| source.primary)
                .or_else(|| state.sources.iter().position(|source| source.display))
        };
        if let Some(index) = primary {
            ui.invoke_select_source(index as i32);
        }
    });
    let weak = ui.as_weak();
    ui.on_tray_show(move || {
        if let Some(ui) = weak.upgrade() {
            restore_studio(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_tray_stop(move || {
        if let Some(ui) = weak.upgrade()
            && matches!(ui.get_session_state(), 1 | 2)
        {
            ui.invoke_toggle_recording();
        }
    });
    let weak = ui.as_weak();
    let encoder_state = state.clone();
    ui.on_encoder_selected(move |index| {
        if let Some(ui) = weak.upgrade() {
            let state = encoder_state.lock().unwrap();
            let choices = encoder_choices(&ui, &state);
            if let Some(&choice) = usize::try_from(index)
                .ok()
                .and_then(|index| choices.get(index))
            {
                ui.set_encoder_choice(choice as i32);
                update_encoding_labels(&ui, &state);
                persist_preferences(&ui, &state);
            }
        }
    });
    let weak = ui.as_weak();
    let settings_state = state.clone();
    let settings_timer = std::rc::Rc::new(Timer::default());
    let settings_save_timer = settings_timer.clone();
    ui.on_settings_changed(move || {
        if let Some(ui) = weak.upgrade() {
            if ui.get_session_state() != 0 {
                return;
            }
            let mut state = settings_state.lock().unwrap();
            if ui.get_audio_ready() && ui.get_desktop_audio_device() == 0 {
                state.desktop_device = None;
            } else if let Some(device) = usize::try_from(ui.get_desktop_audio_device() - 1)
                .ok()
                .and_then(|index| state.desktop_devices.get(index))
            {
                state.desktop_device = Some(device.id.clone());
            }
            if ui.get_audio_ready() && ui.get_microphone_audio_device() == 0 {
                state.microphone_device = None;
            } else if let Some(device) = usize::try_from(ui.get_microphone_audio_device() - 1)
                .ok()
                .and_then(|index| state.microphone_devices.get(index))
            {
                state.microphone_device = Some(device.id.clone());
            }
            state.preview_channel.set_cursor(ui.get_capture_cursor());
            update_encoding_labels(&ui, &state);
            drop(state);
            let save_weak = ui.as_weak();
            let save_state = settings_state.clone();
            settings_save_timer.start(
                TimerMode::SingleShot,
                Duration::from_millis(500),
                move || {
                    if let Some(ui) = save_weak.upgrade() {
                        persist_preferences(&ui, &save_state.lock().unwrap());
                    }
                },
            );
        }
    });
    let weak = ui.as_weak();
    let hardware_state = state.clone();
    std::thread::Builder::new()
        .name("fastrecorder-capabilities".into())
        .spawn(move || {
            let inventory = native::gpu_inventory().map_err(|e| e.to_string());
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let mut state = hardware_state.lock().unwrap();
                match inventory {
                    Ok(gpus) => {
                        let names = std::iter::once(slint::SharedString::from("Automatic"))
                            .chain(gpus.iter().map(|gpu| gpu.name.clone().into()))
                            .collect::<Vec<_>>();
                        ui.set_gpu_names(std::rc::Rc::new(slint::VecModel::from(names)).into());
                        ui.set_hardware_details(
                            gpus.iter()
                                .filter_map(|gpu| {
                                    let codecs = gpu
                                        .codecs
                                        .iter()
                                        .zip(["AV1", "HEVC", "H.264"])
                                        .filter_map(|(available, name)| available.then_some(name))
                                        .collect::<Vec<_>>();
                                    (!codecs.is_empty())
                                        .then(|| format!("{}\n{}", gpu.name, codecs.join(" · ")))
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n")
                                .into(),
                        );
                        state.gpus = gpus;
                        if let Some(name) = state.preferred_gpu.take() {
                            ui.set_gpu_choice(
                                state
                                    .gpus
                                    .iter()
                                    .position(|gpu| gpu.name == name)
                                    .map_or(0, |index| index as i32 + 1),
                            );
                        }
                        if let Some(choice) = state.preferred_encoder.take() {
                            ui.set_encoder_choice(choice);
                        }
                        update_encoding_labels(&ui, &state);
                    }
                    Err(error) => {
                        ui.set_hardware_details(error.clone().into());
                        notify(&ui, error, true);
                    }
                }
                ui.set_hardware_ready(true);
            });
        })?;
    let preview_timer = Timer::default();
    let weak = ui.as_weak();
    let preview_state = state.clone();
    preview_timer.start(TimerMode::Repeated, Duration::from_millis(83), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let channel = preview_state.lock().unwrap().preview_channel.clone();
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
                let mut state = preview_state.lock().unwrap();
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
    #[cfg(feature = "diagnostics")]
    diagnostics(&ui, state.clone());
    slint::run_event_loop()?;
    timer.stop();
    settings_timer.stop();
    preview_timer.stop();
    if let Some(mut preview) = state.lock().unwrap().preview.take() {
        preview.join();
    }
    if let Some(mut recording) = state.lock().unwrap().recording.take() {
        recording.join();
    }
    #[cfg(feature = "diagnostics")]
    if let Some(error) = state.lock().unwrap().diagnostic_failure.take() {
        return Err(error.into());
    }
    Ok(())
}

fn handle_event(
    ui: &MainWindow,
    tray: &TrayHandle,
    event_state: &Arc<Mutex<AppState>>,
    event: RecordingEvent,
) {
    let mut state = event_state.lock().unwrap();
    match event {
        RecordingEvent::Started {
            hardware: _,
            encoder,
            gpu,
            codec,
            fallback,
            bitrate_mbps,
            audio,
        } => {
            ui.set_bitrate_mbps(bitrate_mbps as i32);
            state.session.started();
            if matches!(
                state.session.state(),
                SessionState::Starting | SessionState::Recording
            ) {
                ui.set_session_state(2);
                if ui.get_auto_minimize() && tray.borrow().is_some() {
                    ui.window().set_minimized(true);
                }
            }
            if let Some(tray) = tray.borrow().as_ref() {
                tray.update(state.session.state(), "00:00");
            }
            state.started = Some(Instant::now());
            ui.set_encoder_label(encoder.clone().into());
            ui.set_gpu_label(gpu.clone().into());
            ui.set_codec_label(codec.clone().into());
            ui.set_audio_details(audio.into());
            let rate = if encoder.starts_with("NVIDIA") && ui.get_quality_mode() {
                format!("CQP {} · variable bitrate", ui.get_quality_level())
            } else {
                format!("{bitrate_mbps} Mbps target")
            };
            ui.set_active_encoder(
                format!(
                    "{encoder}\nGPU: {gpu}\nCodec: {codec} · {rate} · {} fps",
                    ui.get_fps()
                )
                .into(),
            );
            ui.set_fallback_detail(fallback.clone().unwrap_or_else(|| "None".into()).into());
            notify(ui, "", false);
            #[cfg(feature = "diagnostics")]
            println!("ENCODER {encoder}");
        }
        RecordingEvent::Statistics { dropped } => {
            ui.set_recording_stats(format!("{dropped} frames skipped").into());
            if dropped > 0 && state.session.state() == SessionState::Recording {
                notify(
                    ui,
                    "Recording is struggling to keep up. Try 30 fps or a smaller capture area.",
                    false,
                );
            }
        }
        RecordingEvent::Finished { file, error } => {
            state.session.finished();
            state.started = None;
            ui.set_session_state(0);
            ui.set_window_title("FastRecorder".into());
            if let Some(tray) = tray.borrow().as_ref() {
                tray.update(SessionState::Idle, "");
            }
            ui.set_preview_ready(false);
            ui.set_preview_image(slint::Image::default());
            if !state.close_after_save {
                restore_studio(ui);
            }
            state.preview_channel = native::PreviewChannel::default();
            state.preview_channel.set_cursor(ui.get_capture_cursor());
            if let Some(candidate) = &state.selected_candidate {
                match candidate.open() {
                    Ok(source) => state.source = Some(source),
                    Err(_) => {
                        state.source = None;
                        state.selected_candidate = None;
                        ui.set_source_selected(false);
                        ui.set_selected_source(-1);
                        ui.set_preview_error("The source closed. Choose another source.".into());
                    }
                }
            }
            if !state.close_after_save
                && let Some(source) = state.source.clone()
            {
                match native::Preview::start(source, state.preview_channel.clone()) {
                    Ok(preview) => state.preview = Some(preview),
                    Err(error) => ui.set_preview_error(error.into()),
                }
            }
            if let Some(file) = file {
                ui.set_last_recording(
                    file.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                        .into(),
                );
                ui.set_has_recording(true);
                state.last_file = Some(file.clone());
                assign_destination(ui, &mut state);
                persist_preferences(ui, &state);
                notify(
                    ui,
                    error
                        .as_ref()
                        .map(|e| format!("Saved, but recording ended early: {e}"))
                        .unwrap_or_else(|| "Recording saved.".into()),
                    error.is_some(),
                );
                #[cfg(feature = "diagnostics")]
                println!("SAVED {}", file.display());
            } else {
                notify(
                    ui,
                    error
                        .clone()
                        .unwrap_or_else(|| "No recording was saved.".into()),
                    true,
                );
                #[cfg(feature = "diagnostics")]
                eprintln!(
                    "RECORDING FAILED: {}",
                    error.as_deref().unwrap_or("No file")
                );
                #[cfg(feature = "diagnostics")]
                if state.diagnostic {
                    state.diagnostic_failure = error;
                }
            }
            #[cfg(feature = "diagnostics")]
            if state.diagnostic {
                let _ = slint::quit_event_loop();
            }
            if state.close_after_save {
                let _ = slint::quit_event_loop();
            }
        }
    }
}

#[cfg(feature = "diagnostics")]
fn diagnostics(ui: &MainWindow, state: Arc<Mutex<AppState>>) {
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
            if std::env::args().any(|arg| arg == "--60fps") {
                ui.set_fps(60);
            }
            let mut app_state = state.lock().unwrap();
            app_state.source = Some(source);
            app_state.selected_candidate = None;
            app_state.destination = Some(path.into());
            app_state.custom_destination = true;
            app_state.diagnostic = true;
            drop(app_state);
            ui.invoke_toggle_recording();
            if std::env::args().any(|arg| arg == "--resize-source") {
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
                state.lock().unwrap().diagnostic_failure =
                    Some("Diagnostic recording timed out.".into());
                let _ = slint::quit_event_loop();
            });
        } else {
            let _ = slint::quit_event_loop();
        }
    });
}
