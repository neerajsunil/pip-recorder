//! Windows Graphics Capture sources, sessions and frame lifetimes.
mod color;
mod sources;

pub(crate) use color::{CaptureTarget, ColorConverter, DisplayColor};
pub use sources::*;

use windows::{
    Graphics::Capture::*,
    Win32::{Foundation::HWND, System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop},
    core::{Result as WinResult, factory},
};

#[derive(Clone)]
pub struct Source {
    pub(crate) item: GraphicsCaptureItem,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub(crate) target: Option<CaptureTarget>,
}

impl Source {
    pub(crate) fn from_item(item: GraphicsCaptureItem) -> WinResult<Self> {
        let size = item.Size()?;
        Ok(Self {
            name: item.DisplayName()?.to_string(),
            width: size.Width.max(0) as u32,
            height: size.Height.max(0) as u32,
            item,
            target: None,
        })
    }
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

/// Diagnostic capture is restricted to the app's own window.
pub fn own_window_source(hwnd: usize) -> WinResult<Source> {
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    Source::from_item(unsafe { interop.CreateForWindow(HWND(hwnd as *mut _))? })
}

pub(crate) struct Capture {
    pub(crate) pool: Direct3D11CaptureFramePool,
    pub(crate) session: GraphicsCaptureSession,
    pub(crate) item: GraphicsCaptureItem,
    pub(crate) frame_token: Option<i64>,
    pub(crate) closed_token: Option<i64>,
}

/// Explicitly return WGC pool buffers on replacement, resize, error and stop.
pub(crate) struct CapturedFrame(pub Direct3D11CaptureFrame);
impl std::ops::Deref for CapturedFrame {
    type Target = Direct3D11CaptureFrame;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for CapturedFrame {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
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
