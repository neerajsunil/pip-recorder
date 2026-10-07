//! Windows shell integration: COM apartment, dialogs, window styling, Explorer.
use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::HWND,
        System::{Com::*, WinRT::*},
        UI::Shell::{Common::COMDLG_FILTERSPEC, *},
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

/// Startup failures can occur before the Slint window exists.
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
