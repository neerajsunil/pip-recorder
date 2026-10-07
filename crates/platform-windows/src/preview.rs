//! Live preview. With a preview surface (the normal case) the capture GPU scales
//! each frame straight into the studio's preview window; otherwise a small image
//! is read back for the UI to draw.
use crate::{
    Source,
    capture::{Capture, CapturedFrame, DisplayColor},
    gpu::{create_device, texture},
    preview_surface::{self, Presenter, Target},
};
use std::{
    mem::ManuallyDrop,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::*,
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, IDXGIDevice},
        },
        System::WinRT::{Direct3D11::*, *},
    },
    core::{IInspectable, Interface, Result},
};

/// Preview news for the UI. With a preview surface only state changes are sent
/// (`presented`); the CPU fallback carries pixels.
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    /// CPU pixels; empty when the frame went to the preview surface.
    pub rgba: Vec<u8>,
    /// The preview surface is showing live frames.
    pub presented: bool,
}
#[derive(Clone)]
pub struct PreviewChannel {
    latest: Arc<Mutex<Option<std::result::Result<PreviewFrame, String>>>>,
    pub(crate) enabled: Arc<AtomicBool>,
    cursor: Arc<AtomicBool>,
}
impl Default for PreviewChannel {
    fn default() -> Self {
        Self {
            latest: Arc::new(Mutex::new(None)),
            enabled: Arc::new(AtomicBool::new(true)),
            cursor: Arc::new(AtomicBool::new(true)),
        }
    }
}
impl PreviewChannel {
    pub fn take(&self) -> Option<std::result::Result<PreviewFrame, String>> {
        self.latest.lock().ok()?.take()
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }
    pub fn set_cursor(&self, enabled: bool) {
        self.cursor.store(enabled, Ordering::Release);
    }
    fn publish(&self, frame: std::result::Result<PreviewFrame, String>) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(frame);
        }
    }
}
pub struct Preview {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Preview {
    pub fn start(source: Source, channel: PreviewChannel) -> std::result::Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("fastrecorder-preview".into())
            .spawn(move || {
                let result = (|| -> std::result::Result<(), String> {
                    unsafe {
                        RoInitialize(RO_INIT_MULTITHREADED).map_err(|e| e.to_string())?;
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run(source, &channel, &worker_stop)
                    })).unwrap_or_else(|_| Err("Preview stopped after an internal error. Choose a source again or restart Pip.".into()));
                    unsafe {
                        RoUninitialize();
                    }
                    result
                })();
                if let Err(error) = result {
                    channel.publish(Err(error));
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
impl Drop for Preview {
    fn drop(&mut self) {
        self.stop();
    }
}

/// SDR displays are captured as 8-bit BGRA (half the memory of FP16 and no
/// conversion pass); HDR displays need FP16 for tone mapping.
pub(crate) fn capture_format(color: DisplayColor) -> DirectXPixelFormat {
    if color.hdr {
        DirectXPixelFormat::R16G16B16A16Float
    } else {
        DirectXPixelFormat::B8G8R8A8UIntNormalized
    }
}

fn run(
    source: Source,
    channel: &PreviewChannel,
    stop: &AtomicBool,
) -> std::result::Result<(), String> {
    let (device, context) = create_device(None).map_err(|e| e.to_string())?;
    let dxgi: IDXGIDevice = device.cast().map_err(|e| e.to_string())?;
    let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .and_then(|v| v.cast())
        .map_err(|e| e.to_string())?;
    let mut color = DisplayColor::for_source(&source);
    let mut format = capture_format(color);
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        format,
        2,
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
    let latest = Arc::new(Mutex::new(None));
    let slot = latest.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    capture.frame_token = Some(
        capture
            .pool
            .FrameArrived(
                &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                    move |pool, _| {
                        if let Ok(frame) = pool.ok()?.TryGetNextFrame() {
                            let frame = CapturedFrame(frame);
                            if let Ok(mut slot) = slot.lock() {
                                *slot = Some(frame);
                            }
                            let _ = tx.try_send(());
                        }
                        Ok(())
                    },
                ),
            )
            .map_err(|e| e.to_string())?,
    );
    let closed = Arc::new(AtomicBool::new(false));
    let closed_flag = closed.clone();
    capture.closed_token = Some(
        source
            .item
            .Closed(
                &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                    closed_flag.store(true, Ordering::Release);
                    Ok(())
                }),
            )
            .map_err(|e| e.to_string())?,
    );
    capture
        .session
        .SetIsCursorCaptureEnabled(channel.cursor.load(Ordering::Acquire))
        .map_err(|e| e.to_string())?;
    crate::capture::configure_capture_session(&capture.session)
        .map_err(|e| format!("Could not configure borderless preview: {e}"))?;
    // Ask Windows (11 24H2+) not to deliver preview frames faster than we show them.
    let _ = capture
        .session
        .SetMinUpdateInterval(windows::Foundation::TimeSpan {
            Duration: SURFACE_INTERVAL.as_nanos() as i64 / 100,
        });
    capture.session.StartCapture().map_err(|e| e.to_string())?;
    let mut renderer = PreviewRenderer::default();
    let mut converter = None;
    let mut color_check = Instant::now();
    let mut size = source.item.Size().map_err(|e| e.to_string())?;
    let mut due = Instant::now();
    let mut frame_deadline = Instant::now() + Duration::from_secs(8);
    let mut seen_frame = false;
    let mut capture_cursor = channel.cursor.load(Ordering::Acquire);
    while !stop.load(Ordering::Acquire) && !closed.load(Ordering::Acquire) {
        let _ = rx.recv_timeout(Duration::from_millis(20));
        let requested_cursor = channel.cursor.load(Ordering::Acquire);
        if requested_cursor != capture_cursor {
            capture
                .session
                .SetIsCursorCaptureEnabled(requested_cursor)
                .map_err(|e| e.to_string())?;
            capture_cursor = requested_cursor;
        }
        if !channel.enabled.load(Ordering::Acquire) {
            // Hidden panels/minimization should not consume the first-frame timeout.
            frame_deadline = Instant::now() + Duration::from_secs(8);
            continue;
        }
        if !seen_frame && Instant::now() > frame_deadline {
            return Err("No preview frames arrived. Refresh or choose another source.".into());
        }
        if Instant::now() < due {
            continue;
        }
        let frame = latest
            .lock()
            .map_err(|_| "Preview synchronization failed.".to_string())?
            .take(); // Release the slot before GPU work or pool recreation.
        if let Some(frame) = frame {
            let content = frame.ContentSize().map_err(|e| e.to_string())?;
            if content.Width <= 0 || content.Height <= 0 {
                continue;
            }
            if color_check.elapsed() >= Duration::from_secs(2) {
                color = DisplayColor::for_source(&source);
                color_check = Instant::now();
            }
            if content != size || capture_format(color) != format {
                drop(frame);
                format = capture_format(color);
                capture
                    .pool
                    .Recreate(&winrt, format, 2, content)
                    .map_err(|e| e.to_string())?;
                size = content;
                converter = None;
                continue;
            }
            let surface: IDirect3DDxgiInterfaceAccess = frame
                .Surface()
                .and_then(|v| v.cast())
                .map_err(|e| e.to_string())?;
            let input: ID3D11Texture2D =
                unsafe { surface.GetInterface() }.map_err(|e| e.to_string())?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                input.GetDesc(&mut desc);
            }
            if desc.Width < content.Width as u32 || desc.Height < content.Height as u32 {
                continue;
            }
            // SDR frames are already 8-bit BGRA: the video processor reads the
            // capture texture itself. HDR frames are tone-mapped first.
            let source_texture = if desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
                if converter.is_none() {
                    converter = Some(
                        crate::capture::ColorConverter::new(
                            &device,
                            &context,
                            content.Width as u32,
                            content.Height as u32,
                        )
                        .map_err(|e| format!("Preview color conversion failed: {e}"))?,
                    );
                }
                let converter = converter.as_ref().unwrap();
                if !converter
                    .copy(&input, color)
                    .map_err(|e| format!("Preview color conversion failed: {e}"))?
                {
                    continue;
                }
                converter.output.clone()
            } else {
                input.clone()
            };
            renderer.update(
                &device,
                &context,
                &source_texture,
                content.Width as u32,
                content.Height as u32,
                channel,
            );
            // Submit the read before the frame returns to the capture pool.
            unsafe { context.Flush() };
            seen_frame = true;
            due = Instant::now() + renderer.interval();
        }
    }
    if closed.load(Ordering::Acquire) {
        return Err("The preview source closed. Choose another source.".into());
    }
    Ok(())
}

