use fastrecorder_core::{EncoderPreference, RecordingConfig, frame_time};
use std::{
    mem::ManuallyDrop,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::*,
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{HWND, RECT},
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
        Media::MediaFoundation::*,
        System::{
            Com::*,
            WinRT::{Direct3D11::*, Graphics::Capture::IGraphicsCaptureItemInterop, *},
        },
        UI::Shell::{Common::COMDLG_FILTERSPEC, *},
    },
    core::{HSTRING, IInspectable, Interface, PCWSTR, Result as WinResult, factory, w},
};

#[derive(Clone)]
pub struct Source {
    pub(crate) item: GraphicsCaptureItem,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

impl Source {
    pub(crate) fn from_item(item: GraphicsCaptureItem) -> WinResult<Self> {
        let size = item.Size()?;
        Ok(Self {
            name: item.DisplayName()?.to_string(),
            width: size.Width.max(0) as u32,
            height: size.Height.max(0) as u32,
            item,
        })
    }
}

pub struct UiApartment;
impl Drop for UiApartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

pub fn initialize_ui() -> WinResult<UiApartment> {
    unsafe {
        RoInitialize(RO_INIT_SINGLETHREADED)?;
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("neerajsunil.FastRecorder"));
    }
    Ok(UiApartment)
}
pub fn capture_supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

/// Windows owns borderless access. Request once per process, on an MTA worker,
/// before either preview or recording starts. Never repeatedly prompt on source changes.
pub(crate) fn configure_capture_session(session: &GraphicsCaptureSession) -> WinResult<()> {
    static REQUESTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    REQUESTED.get_or_init(|| {
        if let Ok(request) =
            GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
            && let Err(error) = request.join()
        {
            eprintln!("Borderless capture access: {error}");
        }
    });
    // Borderless access is optional. A denied/unsupported request must not
    // prevent recording; Windows keeps its border in that case.
    if let Err(error) = session.SetIsBorderRequired(false) {
        eprintln!("Borderless capture unavailable: {error}");
    }
    Ok(())
}

pub fn timestamped_destination(directory: &Path) -> PathBuf {
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let stem = format!(
        "{:04}-{:02}-{:02}-{:02}-{:02}-{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    );
    let mut path = directory.join(format!("{stem}.mp4"));
    let mut suffix = 2;
    while path.exists() {
        path = directory.join(format!("{stem}-{suffix}.mp4"));
        suffix += 1;
    }
    path
}

pub fn show_details(hwnd: usize, message: &str) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            Some(HWND(hwnd as *mut _)),
            &HSTRING::from(message),
            w!("FastRecorder"),
            windows::Win32::UI::WindowsAndMessaging::MB_OK
                | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
        );
    }
}

/// Keep the studio out of display captures and avoid preview recursion.
pub fn exclude_from_capture(hwnd: usize) -> WinResult<()> {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowDisplayAffinity(
            HWND(hwnd as *mut _),
            windows::Win32::UI::WindowsAndMessaging::WDA_EXCLUDEFROMCAPTURE,
        )
    }
}

pub fn dark_titlebar(hwnd: usize) {
    unsafe {
        let enabled = windows::core::BOOL(1);
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&enabled as *const windows::core::BOOL).cast(),
            std::mem::size_of_val(&enabled) as u32,
        );
    }
}

pub fn default_recording_directory() -> Result<PathBuf, String> {
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None)
            .map_err(|e| e.to_string())?;
        let result = path
            .to_string()
            .map(|p| PathBuf::from(p).join("FastRecorder"))
            .map_err(|e| e.to_string());
        CoTaskMemFree(Some(path.0.cast()));
        let directory = result?;
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        Ok(directory)
    }
}

pub fn preferences_path() -> Result<PathBuf, String> {
    unsafe {
        let raw = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None)
            .map_err(|e| e.to_string())?;
        let result = raw
            .to_string()
            .map(PathBuf::from)
            .map_err(|e| e.to_string());
        CoTaskMemFree(Some(raw.0.cast()));
        Ok(result?.join("FastRecorder").join("preferences.json"))
    }
}

pub fn save_preferences(bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let path = preferences_path()?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            windows::Win32::Storage::FileSystem::MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                windows::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
            .map_err(|e| e.to_string())
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Use the shell's association / PIDL API rather than constructing Explorer commands.
pub fn open_recording(path: &Path) -> Result<(), String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            None,
            None,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        Err(format!(
            "Windows could not open this recording ({})",
            result.0 as isize
        ))
    } else {
        Ok(())
    }
}
pub fn reveal_recording(path: &Path) -> Result<(), String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let item = ILCreateFromPathW(PCWSTR(wide.as_ptr()));
        if item.is_null() {
            return Err("The recording is no longer available.".into());
        }
        let result = SHOpenFolderAndSelectItems(item, None, 0).map_err(|e| e.to_string());
        ILFree(Some(item));
        result
    }
}

fn available_disk_space(path: &Path) -> Result<u64, String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0;
    unsafe {
        windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut available),
            None,
            None,
        )
        .map_err(|e| format!("Could not check free disk space: {e}"))?;
    }
    Ok(available)
}

