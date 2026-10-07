//! The live preview is drawn by the capture GPU straight into a child window
//! laid over the studio's preview area. Frames never reach the CPU, and the UI
//! does not redraw for them: the capture thread blits with the D3D11 video
//! processor into a flip-model swap chain and presents.
use std::sync::Mutex;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, *},
            Gdi::{CreateRoundRectRgn, SetWindowRgn},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    core::{Interface, Result as WinResult, w},
};

/// Where producers should present: the child window and its size in pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Target {
    pub hwnd: usize,
    pub width: u32,
    pub height: u32,
    /// Letterbox colour, matching the studio theme.
    pub background: [f32; 3],
}
static TARGET: Mutex<Option<Target>> = Mutex::new(None);

pub(crate) fn target() -> Option<Target> {
    *TARGET.lock().ok()?
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // Let clicks and the cursor pass through to the studio underneath.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// UI-thread handle to the preview child window.
pub struct PreviewSurface {
    hwnd: HWND,
    placed: Option<(i32, i32, i32, i32, i32)>,
    visible: bool,
}
impl PreviewSurface {
    /// Creates the (hidden) child window inside the studio window `parent`.
    pub fn new(parent: usize) -> Result<Self, String> {
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("PipPreviewSurface"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&class); // Fails harmlessly if already registered.
            let parent = HWND(parent as *mut _);
            // Keep the studio's own drawing out of the preview's area.
            let style = GetWindowLongPtrW(parent, GWL_STYLE);
            SetWindowLongPtrW(parent, GWL_STYLE, style | WS_CLIPCHILDREN.0 as isize);
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("PipPreviewSurface"),
                w!("Pip preview"),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                16,
                16,
                Some(parent),
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| e.to_string())?;
            Ok(Self {
                hwnd,
                placed: None,
                visible: false,
            })
        }
    }

    /// Hides the preview immediately (it reappears on the next visible `place`).
    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }

    /// Positions the preview in physical pixels of the parent's client area.
    /// Producers start presenting once it is visible.
    pub fn place(
        &mut self,
        (x, y, width, height): (i32, i32, i32, i32),
        radius: i32,
        background: [u8; 3],
        visible: bool,
    ) {
        let width = width.max(16);
        let height = height.max(16);
        unsafe {
            if self.placed != Some((x, y, width, height, radius)) {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    x,
                    y,
                    width,
                    height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let region =
                    CreateRoundRectRgn(0, 0, width + 1, height + 1, radius * 2, radius * 2);
                SetWindowRgn(self.hwnd, Some(region), true); // The window owns the region.
                self.placed = Some((x, y, width, height, radius));
            }
            if visible != self.visible {
                let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
                self.visible = visible;
            }
        }
        let next = Target {
            hwnd: self.hwnd.0 as usize,
            width: width as u32,
            height: height as u32,
            background: background.map(|c| f32::from(c) / 255.),
        };
        if let Ok(mut target) = TARGET.lock()
            && *target != Some(next)
        {
            *target = Some(next);
        }
    }
}
impl Drop for PreviewSurface {
    fn drop(&mut self) {
        if let Ok(mut target) = TARGET.lock() {
            *target = None;
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Capture-thread side: a swap chain on the capture device for the child window.
pub(crate) struct Presenter {
    swapchain: IDXGISwapChain1,
    pub view: ID3D11VideoProcessorOutputView,
    pub target: Target,
}
impl Presenter {
    pub(crate) fn new(
        device: &ID3D11Device,
        video_device: &ID3D11VideoDevice,
        enumerator: &ID3D11VideoProcessorEnumerator,
        target: Target,
    ) -> WinResult<Self> {
        unsafe {
            let factory: IDXGIFactory2 = device.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: target.width,
                Height: target.height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: 0,
                ..Default::default()
            };
            let swapchain = factory.CreateSwapChainForHwnd(
                device,
                HWND(target.hwnd as *mut _),
                &desc,
                None,
                None,
            )?;
            // The studio handles Alt+Enter and friends itself.
            let _ = factory.MakeWindowAssociation(
                HWND(target.hwnd as *mut _),
                DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_WINDOW_CHANGES,
            );
            let buffer: ID3D11Texture2D = swapchain.GetBuffer(0)?;
            let mut view = None;
            video_device.CreateVideoProcessorOutputView(
                &buffer,
                enumerator,
                &D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                },
                Some(&mut view),
            )?;
            Ok(Self {
                swapchain,
                view: view.unwrap(),
                target,
            })
        }
    }
    /// Shows the frame just written to the back buffer without waiting for vsync.
    pub(crate) fn present(&self) -> WinResult<()> {
        unsafe { self.swapchain.Present(0, DXGI_PRESENT(0)).ok() }
    }
}