/// CPU fallback: a small image read back to memory at about 12 fps.
const CPU_INTERVAL: Duration = Duration::from_millis(83);
const CPU_MAX: (u32, u32) = (960, 540);
/// Preview surface: 30 fps. Each frame still processes the full-resolution
/// capture, so a higher rate mostly costs GPU bandwidth and battery.
const SURFACE_INTERVAL: Duration = Duration::from_micros(33_333);

#[derive(Default)]
pub(crate) struct PreviewRenderer {
    renderer: Option<Renderer>,
    last: Option<Instant>,
    /// Creating the swap chain failed (for example another device still owns
    /// the window); retry after this time and use the CPU path meanwhile.
    surface_retry: Option<Instant>,
}
impl PreviewRenderer {
    pub fn interval(&self) -> Duration {
        if self
            .renderer
            .as_ref()
            .is_some_and(|r| r.presenter.is_some())
        {
            SURFACE_INTERVAL
        } else {
            CPU_INTERVAL
        }
    }
    pub fn update(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        input: &ID3D11Texture2D,
        width: u32,
        height: u32,
        channel: &PreviewChannel,
    ) {
        if !channel.enabled.load(Ordering::Acquire)
            || self
                .last
                .is_some_and(|time| time.elapsed() < self.interval())
        {
            return;
        }
        self.last = Some(Instant::now());
        let target = preview_surface::target().filter(|_| {
            self.surface_retry
                .is_none_or(|retry| Instant::now() >= retry)
        });
        let size = match target {
            Some(target) => (target.width, target.height),
            None => fit(width, height, CPU_MAX),
        };
        let rebuild = self.renderer.as_ref().is_none_or(|r| {
            r.iw != width
                || r.ih != height
                || (r.width, r.height) != size
                || r.presenter.as_ref().map(|p| p.target) != target
        });
        let frame = (|| -> Result<Option<PreviewFrame>> {
            if rebuild {
                // Release the old swap chain before another can bind the window.
                self.renderer = None;
                self.renderer = Some(
                    match Renderer::new(device, context, width, height, size, target) {
                        Ok(renderer) => renderer,
                        Err(_) if target.is_some() => {
                            self.surface_retry = Some(Instant::now() + Duration::from_secs(1));
                            let size = fit(width, height, CPU_MAX);
                            Renderer::new(device, context, width, height, size, None)?
                        }
                        Err(error) => return Err(error),
                    },
                );
            }
            let renderer = self.renderer.as_mut().unwrap();
            let frame = renderer.read(input)?;
            // The surface needs no per-frame message; announce only a fresh start.
            Ok((rebuild || !frame.presented).then_some(frame))
        })();
        match frame {
            Ok(Some(frame)) => channel.publish(Ok(frame)),
            Ok(None) => {}
            Err(error) => channel.publish(Err(format!("Preview unavailable: {error}"))),
        }
    }
}