/// Diagnostic capture is restricted to the app's own window.
pub fn own_window_source(hwnd: usize) -> WinResult<Source> {
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    Source::from_item(unsafe { interop.CreateForWindow(HWND(hwnd as *mut _))? })
}

pub fn choose_destination(hwnd: usize, suggestion: &str) -> Result<Option<PathBuf>, String> {
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
        dialog
            .SetTitle(w!("Save your recording"))
            .map_err(|e| e.to_string())?;
        dialog
            .SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("MP4 video"),
                pszSpec: w!("*.mp4"),
            }])
            .map_err(|e| e.to_string())?;
        dialog
            .SetDefaultExtension(w!("mp4"))
            .map_err(|e| e.to_string())?;
        dialog
            .SetFileName(&HSTRING::from(suggestion))
            .map_err(|e| e.to_string())?;
        dialog
            .SetOptions(
                FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR | FOS_OVERWRITEPROMPT,
            )
            .map_err(|e| e.to_string())?;
        match dialog.Show(Some(HWND(hwnd as *mut _))) {
            Err(e) if e.code().0 as u32 == 0x800704C7 => return Ok(None),
            Err(e) => return Err(e.to_string()),
            Ok(()) => {}
        }
        let name = dialog
            .GetResult()
            .and_then(|item| item.GetDisplayName(SIGDN_FILESYSPATH))
            .map_err(|e| e.to_string())?;
        let path = name
            .to_string()
            .map(PathBuf::from)
            .map_err(|e| e.to_string());
        CoTaskMemFree(Some(name.0.cast()));
        path.map(Some)
    }
}

#[derive(Debug)]
pub enum RecordingEvent {
    Started {
        hardware: bool,
        bitrate_mbps: u32,
        encoder: String,
        gpu: String,
        codec: String,
        fallback: Option<String>,
        audio: String,
    },
    Statistics {
        dropped: u64,
    },
    Finished {
        file: Option<PathBuf>,
        error: Option<String>,
    },
}

pub struct Recording {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Recording {
    pub fn start(
        source: Source,
        config: RecordingConfig,
        preview: crate::PreviewChannel,
        events: impl Fn(RecordingEvent) + Send + 'static,
    ) -> Result<Self, String> {
        config.validate()?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("fastrecorder-recording".into())
            .spawn(move || {
                // Keep initialization and cleanup on the same MTA thread.
                let result =
                    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string());
                match result {
                    Ok(()) => {
                        let result = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) };
                        if let Err(e) = result {
                            events(RecordingEvent::Finished {
                                file: None,
                                error: Some(e.to_string()),
                            });
                        } else {
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                record(&source, &config, &worker_stop, &preview, &events)
                            })).unwrap_or_else(|_| Err(format!(
                                "Recording stopped after an internal error. An incomplete file may remain in {}. See the local diagnostic log for details.",
                                config.destination.parent().unwrap_or(Path::new(".")).display()
                            )));
                            if let Err(e) = result {
                                events(RecordingEvent::Finished {
                                    file: None,
                                    error: Some(e),
                                });
                            }
                            unsafe {
                                let _ = MFShutdown();
                            }
                        }
                        unsafe {
                            RoUninitialize();
                        }
                    }
                    Err(e) => events(RecordingEvent::Finished {
                        file: None,
                        error: Some(e),
                    }),
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn join(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Recording {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) struct Capture {
    pub(crate) pool: Direct3D11CaptureFramePool,
    pub(crate) session: GraphicsCaptureSession,
    pub(crate) item: GraphicsCaptureItem,
    pub(crate) frame_token: Option<i64>,
    pub(crate) closed_token: Option<i64>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(token) = self.frame_token {
            let _ = self.pool.RemoveFrameArrived(token);
        }
        if let Some(token) = self.closed_token {
            let _ = self.item.RemoveClosed(token);
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Conservative v1 policy: any connected HDR output blocks recording. WGC's
/// selected window may span displays or move between them.
fn ensure_sdr() -> Result<(), String> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| e.to_string())?;
        let mut adapters = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapters) {
            adapters += 1;
            let mut outputs = 0;
            while let Ok(output) = adapter.EnumOutputs(outputs) {
                outputs += 1;
                let output: IDXGIOutput6 = output
                    .cast()
                    .map_err(|_| "Cannot verify the display color mode.".to_string())?;
                let desc = output.GetDesc1().map_err(|e| e.to_string())?;
                if desc.AttachedToDesktop.as_bool()
                    && desc.ColorSpace != DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709
                {
                    return Err("This version records SDR only. Turn off HDR on your displays before recording.".into());
                }
            }
        }
        if adapters == 0 {
            return Err("No graphics adapter is available.".into());
        }
    }
    Ok(())
}

