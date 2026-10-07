//! Enumerate metadata only. A WGC capture item is created when the user selects a row.
use crate::Source;
use windows::{
    Graphics::Capture::GraphicsCaptureItem,
    Win32::{
        Foundation::{HWND, LPARAM, RECT},
        Graphics::{
            Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
            Gdi::*,
        },
        System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop,
        UI::WindowsAndMessaging::*,
    },
    core::{BOOL, factory},
};

#[derive(Clone, PartialEq, Eq)]
enum Target {
    Display(usize),
    Window { handle: usize, process: u32 },
}

#[derive(Clone)]
pub struct SourceCandidate {
    target: Target,
    pub name: String,
    pub detail: String,
    pub display: bool,
    pub primary: bool,
}

impl SourceCandidate {
    pub fn same_target(&self, other: &Self) -> bool {
        self.target == other.target
    }

    pub fn open(&self) -> Result<Source, String> {
        let interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(|e| e.to_string())?;
        let item = unsafe {
            match self.target {
                Target::Display(handle) => interop.CreateForMonitor(HMONITOR(handle as *mut _)),
                Target::Window { handle, process } => {
                    let hwnd = HWND(handle as *mut _);
                    let mut current_process = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut current_process));
                    if !IsWindow(Some(hwnd)).as_bool()
                        || current_process != process
                        || !IsWindowVisible(hwnd).as_bool()
                        || IsIconic(hwnd).as_bool()
                    {
                        return Err(
                            "This window is unavailable. Refresh the sources and choose another."
                                .into(),
                        );
                    }
                    interop.CreateForWindow(hwnd)
                }
            }
        }
        .map_err(|e| e.to_string())?;
        let mut source = Source::from_item(item).map_err(|e| e.to_string())?;
        source.name = self.name.clone();
        source.target = Some(match self.target {
            Target::Display(handle) => crate::color::CaptureTarget::Monitor(handle),
            Target::Window { handle, .. } => crate::color::CaptureTarget::Window(handle),
        });
        Ok(source)
    }
}

struct Enumeration {
    owner: usize,
    entries: Vec<SourceCandidate>,
}

unsafe extern "system" fn monitor_callback(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // EnumDisplayMonitors calls synchronously; data points to the caller's live stack value.
    let entries = unsafe { &mut *(data.0 as *mut Enumeration) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool() {
        let rect = info.monitorInfo.rcMonitor;
        let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
        let device = String::from_utf16_lossy(&info.szDevice);
        let number = device
            .trim_end_matches('\0')
            .strip_prefix("\\\\.\\DISPLAY")
            .unwrap_or("?");
        entries.entries.push(SourceCandidate {
            target: Target::Display(monitor.0 as usize),
            name: format!("Display {number}"),
            detail: format!(
                "{} × {}{}",
                rect.right - rect.left,
                rect.bottom - rect.top,
                if primary { " · Main display" } else { "" }
            ),
            display: true,
            primary,
        });
    }
    BOOL(1)
}

unsafe extern "system" fn window_callback(hwnd: HWND, data: LPARAM) -> BOOL {
    let entries = unsafe { &mut *(data.0 as *mut Enumeration) };
    unsafe {
        if hwnd.0 as usize == entries.owner
            || !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
            || GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW.0 as isize != 0
            || GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.0.is_null())
        {
            return BOOL(1);
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        let mut affinity = 0u32;
        let _ = GetWindowDisplayAffinity(hwnd, &mut affinity);
        if cloaked != 0 || affinity != 0 {
            return BOOL(1);
        }
        let mut title = [0u16; 1024];
        let length = GetWindowTextW(hwnd, &mut title).max(0) as usize;
        let name = String::from_utf16_lossy(&title[..length]);
        if name.trim().is_empty() {
            return BOOL(1);
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err()
            || rect.right <= rect.left
            || rect.bottom <= rect.top
        {
            return BOOL(1);
        }
        let mut process = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut process));
        entries.entries.push(SourceCandidate {
            target: Target::Window {
                handle: hwnd.0 as usize,
                process,
            },
            name,
            detail: "Application window".into(),
            display: false,
            primary: false,
        });
    }
    BOOL(1)
}

pub fn enumerate_sources(owner: usize) -> Result<Vec<SourceCandidate>, String> {
    let mut enumeration = Enumeration {
        owner,
        entries: Vec::new(),
    };
    let data = LPARAM((&mut enumeration as *mut Enumeration) as isize);
    unsafe {
        if !EnumDisplayMonitors(None, None, Some(monitor_callback), data).as_bool() {
            return Err("Could not list displays.".into());
        }
        EnumWindows(Some(window_callback), data).map_err(|e| e.to_string())?;
    }
    enumeration.entries.sort_by(|a, b| {
        b.display
            .cmp(&a.display)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(enumeration.entries)
}
