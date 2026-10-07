use windows::Win32::{Graphics::Dxgi::Common::*, Media::MediaFoundation::*};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN},
            Direct3D11::*,
            Dxgi::*,
        },
    },
    core::{Interface, Result},
};

#[derive(Clone)]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    pub av1: bool,
    pub vendor: u32,
    pub codecs: [bool; 3],
    pub detail: String,
}

pub fn create_device(index: Option<u32>) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let adapter = match index {
        Some(index) => unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            Some(factory.EnumAdapters1(index)?.cast::<IDXGIAdapter>()?)
        },
        None => None,
    };
    device_on(adapter)
}

fn device_on(adapter: Option<IDXGIAdapter>) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    unsafe {
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            adapter.as_ref(),
            if adapter.is_some() {
                D3D_DRIVER_TYPE_UNKNOWN
            } else {
                D3D_DRIVER_TYPE_HARDWARE
            },
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device = device.unwrap();
        let context = context.unwrap();
        let _ = context
            .cast::<ID3D11Multithread>()?
            .SetMultithreadProtected(true);
        Ok((device, context))
    }
}

pub fn gpu_inventory() -> Result<Vec<GpuInfo>> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut gpus = Vec::new();
        let mut index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            let desc = adapter.GetDesc1()?;
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
                let name = String::from_utf16_lossy(&desc.Description)
                    .trim_end_matches('\0')
                    .to_owned();
                let mut codecs = [false; 3];
                let mut details = Vec::new();
                for (i, codec) in [
                    fastrecorder_core::Codec::Av1,
                    fastrecorder_core::Codec::Hevc,
                    fastrecorder_core::Codec::H264,
                ]
                .into_iter()
                .enumerate()
                {
                    let capability = create_device(Some(index))
                        .map_err(|e| e.to_string())
                        .and_then(|(device, _)| match desc.VendorId {
                            0x10de => crate::encode::nvenc::probe_codec(&device, codec),
                            0x8086 => crate::encode::intel::probe(&device, codec),
                            _ => Err("No direct encoder backend for this vendor".into()),
                        });
                    codecs[i] = capability.is_ok();
                    details.push(match capability {
                        Ok(()) => format!("{} available", codec.name()),
                        Err(error) => format!("{} unavailable: {error}", codec.name()),
                    });
                }
                let av1 = codecs[0];
                let detail = details.join("\n");
                gpus.push(GpuInfo {
                    index,
                    name,
                    av1,
                    vendor: desc.VendorId,
                    codecs,
                    detail,
                });
            }
            index += 1;
        }
        Ok(gpus)
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
) -> Result<ID3D11Texture2D> {
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
pub(crate) fn sample_texture(sample: &IMFSample) -> Result<(ID3D11Texture2D, u32)> {
    unsafe {
        let buffer: IMFDXGIBuffer = sample.GetBufferByIndex(0)?.cast()?;
        let subresource = buffer.GetSubresourceIndex()?;
        let mut raw = std::ptr::null_mut();
        buffer.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
        Ok((ID3D11Texture2D::from_raw(raw), subresource))
    }
}