fn record(
    source: &Source,
    config: &RecordingConfig,
    stop: &AtomicBool,
    preview: &crate::PreviewChannel,
    events: &impl Fn(RecordingEvent),
) -> Result<(), String> {
    ensure_sdr()?;
    let directory = config
        .destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if available_disk_space(directory)? < 256 * 1024 * 1024 {
        return Err("Less than 256 MB is available. Choose a drive with more space.".into());
    }
    if source.width == 0 || source.height == 0 {
        return Err("The selected source has no visible content.".into());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = config.destination.with_file_name(format!(
        ".{}.{}.partial.mp4",
        config
            .destination
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy(),
        stamp
    ));
    // Reserve only our own temporary path; destination files are never overwritten.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| format!("Cannot write here: {e}"))?;
    let result = record_inner(source, config, stop, preview, events, &temporary);
    match result {
        Ok((frames, warning)) if frames > 0 => {
            move_without_overwrite(&temporary, &config.destination).map_err(|e| {
                format!(
                    "Recording saved at {}, but could not be moved: {e}",
                    temporary.display()
                )
            })?;
            events(RecordingEvent::Finished {
                file: Some(config.destination.clone()),
                error: warning,
            });
        }
        Ok((_, warning)) => {
            let _ = std::fs::remove_file(&temporary);
            events(RecordingEvent::Finished {
                file: None,
                error: Some(
                    warning
                        .unwrap_or_else(|| "Recording stopped before any frames arrived.".into()),
                ),
            });
        }
        Err(e) => {
            if std::fs::metadata(&temporary).is_ok_and(|metadata| metadata.len() == 0) {
                let _ = std::fs::remove_file(&temporary);
                return Err(e);
            }
            return Err(format!(
                "{e} Incomplete recording retained at {}",
                temporary.display()
            ));
        }
    }
    Ok(())
}

fn record_inner(
    source: &Source,
    config: &RecordingConfig,
    stop: &AtomicBool,
    preview: &crate::PreviewChannel,
    events: &impl Fn(RecordingEvent),
    temporary: &Path,
) -> Result<(u64, Option<String>), String> {
    let mut audio = if config.audio.enabled() {
        Some(crate::audio::AudioRecording::start(
            config.audio.clone(),
            temporary.with_extension("aac.partial"),
        )?)
    } else {
        None
    };
    let native = || -> WinResult<_> {
        unsafe {
            let (device, context) = crate::hardware::create_device(config.gpu_index)?;
            let dxgi: IDXGIDevice = device.cast()?;
            #[cfg(feature = "diagnostics")]
            {
                let desc = dxgi.GetAdapter()?.GetDesc()?;
                let length = desc
                    .Description
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(desc.Description.len());
                println!(
                    "CAPTURE ADAPTER {}",
                    String::from_utf16_lossy(&desc.Description[..length])
                );
            }
            let winrt: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let mut manager = None;
            let mut token = 0;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(&device, token)?;
            Ok((device, context, winrt, manager))
        }
    };
    let (device, context, winrt, manager) =
        native().map_err(|e| format!("Could not initialize graphics: {e}"))?;
    let width = (source.width + 1) & !1;
    let height = (source.height + 1) & !1;
    let input_type =
        media_type(width, height, config.fps, &MFVideoFormat_NV12).map_err(|e| e.to_string())?;
    let mut encoder = Output::new(
        &device,
        &context,
        &manager,
        temporary,
        &input_type,
        width,
        height,
        config,
    )?;
    let hardware = encoder.hardware();
    let mut preview_renderer = crate::preview::PreviewRenderer::default();
    let allocator = unsafe {
        let mut raw = std::ptr::null_mut();
        MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut raw)
            .map_err(|e| e.to_string())?;
        let allocator = IMFVideoSampleAllocatorEx::from_raw(raw);
        allocator
            .SetDirectXManager(&manager)
            .map_err(|e| e.to_string())?;
        let attrs = attributes(1).map_err(|e| e.to_string())?;
        attrs
            .SetUINT32(&MF_SA_D3D11_BINDFLAGS, D3D11_BIND_RENDER_TARGET.0 as u32)
            .map_err(|e| e.to_string())?;
        allocator
            .InitializeSampleAllocatorEx(4, 8, &attrs, &input_type)
            .map_err(|e| e.to_string())?;
        allocator
    };
    let mut converter = Converter::new(
        &device,
        &context,
        source.width,
        source.height,
        width,
        height,
        config.fps,
    )
    .map_err(|e| format!("GPU color conversion is unavailable: {e}"))?;
    let latest = Arc::new(std::sync::Mutex::new(None::<Direct3D11CaptureFrame>));
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        3,
        source.item.Size().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let session = pool
        .CreateCaptureSession(&source.item)
        .map_err(|e| e.to_string())?;
    let mut capture = Capture {
        pool,
        session,
        item: source.item.clone(),
        frame_token: None,
        closed_token: None,
    };
    let callback_latest = latest.clone();
    let frame_token = capture
        .pool
        .FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                if let Ok(frame) = pool.ok()?.TryGetNextFrame() {
                    if let Ok(mut latest) = callback_latest.lock() {
                        *latest = Some(frame);
                    }
                    let _ = wake_tx.try_send(());
                }
                Ok(())
            }),
        )
        .map_err(|e| e.to_string())?;
    capture.frame_token = Some(frame_token);
    let closed = Arc::new(AtomicBool::new(false));
    let callback_closed = closed.clone();
    let closed_token = source
        .item
        .Closed(
            &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                callback_closed.store(true, Ordering::Release);
                Ok(())
            }),
        )
        .map_err(|e| e.to_string())?;
    capture.closed_token = Some(closed_token);
    capture
        .session
        .SetIsCursorCaptureEnabled(config.capture_cursor)
        .map_err(|e| e.to_string())?;
    configure_capture_session(&capture.session)
        .map_err(|e| format!("Could not configure borderless recording: {e}"))?;
    capture.session.StartCapture().map_err(|e| e.to_string())?;
    let mut frame_wait_deadline = Instant::now() + Duration::from_secs(8);
    let mut clock: Option<Instant> = None;
    let mut index = 0;
    let mut written = 0;
    let mut video_end = 0;
    let mut dropped = 0;
    let mut last_statistics = Instant::now();
    let mut capture_size = source.item.Size().map_err(|e| e.to_string())?;
    let mut frame_valid = false;
    let gpu = unsafe {
        device
            .cast::<IDXGIDevice>()
            .and_then(|d| d.GetAdapter())
            .and_then(|a| a.GetDesc())
    }
    .map(|d| {
        String::from_utf16_lossy(&d.Description)
            .trim_end_matches('\0')
            .to_owned()
    })
    .unwrap_or_else(|_| "Unknown adapter".into());
    let mut started_event = Some(RecordingEvent::Started {
        hardware,
        bitrate_mbps: config.bitrate_for(
            match encoder.codec() {
                "AV1" => fastrecorder_core::Codec::Av1,
                "HEVC" => fastrecorder_core::Codec::Hevc,
                _ => fastrecorder_core::Codec::H264,
            },
            width,
            height,
        ),
        encoder: encoder.label().into(),
        codec: encoder.codec().into(),
        gpu,
        fallback: encoder.fallback().map(str::to_owned),
        audio: audio.as_ref().map_or_else(
            || "Audio disabled".into(),
            |audio| audio.description.clone(),
        ),
    });
    let loop_result = (|| -> Result<(), String> {
        while !stop.load(Ordering::Acquire) && !closed.load(Ordering::Acquire) {
            if let Some(error) = audio.as_ref().and_then(|audio| audio.failure()) {
                return Err(error);
            }
            if frame_valid {
                let target = Duration::from_nanos(
                    (u128::from(index) * 1_000_000_000 / u128::from(config.fps)) as u64,
                );
                let delay = target.saturating_sub(clock.unwrap().elapsed());
                if !delay.is_zero() {
                    // High-refresh sources can wake us more often than output
                    // FPS. Keep their newest frame, but copy it only when due.
                    let _ = wake_rx.recv_timeout(delay.min(Duration::from_millis(20)));
                    continue;
                }
            }
            let frame = latest
                .lock()
                .map_err(|_| "Capture synchronization failed.".to_string())?
                .take();
            if let Some(frame) = frame {
                let content = frame.ContentSize().map_err(|e| e.to_string())?;
                if content.Width > 0 && content.Height > 0 {
                    if content != capture_size {
                        drop(frame);
                        capture
                            .pool
                            .Recreate(
                                &winrt,
                                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                                3,
                                content,
                            )
                            .map_err(|e| e.to_string())?;
                        capture_size = content;
                        converter = Converter::new(
                            &device,
                            &context,
                            content.Width as u32,
                            content.Height as u32,
                            width,
                            height,
                            config.fps,
                        )
                        .map_err(|e| e.to_string())?;
                        frame_valid = false;
                        frame_wait_deadline = Instant::now() + Duration::from_secs(8);
                        continue;
                    }
                    if !converter.copy_frame(&frame).map_err(|e| e.to_string())? {
                        continue;
                    }
                    preview_renderer.update(
                        &device,
                        &context,
                        &converter.input,
                        content.Width as u32,
                        content.Height as u32,
                        preview,
                    );
                    frame_valid = true;
                    if clock.is_none() {
                        if let Some(audio) = &audio {
                            audio.begin()?;
                        }
                        clock = Some(Instant::now());
                    }
                }
            }
            if !frame_valid {
                if Instant::now() > frame_wait_deadline {
                    return Err(
                        "No frames arrived. The source may be minimized or unavailable.".into(),
                    );
                }
                let _ = wake_rx.recv_timeout(Duration::from_millis(20));
                continue;
            }
            let elapsed = clock.unwrap().elapsed();
            let due = frame_time(index, config.fps);
            if elapsed.as_nanos() / 100 >= due as u128 {
                // Never build a catch-up queue: skip missed ticks while preserving wall time.
                let now_index =
                    (elapsed.as_nanos() * u128::from(config.fps) / 1_000_000_000) as u64;
                if now_index > index && written > 0 {
                    dropped += now_index - index;
                    index = now_index;
                }
                let sample = unsafe { allocator.AllocateSample() };
                match sample {
                    Ok(sample) => {
                        converter.convert(&sample).map_err(|e| e.to_string())?;
                        let sample = if !encoder.cpu_input() {
                            sample
                        } else {
                            converter.cpu_sample(&sample).map_err(|e| e.to_string())?
                        };
                        unsafe {
                            sample
                                .SetSampleTime(frame_time(index, config.fps))
                                .map_err(|e| e.to_string())?;
                            sample
                                .SetSampleDuration(
                                    frame_time(index + 1, config.fps)
                                        - frame_time(index, config.fps),
                                )
                                .map_err(|e| e.to_string())?;
                            encoder.write(&sample, index, config.fps)?;
                        }
                        written += 1;
                        video_end = index + 1;
                        // Don't minimize the studio or report recording before
                        // capture supplies a frame and encoding accepts it.
                        if let Some(event) = started_event.take() {
                            events(event);
                        }
                    }
                    Err(e) if e.code() == MF_E_SAMPLEALLOCATOR_EMPTY => {
                        dropped += 1;
                    }
                    Err(e) => return Err(e.to_string()),
                }
                index += 1;
            }
            if last_statistics.elapsed() >= Duration::from_secs(2) {
                let directory = temporary
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                if available_disk_space(directory)? < 64 * 1024 * 1024 {
                    return Err(
                        "Recording stopped because the drive is running out of space.".into(),
                    );
                }
                ensure_sdr()?;
                events(RecordingEvent::Statistics { dropped });
                last_statistics = Instant::now();
            }
            let target = Duration::from_nanos(
                (u128::from(index) * 1_000_000_000 / u128::from(config.fps)) as u64,
            );
            let sleep = target
                .saturating_sub(clock.unwrap().elapsed())
                .min(Duration::from_millis(20));
            match wake_rx.recv_timeout(sleep) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("Capture unexpectedly disconnected.".into());
                }
            }
        }
        if closed.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
            return Err("The capture source closed.".into());
        }
        Ok(())
    })();
    drop(capture);
    // Drop checked-out WGC frames before finalization and device shutdown.
    if let Ok(mut latest) = latest.lock() {
        *latest = None;
    }
    let audio = audio.as_mut().map(|audio| {
        audio.finish(video_end * u64::from(crate::audio::RATE) / u64::from(config.fps))
    });
    let audio_error = audio.as_ref().and_then(|audio| audio.error.clone());
    let warning = match (loop_result.err(), audio_error) {
        (Some(video), Some(audio)) if video != audio => Some(format!("{video}; {audio}")),
        (video, audio) => video.or(audio),
    };
    if written == 0 {
        return Ok((0, warning));
    }
    encoder
        .finalize(
            temporary,
            audio.as_ref().and_then(|audio| audio.track.as_ref()),
        )
        .map_err(|e| {
            format!(
                "{}Could not finish the MP4: {e}",
                warning
                    .as_ref()
                    .map(|e| format!("{e} "))
                    .unwrap_or_default()
            )
        })?;
    Ok((written, warning))
}

