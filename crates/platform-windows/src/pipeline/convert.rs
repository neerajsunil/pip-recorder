//! GPU colour conversion (scRGB → NV12) and optional CPU readback per frame.
use crate::gpu::{sample_texture, texture};
use std::mem::ManuallyDrop;
use windows::{
    Graphics::Capture::*,
    Win32::{
        Foundation::RECT,
        Graphics::{Direct3D11::*, Dxgi::Common::*},
        Media::MediaFoundation::*,
        System::WinRT::Direct3D11::*,
    },
    core::{Interface, Result as WinResult},
};

pub(super) struct Converter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    pub(super) input: ID3D11Texture2D,
    color: crate::capture::ColorConverter,
    input_view: ID3D11VideoProcessorInputView,
    staging: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
}
impl Converter {
    pub(super) fn new(
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
            let color = crate::capture::ColorConverter::new(device, context, iw, ih)?;
            let input = color.output.clone();
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
                color,
                input_view: input_view.unwrap(),
                staging: None,
                width: ow,
                height: oh,
            })
        }
    }
    pub(super) fn copy_frame(
        &self,
        frame: &Direct3D11CaptureFrame,
        color: crate::capture::DisplayColor,
    ) -> WinResult<bool> {
        let surface = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?;
        let texture: ID3D11Texture2D = unsafe { surface.GetInterface()? };
        self.color.copy(&texture, color)
    }
    pub(super) fn convert(&self, sample: &IMFSample) -> WinResult<()> {
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
    pub(super) fn cpu_sample(&mut self, sample: &IMFSample) -> WinResult<IMFSample> {
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
