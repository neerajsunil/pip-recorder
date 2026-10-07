//! GPU-only scRGB → sRGB normalization/tone mapping shared by capture and preview.
use crate::{Source, gpu::texture};
use windows::{
    Win32::{
        Devices::Display::*,
        Foundation::HWND,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{Common::*, *},
            Gdi::*,
        },
    },
    core::{Error, HRESULT, Interface, Result},
};

#[derive(Clone, Copy)]
pub(crate) enum CaptureTarget {
    Monitor(usize),
    Window(usize),
}

#[derive(Clone, Copy)]
pub(crate) struct DisplayColor {
    pub white: f32,
    pub hdr: bool,
}
impl DisplayColor {
    pub fn for_source(source: &Source) -> Self {
        // A window spanning monitors uses the monitor containing its largest area.
        let monitor = unsafe {
            match source.target {
                Some(CaptureTarget::Monitor(handle)) => HMONITOR(handle as *mut _),
                Some(CaptureTarget::Window(handle)) => {
                    MonitorFromWindow(HWND(handle as *mut _), MONITOR_DEFAULTTONEAREST)
                }
                None => {
                    return Self {
                        white: 1.,
                        hdr: false,
                    };
                }
            }
        };
        unsafe {
            let find = || -> Result<Self> {
                let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
                let mut adapter_index = 0;
                while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
                    adapter_index += 1;
                    let mut output_index = 0;
                    while let Ok(output) = adapter.EnumOutputs(output_index) {
                        output_index += 1;
                        let desc = output.GetDesc()?;
                        if desc.Monitor != monitor {
                            continue;
                        }
                        let hdr = output.cast::<IDXGIOutput6>()?.GetDesc1()?.ColorSpace
                            == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
                        return Ok(Self {
                            white: if hdr {
                                sdr_white(&desc.DeviceName).unwrap_or(1.)
                            } else {
                                1.
                            },
                            hdr,
                        });
                    }
                }
                Ok(Self {
                    white: 1.,
                    hdr: false,
                })
            };
            find().unwrap_or(Self {
                white: 1.,
                hdr: false,
            })
        }
    }
}

fn sdr_white(device_name: &[u16; 32]) -> Option<f32> {
    unsafe {
        // Display topology can change between sizing and enumeration; retry once.
        for _ in 0..2 {
            let (mut path_count, mut mode_count) = (0, 0);
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
                .ok()
                .ok()?;
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            if QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
            .is_err()
            {
                continue;
            }
            for path in paths.into_iter().take(path_count as usize) {
                let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                        adapterId: path.sourceInfo.adapterId,
                        id: path.sourceInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut name.header) != 0
                    || &name.viewGdiDeviceName != device_name
                {
                    continue;
                }
                let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                        size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                        adapterId: path.targetInfo.adapterId,
                        id: path.targetInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut white.header) == 0 && white.SDRWhiteLevel > 0 {
                    return Some((white.SDRWhiteLevel as f32 / 1000.).max(1.));
                }
            }
        }
    }
    None
}

/// Turns a captured frame into 8-bit BGRA at `output`. SDR frames already are,
/// so they are copied; FP16 (HDR) frames go through the tone-mapping shader,
/// whose FP16 staging texture is only allocated when first needed.
pub(crate) struct ColorConverter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    input: std::cell::OnceCell<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    pub output: ID3D11Texture2D,
    output_view: ID3D11RenderTargetView,
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    constants: ID3D11Buffer,
    width: u32,
    height: u32,
}
impl ColorConverter {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let output = texture(
            device,
            width,
            height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_RENDER_TARGET.0 as u32,
            0,
        )?;
        let (mut output_view, mut vertex, mut pixel, mut constants) = (None, None, None, None);
        unsafe {
            device.CreateRenderTargetView(&output, None, Some(&mut output_view))?;
            device.CreateVertexShader(
                include_bytes!(concat!(env!("OUT_DIR"), "/color-vertex.cso")),
                None,
                Some(&mut vertex),
            )?;
            device.CreatePixelShader(
                include_bytes!(concat!(env!("OUT_DIR"), "/color-pixel.cso")),
                None,
                Some(&mut pixel),
            )?;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 16,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )?;
        }
        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            input: std::cell::OnceCell::new(),
            output,
            output_view: output_view.unwrap(),
            vertex: vertex.unwrap(),
            pixel: pixel.unwrap(),
            constants: constants.unwrap(),
            width,
            height,
        })
    }
    pub fn copy(&self, input: &ID3D11Texture2D, color: DisplayColor) -> Result<bool> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            input.GetDesc(&mut desc);
            if desc.Width < self.width || desc.Height < self.height {
                return Ok(false);
            }
            let region = D3D11_BOX {
                right: self.width,
                bottom: self.height,
                back: 1,
                ..Default::default()
            };
            if desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM {
                self.context.CopySubresourceRegion(
                    &self.output,
                    0,
                    0,
                    0,
                    0,
                    input,
                    0,
                    Some(&region),
                );
                self.context.Flush();
                return Ok(true);
            }
            if desc.Format != DXGI_FORMAT_R16G16B16A16_FLOAT {
                return Err(Error::new(
                    HRESULT(0x80004005u32 as i32),
                    "Unexpected capture color format. Choose the source again.",
                ));
            }
            if self.input.get().is_none() {
                let staging = texture(
                    &self.device,
                    self.width,
                    self.height,
                    DXGI_FORMAT_R16G16B16A16_FLOAT,
                    D3D11_USAGE_DEFAULT,
                    D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    0,
                )?;
                let mut view = None;
                self.device
                    .CreateShaderResourceView(&staging, None, Some(&mut view))?;
                let _ = self.input.set((staging, view.unwrap()));
            }
            let (staging, staging_view) = self.input.get().unwrap();
            self.context
                .CopySubresourceRegion(staging, 0, 0, 0, 0, input, 0, Some(&region));
            let constants = [color.white, if color.hdr { 1. } else { 0. }, 0., 0.];
            self.context.UpdateSubresource(
                &self.constants,
                0,
                None,
                constants.as_ptr().cast(),
                0,
                0,
            );
            self.context.IASetInputLayout(None);
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.context.VSSetShader(&self.vertex, None);
            self.context.PSSetShader(&self.pixel, None);
            self.context
                .PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            self.context
                .PSSetShaderResources(0, Some(&[Some(staging_view.clone())]));
            self.context
                .OMSetRenderTargets(Some(&[Some(self.output_view.clone())]), None);
            self.context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: self.width as f32,
                Height: self.height as f32,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            self.context.Draw(3, 0);
            // Remove bindings before the video processor reads the output.
            self.context.PSSetShaderResources(0, Some(&[None]));
            self.context.OMSetRenderTargets(None, None);
            // Submit reads before the caller returns the borrowed WGC buffer to
            // its pool. No CPU readback or wait for GPU completion is introduced.
            self.context.Flush();
            self.device.GetDeviceRemovedReason()?;
        }
        Ok(true)
    }
}