enum Output {
    Nvenc(Box<crate::nvenc::Nvenc>),
    Intel(Box<crate::intel::Intel>),
    SoftwareAv1(Box<crate::software_av1::SoftwareAv1>),
    Mf {
        writer: IMFSinkWriter,
        stream: u32,
        hardware: bool,
        label: String,
        fallback: Option<String>,
    },
}
impl Output {
    #[allow(clippy::too_many_arguments)] // Native graphics handles and media setup share this boundary.
    fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        manager: &IMFDXGIDeviceManager,
        path: &Path,
        input: &IMFMediaType,
        width: u32,
        height: u32,
        config: &RecordingConfig,
    ) -> Result<Self, String> {
        let mut fallback = None;
        let automatic = config.encoder == EncoderPreference::Auto;
        let adapter: windows::Win32::Graphics::Dxgi::IDXGIDevice =
            device.cast().map_err(|e| e.to_string())?;
        let vendor = unsafe { adapter.GetAdapter().and_then(|a| a.GetDesc()) }
            .map_err(|e| e.to_string())?
            .VendorId;
        let candidates: Vec<fastrecorder_core::Codec> = if automatic {
            vec![
                fastrecorder_core::Codec::Av1,
                fastrecorder_core::Codec::Hevc,
                fastrecorder_core::Codec::H264,
            ]
        } else {
            vec![config.encoder.codec()]
        };
        if automatic
            || matches!(
                config.encoder,
                EncoderPreference::NvencAv1
                    | EncoderPreference::NvencHevc
                    | EncoderPreference::NvencH264
            )
        {
            if vendor == 0x10de {
                for codec in &candidates {
                    match crate::nvenc::Nvenc::new(
                        device,
                        context,
                        path,
                        width,
                        height,
                        config.fps,
                        config.bitrate_for(*codec, width, height) * 1_000_000,
                        config.nvenc_preset,
                        config.keyframe_seconds,
                        config.constant_bitrate,
                        config.quality_qp,
                        *codec,
                    ) {
                        Ok(mut encoder) => {
                            encoder.fallback = match (fallback, encoder.fallback.take()) {
                                (Some(startup), Some(quality)) => {
                                    Some(format!("{startup}; {quality}"))
                                }
                                (startup, quality) => startup.or(quality),
                            };
                            return Ok(Self::Nvenc(Box::new(encoder)));
                        }
                        Err(error) if !automatic => return Err(error),
                        Err(error) => {
                            fallback = Some(format!(
                                "{}; NVENC {}: {error}",
                                fallback.unwrap_or_default(),
                                codec.name()
                            ))
                        }
                    }
                }
            } else if !automatic {
                return Err("NVENC requires an NVIDIA adapter".into());
            }
        }
        if automatic && vendor == 0x8086 {
            for codec in &candidates {
                match crate::intel::Intel::new(device, path, width, height, config, *codec) {
                    Ok(mut encoder) => {
                        encoder.fallback = fallback;
                        return Ok(Self::Intel(Box::new(encoder)));
                    }
                    Err(error) => {
                        fallback = Some(format!(
                            "{}; Intel {}: {error}",
                            fallback.unwrap_or_default(),
                            codec.name()
                        ))
                    }
                }
            }
        }
        let bitrate = config.bitrate_for(fastrecorder_core::Codec::H264, width, height) * 1_000_000;
        if config.encoder == EncoderPreference::SoftwareAv1 {
            return crate::software_av1::SoftwareAv1::new(path, width, height, config)
                .map(|encoder| Self::SoftwareAv1(Box::new(encoder)));
        }
        if matches!(
            config.encoder,
            EncoderPreference::IntelAv1
                | EncoderPreference::IntelHevc
                | EncoderPreference::IntelH264
        ) {
            return crate::intel::Intel::new(
                device,
                path,
                width,
                height,
                config,
                config.encoder.codec(),
            )
            .map(|encoder| Self::Intel(Box::new(encoder)));
        }
        if config.encoder != EncoderPreference::SoftwareOnly {
            match create_writer(
                path,
                input,
                width,
                height,
                config.fps,
                Some(manager),
                bitrate,
            ) {
                Ok((writer, stream, label)) => {
                    return Ok(Self::Mf {
                        writer,
                        stream,
                        hardware: true,
                        label: format!("Media Foundation · {label} · H.264 hardware"),
                        fallback,
                    });
                }
                Err(error) if config.encoder == EncoderPreference::HardwareH264 => {
                    return Err(format!("Hardware H.264 unavailable: {error}"));
                }
                Err(error) => {
                    fallback = Some(format!(
                        "{}; hardware H.264 unavailable: {error}",
                        fallback.unwrap_or_default()
                    ));
                }
            }
        }
        let (writer, stream, label) =
            create_writer(path, input, width, height, config.fps, None, bitrate)
                .map_err(|e| format!("Windows software encoding could not start: {e}"))?;
        Ok(Self::Mf {
            writer,
            stream,
            hardware: false,
            label: format!("Media Foundation · {label} · H.264 software"),
            fallback,
        })
    }
    fn hardware(&self) -> bool {
        match self {
            Self::Nvenc(_) | Self::Intel(_) => true,
            Self::SoftwareAv1(_) => false,
            Self::Mf { hardware, .. } => *hardware,
        }
    }
    fn cpu_input(&self) -> bool {
        matches!(
            self,
            Self::Intel(_)
                | Self::SoftwareAv1(_)
                | Self::Mf {
                    hardware: false,
                    ..
                }
        )
    }
    fn label(&self) -> &str {
        match self {
            Self::Nvenc(encoder) => &encoder.label,
            Self::Intel(encoder) => &encoder.label,
            Self::SoftwareAv1(_) => "rav1e · AV1 software · Rust · speed 8 · 8-frame lookahead",
            Self::Mf { label, .. } => label,
        }
    }
    fn codec(&self) -> &str {
        match self {
            Self::Nvenc(encoder) => encoder.codec.name(),
            Self::Intel(encoder) => encoder.codec.name(),
            Self::SoftwareAv1(_) => "AV1",
            Self::Mf { .. } => "H.264",
        }
    }
    fn fallback(&self) -> Option<&str> {
        match self {
            Self::Nvenc(encoder) => encoder.fallback.as_deref(),
            Self::Intel(encoder) => encoder.fallback.as_deref(),
            Self::SoftwareAv1(_) => None,
            Self::Mf { fallback, .. } => fallback.as_deref(),
        }
    }
    fn write(&mut self, sample: &IMFSample, index: u64, _: u32) -> Result<(), String> {
        match self {
            Self::Nvenc(encoder) => {
                let (texture, subresource) = sample_texture(sample).map_err(|e| e.to_string())?;
                encoder.write(&texture, subresource, index)
            }
            Self::Intel(encoder) => encoder.write(sample, index),
            Self::SoftwareAv1(encoder) => encoder.write(sample, index),
            Self::Mf { writer, stream, .. } => unsafe {
                writer
                    .WriteSample(*stream, sample)
                    .map_err(|e| format!("Encoding failed: {e}"))
            },
        }
    }
    fn finalize(self, path: &Path, audio: Option<&crate::audio::AudioTrack>) -> Result<(), String> {
        match self {
            Self::Nvenc(mut encoder) => encoder.finalize(audio),
            Self::Intel(mut encoder) => encoder.finalize(audio),
            Self::SoftwareAv1(mut encoder) => encoder.finalize(audio),
            Self::Mf { writer, .. } => {
                unsafe {
                    writer.Finalize().map_err(|e| e.to_string())?;
                }
                drop(writer); // Release the native file handle before adding AAC.
                if let Some(audio) = audio {
                    crate::mp4::attach_audio(path, audio).map_err(|e| e.to_string())?;
                }
                Ok(())
            }
        }
    }
}

