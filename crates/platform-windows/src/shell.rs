//! Windows shell integration: COM apartment, dialogs, window styling, Explorer.
use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::HWND,
        System::{Com::*, WinRT::*},
        UI::Shell::*,
    },
    core::{HSTRING, PCWSTR, Result as WinResult, w},
};

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

/// A new MP4 path in `directory` named from `pattern` (see
/// `fastrecorder_core::file_name`); a numeric suffix avoids overwriting.
pub fn timestamped_destination(directory: &Path, pattern: &str, source: &str) -> PathBuf {
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let stem = fastrecorder_core::file_name(
        pattern,
        fastrecorder_core::LocalTime {
            year: time.wYear,
            month: time.wMonth,
            day: time.wDay,
            hour: time.wHour,
            minute: time.wMinute,
            second: time.wSecond,
        },
        source,
    );
    let mut path = directory.join(format!("{stem}.mp4"));
    let mut suffix = 2;
    while path.exists() {
        path = directory.join(format!("{stem}-{suffix}.mp4"));
        suffix += 1;
    }
    path
}

/// Startup failures can occur before the Slint window exists.
pub fn show_details(hwnd: usize, message: &str) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            Some(HWND(hwnd as *mut _)),
            &HSTRING::from(message),
            w!("Pip"),
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

/// Whether Windows apps are set to dark mode (Settings → Personalization → Colors).
pub fn system_prefers_dark() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    result.is_ok() && value == 0
}

/// Match the native title bar to the studio's light or dark theme.
pub fn set_dark_titlebar(hwnd: usize, dark: bool) {
    unsafe {
        let enabled = windows::core::BOOL(dark.into());
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&enabled as *const windows::core::BOOL).cast(),
            std::mem::size_of_val(&enabled) as u32,
        );
    }
}

/// Process priority: 0 normal, 1 above normal, 2 high. Higher priority helps
/// recordings keep up while a game or heavy app uses the CPU.
pub fn set_process_priority(level: i32) {
    use windows::Win32::System::Threading::{
        ABOVE_NORMAL_PRIORITY_CLASS, GetCurrentProcess, HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        SetPriorityClass,
    };
    let class = match level {
        1 => ABOVE_NORMAL_PRIORITY_CLASS,
        2 => HIGH_PRIORITY_CLASS,
        _ => NORMAL_PRIORITY_CLASS,
    };
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), class);
    }
}

/// Open a folder in Explorer.
pub fn open_folder(path: &Path) -> Result<(), String> {
    open_recording(path).map_err(|_| "Windows could not open this folder.".to_string())
}

/// Open an https link (community pages) in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Only https links can be opened.".into());
    }
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(url),
            None,
            None,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        Err(format!(
            "Windows could not open the link ({})",
            result.0 as isize
        ))
    } else {
        Ok(())
    }
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

/// Folder picker for where new recordings are saved.
pub fn choose_folder(hwnd: usize, current: Option<&Path>) -> Result<Option<PathBuf>, String> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
        dialog
            .SetTitle(w!("Choose where to save recordings"))
            .map_err(|e| e.to_string())?;
        dialog
            .SetOkButtonLabel(w!("Save recordings here"))
            .map_err(|e| e.to_string())?;
        dialog
            .SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR)
            .map_err(|e| e.to_string())?;
        if let Some(current) = current
            && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                &HSTRING::from(current.as_os_str()),
                None,
            )
        {
            let _ = dialog.SetFolder(&item);
        }
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