/// Fits a source into `bounds` keeping its aspect ratio, never enlarging it.
fn fit(width: u32, height: u32, bounds: (u32, u32)) -> (u32, u32) {
    let scale = (f64::from(bounds.0) / f64::from(width))
        .min(f64::from(bounds.1) / f64::from(height))
        .min(1.0);
    (
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
    )
}

struct Renderer {
    context: ID3D11DeviceContext,
    video_context: ID3D11VideoContext,
    processor: ID3D11VideoProcessor,
    video_device: ID3D11VideoDevice,
    enumerator: ID3D11VideoProcessorEnumerator,
    /// Input views by texture: the capture pool's few textures, or the HDR
    /// converter's output. Reading them directly avoids a full-size copy.
    input_views: Vec<(usize, ID3D11VideoProcessorInputView)>,
    /// CPU path: output texture, its view, and a staging copy for readback.
    output: Option<(
        ID3D11Texture2D,
        ID3D11VideoProcessorOutputView,
        ID3D11Texture2D,
    )>,
    /// Surface path: the swap chain of the studio's preview window.
    presenter: Option<Presenter>,
    iw: u32,
    ih: u32,
    width: u32,
    height: u32,
}
impl Renderer {
    fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        iw: u32,
        ih: u32,
        (width, height): (u32, u32),
        target: Option<Target>,
    ) -> Result<Self> {
        unsafe {
            let video_device: ID3D11VideoDevice = device.cast()?;
            let video_context: ID3D11VideoContext = context.cast()?;
            let rate = DXGI_RATIONAL {
                Numerator: if target.is_some() { 30 } else { 12 },
                Denominator: 1,
            };
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: iw,
                InputHeight: ih,
                OutputFrameRate: rate,
                OutputWidth: width,
                OutputHeight: height,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumerator = video_device.CreateVideoProcessorEnumerator(&desc)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;
            let (output, presenter) = match target {
                Some(target) => (
                    None,
                    Some(Presenter::new(device, &video_device, &enumerator, target)?),
                ),
                None => {
                    let output = texture(
                        device,
                        width,
                        height,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        D3D11_USAGE_DEFAULT,
                        D3D11_BIND_RENDER_TARGET.0 as u32,
                        0,
                    )?;
                    let staging = texture(
                        device,
                        width,
                        height,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        D3D11_USAGE_STAGING,
                        0,
                        D3D11_CPU_ACCESS_READ.0 as u32,
                    )?;
                    let mut output_view = None;
                    video_device.CreateVideoProcessorOutputView(
                        &output,
                        &enumerator,
                        &D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                            ..Default::default()
                        },
                        Some(&mut output_view),
                    )?;
                    (Some((output, output_view.unwrap(), staging)), None)
                }
            };
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let source = RECT {
                left: 0,
                top: 0,
                right: iw as i32,
                bottom: ih as i32,
            };
            // Letterbox inside the surface; the CPU image is already fitted.
            let (dw, dh) = fit(iw, ih, (width, height));
            let left = (width - dw) as i32 / 2;
            let top = (height - dh) as i32 / 2;
            let destination = RECT {
                left,
                top,
                right: left + dw as i32,
                bottom: top + dh as i32,
            };
            let full = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&source));
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&destination));
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&full));
            if let Some(target) = target {
                let [r, g, b] = target.background;
                video_context.VideoProcessorSetOutputBackgroundColor(
                    &processor,
                    false,
                    &D3D11_VIDEO_COLOR {
                        Anonymous: D3D11_VIDEO_COLOR_0 {
                            RGBA: D3D11_VIDEO_COLOR_RGBA {
                                R: r,
                                G: g,
                                B: b,
                                A: 1.0,
                            },
                        },
                    },
                );
            }
            video_context.VideoProcessorSetStreamColorSpace(
                &processor,
                0,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE::default(),
            );
            video_context.VideoProcessorSetOutputColorSpace(
                &processor,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE::default(),
            );
            Ok(Self {
                context: context.clone(),
                video_context,
                processor,
                video_device,
                enumerator,
                input_views: Vec::new(),
                output,
                presenter,
                iw,
                ih,
                width,
                height,
            })
        }
    }
    fn input_view(&mut self, texture: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorInputView> {
        let key = texture.as_raw() as usize;
        if let Some((_, view)) = self.input_views.iter().find(|(k, _)| *k == key) {
            return Ok(view.clone());
        }
        let mut view = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                texture,
                &self.enumerator,
                &D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                },
                Some(&mut view),
            )?;
        }
        let view = view.unwrap();
        if self.input_views.len() >= 4 {
            self.input_views.remove(0);
        }
        self.input_views.push((key, view.clone()));
        Ok(view)
    }
    fn blit(
        &self,
        input: &ID3D11VideoProcessorInputView,
        output_view: &ID3D11VideoProcessorOutputView,
    ) -> Result<()> {
        unsafe {
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(input.clone())),
                ..Default::default()
            };
            let result = self.video_context.VideoProcessorBlt(
                &self.processor,
                output_view,
                0,
                std::slice::from_ref(&stream),
            );
            ManuallyDrop::drop(&mut stream.pInputSurface);
            result
        }
    }
    fn read(&mut self, texture: &ID3D11Texture2D) -> Result<PreviewFrame> {
        let input = self.input_view(texture)?;
        if let Some(presenter) = &self.presenter {
            self.blit(&input, &presenter.view)?;
            presenter.present()?;
            return Ok(PreviewFrame {
                width: self.width,
                height: self.height,
                source_width: self.iw,
                source_height: self.ih,
                rgba: Vec::new(),
                presented: true,
            });
        }
        let (output, output_view, staging) = self.output.as_ref().unwrap();
        self.blit(&input, output_view)?;
        unsafe {
            self.context.CopyResource(staging, output);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut rgba = vec![0; self.width as usize * self.height as usize * 4];
            for y in 0..self.height as usize {
                let row = std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    self.width as usize * 4,
                );
                for (input, output) in row.as_chunks::<4>().0.iter().zip(
                    rgba[y * self.width as usize * 4..(y + 1) * self.width as usize * 4]
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut(),
                ) {
                    output.copy_from_slice(&[input[2], input[1], input[0], 255]);
                }
            }
            self.context.Unmap(staging, 0);
            Ok(PreviewFrame {
                width: self.width,
                height: self.height,
                source_width: self.iw,
                source_height: self.ih,
                rgba,
                presented: false,
            })
        }
    }
}