fn attributes(count: u32) -> WinResult<IMFAttributes> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, count)?;
        Ok(attrs.unwrap())
    }
}

fn media_type(
    width: u32,
    height: u32,
    fps: u32,
    subtype: &windows::core::GUID,
) -> WinResult<IMFMediaType> {
    unsafe {
        let media = MFCreateMediaType()?;
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        if *subtype == MFVideoFormat_NV12 {
            media.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
            media.SetUINT32(&MF_MT_SAMPLE_SIZE, width * height * 3 / 2)?;
        }
        media.SetUINT64(
            &MF_MT_FRAME_SIZE,
            (u64::from(width) << 32) | u64::from(height),
        )?;
        media.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(fps) << 32) | 1)?;
        media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1_u64 << 32) | 1)?;
        media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        media.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        media.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        media.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        Ok(media)
    }
}

fn create_writer(
    path: &Path,
    input: &IMFMediaType,
    width: u32,
    height: u32,
    fps: u32,
    manager: Option<&IMFDXGIDeviceManager>,
    bitrate: u32,
) -> WinResult<(IMFSinkWriter, u32, String)> {
    unsafe {
        let attrs = attributes(4)?;
        attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
        attrs.SetUINT32(
            &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
            u32::from(manager.is_some()),
        )?;
        if let Some(manager) = manager {
            attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, manager)?;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let writer = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, &attrs)?;
        let output = media_type(width, height, fps, &MFVideoFormat_H264)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        let stream = writer.AddStream(&output)?;
        writer.SetInputMediaType(stream, input, None)?;
        let mut label = "Windows software H.264 encoder".to_owned();
        if manager.is_some() {
            // Verify the chosen encoder instead of labelling software as hardware.
            let extended: IMFSinkWriterEx = writer.cast()?;
            let mut transform = None;
            extended.GetTransformForStream(stream, 0, None, &mut transform)?;
            let attrs = transform.unwrap().GetAttributes()?;
            attrs.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute)?;
            let mut name = windows::core::PWSTR::null();
            let mut length = 0;
            if attrs
                .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut length)
                .is_ok()
            {
                label = name
                    .to_string()
                    .unwrap_or_else(|_| "Media Foundation hardware H.264".into());
                CoTaskMemFree(Some(name.0.cast()));
            } else {
                label = "Media Foundation hardware H.264".into();
            }
        }
        writer.BeginWriting()?;
        Ok((writer, stream, label))
    }
}

