use crate::{
    Source,
    capture::{Capture, CapturedFrame},
    gpu::{create_device, texture},
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

pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub rgba: Vec<u8>,
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
                    })).unwrap_or_else(|_| Err("Preview stopped after an internal error. Choose a source again or restart FastRecorder.".into()));
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
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        DirectXPixelFormat::R16G16B16A16Float,
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
    capture.session.StartCapture().map_err(|e| e.to_string())?;
    let mut renderer = PreviewRenderer::default();
    let mut converter = None;
    let mut color = crate::capture::DisplayColor::for_source(&source);
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
        if Instant::now() < due || !channel.enabled.load(Ordering::Acquire) {
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
            if content != size {
                drop(frame);
                capture
                    .pool
                    .Recreate(&winrt, DirectXPixelFormat::R16G16B16A16Float, 2, content)
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
            if color_check.elapsed() >= Duration::from_secs(2) {
                color = crate::capture::DisplayColor::for_source(&source);
                color_check = Instant::now();
            }
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
            renderer.update(
                &device,
                &context,
                &converter.output,
                content.Width as u32,
                content.Height as u32,
                channel,
            );
            seen_frame = true;
            due = Instant::now() + Duration::from_millis(83);
        }
    }
    if closed.load(Ordering::Acquire) {
        return Err("The preview source closed. Choose another source.".into());
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct PreviewRenderer {
    renderer: Option<Renderer>,
    last: Option<Instant>,
}
impl PreviewRenderer {
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
                .is_some_and(|time| time.elapsed() < Duration::from_millis(83))
        {
            return;
        }
        self.last = Some(Instant::now());
        let frame = (|| -> Result<PreviewFrame> {
            if self
                .renderer
                .as_ref()
                .is_none_or(|r| r.iw != width || r.ih != height)
            {
                self.renderer = Some(Renderer::new(device, context, width, height)?);
            }
            self.renderer.as_ref().unwrap().read(input)
        })()
        .map_err(|e| format!("Preview unavailable: {e}"));
        channel.publish(frame);
    }
}
struct Renderer {
    context: ID3D11DeviceContext,
    video_context: ID3D11VideoContext,
    processor: ID3D11VideoProcessor,
    input: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    output: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
    iw: u32,
    ih: u32,
    width: u32,
    height: u32,
}
impl Renderer {
    fn new(device: &ID3D11Device, context: &ID3D11DeviceContext, iw: u32, ih: u32) -> Result<Self> {
        let scale = (960.0 / iw as f64).min(540.0 / ih as f64).min(1.0);
        let width = (iw as f64 * scale).round().max(1.0) as u32;
        let height = (ih as f64 * scale).round().max(1.0) as u32;
        unsafe {
            let video_device: ID3D11VideoDevice = device.cast()?;
            let video_context: ID3D11VideoContext = context.cast()?;
            let rate = DXGI_RATIONAL {
                Numerator: 12,
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
            let input = texture(
                device,
                iw,
                ih,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_RENDER_TARGET.0 as u32,
                0,
            )?;
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
            let mut input_view = None;
            let mut output_view = None;
            video_device.CreateVideoProcessorInputView(
                &input,
                &enumerator,
                &D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                },
                Some(&mut input_view),
            )?;
            video_device.CreateVideoProcessorOutputView(
                &output,
                &enumerator,
                &D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                },
                Some(&mut output_view),
            )?;
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let source = RECT {
                left: 0,
                top: 0,
                right: iw as i32,
                bottom: ih as i32,
            };
            let destination = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&source));
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&destination));
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&destination));
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
                input,
                input_view: input_view.unwrap(),
                output,
                output_view: output_view.unwrap(),
                staging,
                iw,
                ih,
                width,
                height,
            })
        }
    }
    fn read(&self, texture: &ID3D11Texture2D) -> Result<PreviewFrame> {
        unsafe {
            let region = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: self.iw,
                bottom: self.ih,
                back: 1,
            };
            self.context
                .CopySubresourceRegion(&self.input, 0, 0, 0, 0, texture, 0, Some(&region));
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(self.input_view.clone())),
                ..Default::default()
            };
            let result = self.video_context.VideoProcessorBlt(
                &self.processor,
                &self.output_view,
                0,
                std::slice::from_ref(&stream),
            );
            ManuallyDrop::drop(&mut stream.pInputSurface);
            result?;
            self.context.CopyResource(&self.staging, &self.output);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
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
            self.context.Unmap(&self.staging, 0);
            Ok(PreviewFrame {
                width: self.width,
                height: self.height,
                source_width: self.iw,
                source_height: self.ih,
                rgba,
            })
        }
    }
}