struct Converter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    input: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    staging: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
}
impl Converter {
    fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        iw: u32,
        ih: u32,
        ow: u32,
        oh: u32,
        fps: u32,
    ) -> WinResult<Self> {
        unsafe {
            let video_device: ID3D11VideoDevice = device.cast()?;
            let video_context: ID3D11VideoContext = context.cast()?;
            let rate = DXGI_RATIONAL {
                Numerator: fps,
                Denominator: 1,
            };
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: iw,
                InputHeight: ih,
                OutputFrameRate: rate,
                OutputWidth: ow,
                OutputHeight: oh,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumerator = video_device.CreateVideoProcessorEnumerator(&desc)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;
            let input = texture(
                device,
                iw,
                ih,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_RENDER_TARGET.0 as u32,
                0,
            )?;
            let mut input_view = None;
            let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                ..Default::default()
            };
            video_device.CreateVideoProcessorInputView(
                &input,
                &enumerator,
                &desc,
                Some(&mut input_view),
            )?;
            let src = RECT {
                left: 0,
                top: 0,
                right: iw as i32,
                bottom: ih as i32,
            };
            let scale = (f64::from(ow) / f64::from(iw)).min(f64::from(oh) / f64::from(ih));
            let dw = (f64::from(iw) * scale).round() as i32;
            let dh = (f64::from(ih) * scale).round() as i32;
            let left = (ow as i32 - dw) / 2;
            let top = (oh as i32 - dh) / 2;
            let dst = RECT {
                left,
                top,
                right: left + dw,
                bottom: top + dh,
            };
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src));
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst));
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let color_context: ID3D11VideoContext1 = context.cast()?;
            color_context.VideoProcessorSetStreamColorSpace1(
                &processor,
                0,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            );
            color_context.VideoProcessorSetOutputColorSpace1(
                &processor,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            );
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 {
                    YCbCr: D3D11_VIDEO_COLOR_YCbCrA {
                        Y: 0.0627451,
                        Cb: 0.5,
                        Cr: 0.5,
                        A: 1.0,
                    },
                },
            };
            video_context.VideoProcessorSetOutputBackgroundColor(&processor, true, &black);
            Ok(Self {
                device: device.clone(),
                context: context.clone(),
                video_device,
                video_context,
                enumerator,
                processor,
                input,
                input_view: input_view.unwrap(),
                staging: None,
                width: ow,
                height: oh,
            })
        }
    }
    fn copy_frame(&self, frame: &Direct3D11CaptureFrame) -> WinResult<bool> {
        let surface = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?;
        let texture: ID3D11Texture2D = unsafe { surface.GetInterface()? };
        unsafe {
            let mut source = D3D11_TEXTURE2D_DESC::default();
            let mut destination = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut source);
            self.input.GetDesc(&mut destination);
            // An in-flight frame can still belong to the old pool after resize.
            if source.Width < destination.Width || source.Height < destination.Height {
                return Ok(false);
            }
            let content = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: destination.Width,
                bottom: destination.Height,
                back: 1,
            };
            self.context.CopySubresourceRegion(
                &self.input,
                0,
                0,
                0,
                0,
                &texture,
                0,
                Some(&content),
            );
        }
        Ok(true)
    }
    fn convert(&self, sample: &IMFSample) -> WinResult<()> {
        unsafe {
            let (texture, subresource) = sample_texture(sample)?;
            let mut texture_desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut texture_desc);
            let mut output_view = None;
            let desc = if texture_desc.ArraySize > 1 {
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2DARRAY,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2DArray: D3D11_TEX2D_ARRAY_VPOV {
                            MipSlice: 0,
                            FirstArraySlice: subresource / texture_desc.MipLevels,
                            ArraySize: 1,
                        },
                    },
                }
            } else {
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                }
            };
            self.video_device.CreateVideoProcessorOutputView(
                &texture,
                &self.enumerator,
                &desc,
                Some(&mut output_view),
            )?;
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(self.input_view.clone())),
                ..Default::default()
            };
            let result = self.video_context.VideoProcessorBlt(
                &self.processor,
                output_view.as_ref().unwrap(),
                0,
                std::slice::from_ref(&stream),
            );
            // Windows bindings represent COM pointers in C structs as ManuallyDrop.
            ManuallyDrop::drop(&mut stream.pInputSurface);
            result?;
            let buffer = sample.GetBufferByIndex(0)?;
            // DXGI-backed buffers start with no valid bytes. Sink Writer rejects
            // an otherwise valid GPU sample until its payload length is set.
            let length = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
            buffer.SetCurrentLength(length)
        }
    }
    fn cpu_sample(&mut self, sample: &IMFSample) -> WinResult<IMFSample> {
        unsafe {
            if self.staging.is_none() {
                self.staging = Some(texture(
                    &self.device,
                    self.width,
                    self.height,
                    DXGI_FORMAT_NV12,
                    D3D11_USAGE_STAGING,
                    0,
                    D3D11_CPU_ACCESS_READ.0 as u32,
                )?);
            }
            let staging = self.staging.as_ref().unwrap();
            let (texture, subresource) = sample_texture(sample)?;
            self.context
                .CopySubresourceRegion(staging, 0, 0, 0, 0, &texture, subresource, None);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let length = self.width * self.height * 3 / 2;
            let result = (|| {
                let buffer = MFCreateMemoryBuffer(length)?;
                let mut bytes = std::ptr::null_mut();
                buffer.Lock(&mut bytes, None, None)?;
                for row in 0..(self.height * 3 / 2) {
                    std::ptr::copy_nonoverlapping(
                        (mapped.pData as *const u8).add((row * mapped.RowPitch) as usize),
                        bytes.add((row * self.width) as usize),
                        self.width as usize,
                    );
                }
                buffer.Unlock()?;
                buffer.SetCurrentLength(length)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buffer)?;
                Ok(sample)
            })();
            self.context.Unmap(staging, 0);
            result
        }
    }
}

pub(crate) fn texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    usage: D3D11_USAGE,
    bind: u32,
    cpu: u32,
) -> WinResult<ID3D11Texture2D> {
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        let mut texture = None;
        device.CreateTexture2D(&desc, None, Some(&mut texture))?;
        Ok(texture.unwrap())
    }
}
pub(crate) fn sample_texture(sample: &IMFSample) -> WinResult<(ID3D11Texture2D, u32)> {
    unsafe {
        let buffer: IMFDXGIBuffer = sample.GetBufferByIndex(0)?.cast()?;
        let subresource = buffer.GetSubresourceIndex()?;
        let mut raw = std::ptr::null_mut();
        buffer.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
        Ok((ID3D11Texture2D::from_raw(raw), subresource))
    }
}

fn move_without_overwrite(from: &Path, to: &Path) -> WinResult<()> {
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // No MOVEFILE_REPLACE_EXISTING: a file created since validation is protected.
    unsafe {
        windows::Win32::Storage::FileSystem::MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            windows::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalization_never_replaces_a_destination_created_after_validation() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fastrecorder-move-{stamp}"));
        std::fs::create_dir(&dir).unwrap();
        let temporary = dir.join("temporary.mp4");
        let destination = dir.join("recording.mp4");
        std::fs::write(&temporary, b"new recording").unwrap();
        std::fs::write(&destination, b"existing recording").unwrap();
        assert!(move_without_overwrite(&temporary, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing recording");
        assert!(temporary.exists());
        std::fs::remove_file(&destination).unwrap();
        move_without_overwrite(&temporary, &destination).unwrap();
        assert!(!temporary.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"new recording");
        std::fs::remove_file(&destination).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
