//! Direct driver NVENC with synchronous GPU input and bounded resources.
#![allow(clippy::field_reassign_with_default)] // Zero reserved ABI fields before filling driver parameters.
use crate::{mp4::Av1Mp4, nvenc_api::bindings::*};
use fastrecorder_core::Codec;
use std::{
    ffi::{CStr, c_void},
    path::Path,
    ptr,
};
use windows::{
    Win32::Graphics::{Direct3D11::*, Dxgi::Common::*},
    core::Interface,
};

struct Session {
    api: NV_ENCODE_API_FUNCTION_LIST,
    handle: *mut c_void,
    _library: libloading::Library,
}

impl Session {
    fn open(device: &ID3D11Device) -> Result<Self, String> {
        unsafe {
            let library: libloading::Library =
                libloading::os::windows::Library::load_with_flags("nvEncodeAPI64.dll", 0x800)
                    .map_err(|e| format!("NVIDIA driver NVENC library unavailable: {e}"))?
                    .into();
            let create = library
                .get::<unsafe extern "system" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS>(
                    b"NvEncodeAPICreateInstance\0",
                )
                .map_err(|e| e.to_string())?;
            let mut api = NV_ENCODE_API_FUNCTION_LIST::default();
            api.version = NV_ENCODE_API_FUNCTION_LIST_VER;
            let status = create(&mut api);
            if status != NVENCSTATUS::NV_ENC_SUCCESS {
                return Err(format!("NVENC API initialization: {status:?}"));
            }
            let mut session = Self {
                api,
                handle: ptr::null_mut(),
                _library: library,
            };
            let mut open = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS::default();
            open.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
            open.deviceType = NV_ENC_DEVICE_TYPE::NV_ENC_DEVICE_TYPE_DIRECTX;
            open.device = device.as_raw();
            open.apiVersion = NVENCAPI_VERSION;
            let status = (session
                .api
                .nvEncOpenEncodeSessionEx
                .ok_or("Missing NVENC session API")?)(
                &mut open, &mut session.handle
            );
            session.check(status)?;
            Ok(session)
        }
    }
    fn check(&self, status: NVENCSTATUS) -> Result<(), String> {
        if status == NVENCSTATUS::NV_ENC_SUCCESS {
            return Ok(());
        }
        let detail = unsafe {
            self.api
                .nvEncGetLastErrorString
                .and_then(|get| {
                    if self.handle.is_null() {
                        return None;
                    }
                    let text = get(self.handle);
                    (!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned())
                })
                .unwrap_or_default()
        };
        Err(format!("NVENC {status:?}: {detail}"))
    }
    fn codec_supported(&self, codec: Codec) -> Result<(), String> {
        unsafe {
            let mut count = 0;
            self.check((self
                .api
                .nvEncGetEncodeGUIDCount
                .ok_or("Missing capability query")?)(
                self.handle, &mut count
            ))?;
            let mut guids = vec![GUID::default(); count as usize];
            let mut written = 0;
            self.check((self
                .api
                .nvEncGetEncodeGUIDs
                .ok_or("Missing codec query")?)(
                self.handle,
                guids.as_mut_ptr(),
                count,
                &mut written,
            ))?;
            if guids[..written as usize].contains(&codec_guid(codec)) {
                Ok(())
            } else {
                Err(format!(
                    "This GPU/driver does not expose {} encoding",
                    codec.name()
                ))
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.handle.is_null()
            && let Some(destroy) = self.api.nvEncDestroyEncoder
        {
            unsafe {
                let _ = destroy(self.handle);
            }
        }
    }
}
fn codec_guid(codec: Codec) -> GUID {
    match codec {
        Codec::Av1 => NV_ENC_CODEC_AV1_GUID,
        Codec::Hevc => NV_ENC_CODEC_HEVC_GUID,
        Codec::H264 => NV_ENC_CODEC_H264_GUID,
    }
}
pub fn probe_codec(device: &ID3D11Device, codec: Codec) -> Result<(), String> {
    Session::open(device)?.codec_supported(codec)
}
pub(crate) struct Nvenc {
    session: Session,
    input: ID3D11Texture2D,
    context: ID3D11DeviceContext,
    registered: NV_ENC_REGISTERED_PTR,
    bitstream: NV_ENC_OUTPUT_PTR,
    mux: Av1Mp4,
    width: u32,
    height: u32,
    fps: u32,
    pub label: String,
    pub codec: Codec,
    pub fallback: Option<String>,
}

impl Nvenc {
    #[allow(clippy::too_many_arguments)] // Explicit NVENC initialization parameters.
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        preset_index: u32,
        keyframe_seconds: u32,
        constant_bitrate: bool,
        quality_qp: Option<u32>,
        codec: Codec,
    ) -> Result<Self, String> {
        let session = Session::open(device)?;
        session.codec_supported(codec)?;
        unsafe {
            let preset_guid = match preset_index {
                1 => NV_ENC_PRESET_P1_GUID,
                2 => NV_ENC_PRESET_P2_GUID,
                3 => NV_ENC_PRESET_P3_GUID,
                5 => NV_ENC_PRESET_P5_GUID,
                6 => NV_ENC_PRESET_P6_GUID,
                7 => NV_ENC_PRESET_P7_GUID,
                _ => NV_ENC_PRESET_P4_GUID,
            };
            let mut preset = NV_ENC_PRESET_CONFIG::default();
            preset.version = NV_ENC_PRESET_CONFIG_VER;
            preset.presetCfg.version = NV_ENC_CONFIG_VER;
            session.check((session
                .api
                .nvEncGetEncodePresetConfigEx
                .ok_or("Missing preset API")?)(
                session.handle,
                codec_guid(codec),
                preset_guid,
                NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_HIGH_QUALITY,
                &mut preset,
            ))?;
            let mut config = preset.presetCfg;
            config.profileGUID = match codec {
                Codec::Av1 => NV_ENC_AV1_PROFILE_MAIN_GUID,
                Codec::Hevc => NV_ENC_HEVC_PROFILE_MAIN_GUID,
                Codec::H264 => NV_ENC_H264_PROFILE_HIGH_GUID,
            };
            config.gopLength = fps * keyframe_seconds;
            config.frameIntervalP = 1; // No B-frame delay or timestamp reordering.
            config.rcParams.rateControlMode = if constant_bitrate {
                NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_CBR
            } else {
                NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_VBR
            };
            config.rcParams.averageBitRate = bitrate;
            config.rcParams.maxBitRate = if constant_bitrate {
                bitrate
            } else {
                bitrate.saturating_mul(2)
            };
            config.rcParams.set_enableLookahead(0);
            if let Some(qp) = quality_qp {
                config.rcParams.rateControlMode = NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_CONSTQP;
                config.rcParams.constQP = NV_ENC_QP {
                    qpInterP: qp,
                    qpInterB: qp,
                    qpIntra: qp,
                };
                config.rcParams.averageBitRate = 0;
                config.rcParams.maxBitRate = 0;
            }
            // The current synchronous input/mux path does not support reordered
            // output. Keep P-only frames; quality tuning, spatial AQ and multipass
            // work without delaying or reordering frames.
            config.rcParams.set_zeroReorderDelay(0);
            config.rcParams.set_enableAQ(1);
            config.rcParams.set_aqStrength(0); // Driver selects the strength.
            config.rcParams.set_enableTemporalAQ(0);
            config.rcParams.multiPass = if quality_qp.is_some() {
                NV_ENC_MULTI_PASS::NV_ENC_MULTI_PASS_DISABLED
            } else {
                NV_ENC_MULTI_PASS::NV_ENC_TWO_PASS_QUARTER_RESOLUTION
            };
            match codec {
                Codec::Av1 => {
                    let av1 = &mut config.encodeCodecConfig.av1Config;
                    av1.level = NV_ENC_LEVEL::NV_ENC_LEVEL_AV1_AUTOSELECT.0 as u32;
                    av1.idrPeriod = fps * keyframe_seconds;
                    av1.set_repeatSeqHdr(1);
                    av1.set_chromaFormatIDC(1);
                    av1.set_pixelBitDepthMinus8(0);
                    av1.set_inputPixelBitDepthMinus8(0);
                    av1.set_outputAnnexBFormat(0);
                    av1.colorPrimaries =
                        NV_ENC_VUI_COLOR_PRIMARIES::NV_ENC_VUI_COLOR_PRIMARIES_BT709;
                    av1.transferCharacteristics =
                NV_ENC_VUI_TRANSFER_CHARACTERISTIC::NV_ENC_VUI_TRANSFER_CHARACTERISTIC_BT709;
                    av1.matrixCoefficients =
                        NV_ENC_VUI_MATRIX_COEFFS::NV_ENC_VUI_MATRIX_COEFFS_BT709;
                    av1.colorRange = 0;
                    av1.chromaSamplePosition = 0;
                }
                Codec::Hevc => {
                    let hevc = &mut config.encodeCodecConfig.hevcConfig;
                    hevc.idrPeriod = fps * keyframe_seconds;
                    hevc.set_repeatSPSPPS(1);
                    hevc.set_chromaFormatIDC(1);
                    hevc.set_pixelBitDepthMinus8(0);
                }
                Codec::H264 => {
                    let h264 = &mut config.encodeCodecConfig.h264Config;
                    h264.idrPeriod = fps * keyframe_seconds;
                    h264.set_repeatSPSPPS(1);
                    h264.chromaFormatIDC = 1;
                }
            }
            let mut init = NV_ENC_INITIALIZE_PARAMS::default();
            init.version = NV_ENC_INITIALIZE_PARAMS_VER;
            init.encodeGUID = codec_guid(codec);
            init.presetGUID = preset_guid;
            init.encodeWidth = width;
            init.encodeHeight = height;
            init.darWidth = width;
            init.darHeight = height;
            init.frameRateNum = fps;
            init.frameRateDen = 1;
            init.maxEncodeWidth = width;
            init.maxEncodeHeight = height;
            init.bufferFormat = NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_NV12;
            init.enablePTD = 1;
            init.encodeConfig = &mut config;
            init.tuningInfo = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_HIGH_QUALITY;
            let initialize = session
                .api
                .nvEncInitializeEncoder
                .ok_or("Missing encoder initialization")?;
            let status = initialize(session.handle, &mut init);
            let mut quality_fallback = None;
            if matches!(
                status,
                NVENCSTATUS::NV_ENC_ERR_UNSUPPORTED_PARAM | NVENCSTATUS::NV_ENC_ERR_INVALID_PARAM
            ) {
                let original = session.check(status).unwrap_err();
                // Preserve codec, preset, rate control and HQ tuning; optional
                // enhancements may be unavailable on an older GPU/driver.
                config.rcParams.set_enableAQ(0);
                config.rcParams.multiPass = NV_ENC_MULTI_PASS::NV_ENC_MULTI_PASS_DISABLED;
                init.encodeConfig = &mut config;
                session
                    .check(initialize(session.handle, &mut init))
                    .map_err(|error| format!("{original}; single-pass retry: {error}"))?;
                quality_fallback = Some("Driver rejected optional AQ / multipass settings; using HQ single-pass encoding without AQ.".to_string());
            } else {
                session.check(status)?;
            }
            let mut header = vec![0u8; 4096];
            let mut length = 0;
            let mut params = NV_ENC_SEQUENCE_PARAM_PAYLOAD::default();
            params.version = NV_ENC_SEQUENCE_PARAM_PAYLOAD_VER;
            params.inBufferSize = header.len() as u32;
            params.spsppsBuffer = header.as_mut_ptr().cast();
            params.outSPSPPSPayloadSize = &mut length;
            session.check((session
                .api
                .nvEncGetSequenceParams
                .ok_or("Missing sequence header API")?)(
                session.handle, &mut params
            ))?;
            header.truncate(length as usize);
            let mux = Av1Mp4::new_codec(path, width, height, codec, &header)
                .map_err(|e| e.to_string())?;
            let create_buffer = session
                .api
                .nvEncCreateBitstreamBuffer
                .ok_or("Missing output buffer API")?;
            let unregister = session
                .api
                .nvEncUnregisterResource
                .ok_or("Missing resource cleanup API")?;
            let input = crate::native::texture(
                device,
                width,
                height,
                DXGI_FORMAT_NV12,
                D3D11_USAGE_DEFAULT,
                (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
                0,
            )
            .map_err(|e| e.to_string())?;
            let mut resource = NV_ENC_REGISTER_RESOURCE::default();
            resource.version = NV_ENC_REGISTER_RESOURCE_VER;
            resource.resourceType = NV_ENC_INPUT_RESOURCE_TYPE::NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
            resource.resourceToRegister = input.as_raw();
            resource.width = width;
            resource.height = height;
            resource.bufferFormat = NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_NV12;
            resource.bufferUsage = NV_ENC_BUFFER_USAGE::NV_ENC_INPUT_IMAGE;
            session.check((session
                .api
                .nvEncRegisterResource
                .ok_or("Missing GPU resource API")?)(
                session.handle, &mut resource
            ))?;
            let mut output = NV_ENC_CREATE_BITSTREAM_BUFFER::default();
            output.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
            if let Err(error) = session.check(create_buffer(session.handle, &mut output)) {
                let _ = unregister(session.handle, resource.registeredResource);
                return Err(error);
            }
            Ok(Self {
                session,
                input,
                context: context.clone(),
                registered: resource.registeredResource,
                bitstream: output.bitstreamBuffer,
                mux,
                width,
                height,
                fps,
                codec,
                fallback: quality_fallback,
                label: format!(
                    "NVIDIA NVENC · {} · P{preset_index} HQ · {} · {}",
                    codec.name(),
                    if config.rcParams.enableAQ() != 0 {
                        "spatial AQ"
                    } else {
                        "AQ off"
                    },
                    quality_qp.map_or_else(
                        || format!(
                            "{} · {}",
                            if constant_bitrate { "CBR" } else { "VBR" },
                            if config.rcParams.multiPass
                                == NV_ENC_MULTI_PASS::NV_ENC_MULTI_PASS_DISABLED
                            {
                                "single pass"
                            } else {
                                "two-pass quarter"
                            }
                        ),
                        |qp| format!("CQP {qp} · single pass")
                    )
                ),
            })
        }
    }

    pub fn write(
        &mut self,
        texture: &ID3D11Texture2D,
        subresource: u32,
        index: u64,
    ) -> Result<(), String> {
        unsafe {
            self.context
                .CopySubresourceRegion(&self.input, 0, 0, 0, 0, texture, subresource, None);
            let mut mapped = NV_ENC_MAP_INPUT_RESOURCE::default();
            mapped.version = NV_ENC_MAP_INPUT_RESOURCE_VER;
            mapped.registeredResource = self.registered;
            let s = &self.session;
            s.check((s.api.nvEncMapInputResource.ok_or("Missing map API")?)(
                s.handle,
                &mut mapped,
            ))?;
            let result = (|| {
                let mut pic = NV_ENC_PIC_PARAMS::default();
                pic.version = NV_ENC_PIC_PARAMS_VER;
                pic.inputBuffer = mapped.mappedResource;
                pic.bufferFmt = NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_NV12;
                pic.inputWidth = self.width;
                pic.inputHeight = self.height;
                pic.outputBitstream = self.bitstream;
                pic.pictureStruct = NV_ENC_PIC_STRUCT::NV_ENC_PIC_STRUCT_FRAME;
                pic.inputTimeStamp = index;
                pic.frameIdx = index as u32;
                pic.inputDuration = 1;
                s.check((s.api.nvEncEncodePicture.ok_or("Missing encode API")?)(
                    s.handle, &mut pic,
                ))?;
                let mut lock = NV_ENC_LOCK_BITSTREAM::default();
                lock.version = NV_ENC_LOCK_BITSTREAM_VER;
                lock.outputBitstream = self.bitstream;
                s.check((s.api.nvEncLockBitstream.ok_or("Missing output API")?)(
                    s.handle, &mut lock,
                ))?;
                let written = if lock.bitstreamBufferPtr.is_null() || lock.bitstreamSizeInBytes == 0
                {
                    Err("NVENC returned an empty output packet".to_string())
                } else {
                    let packet = std::slice::from_raw_parts(
                        lock.bitstreamBufferPtr.cast::<u8>(),
                        lock.bitstreamSizeInBytes as usize,
                    );
                    self.mux
                        .write(
                            packet,
                            lock.outputTimeStamp,
                            matches!(
                                lock.pictureType,
                                NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_IDR
                                    | NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_I
                            ),
                        )
                        .map_err(|e| e.to_string())
                };
                let unlock = s.check((s.api.nvEncUnlockBitstream.ok_or("Missing unlock API")?)(
                    s.handle,
                    self.bitstream,
                ));
                written.and(unlock)
            })();
            let unmap = s.check((s
                .api
                .nvEncUnmapInputResource
                .ok_or("Missing unmap API")?)(
                s.handle, mapped.mappedResource
            ));
            result.and(unmap)
        }
    }
    pub fn finalize(&mut self, audio: Option<&crate::audio::AudioTrack>) -> Result<(), String> {
        unsafe {
            let mut eos = NV_ENC_PIC_PARAMS::default();
            eos.version = NV_ENC_PIC_PARAMS_VER;
            eos.encodePicFlags = NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_EOS.0 as u32;
            self.session.check((self
                .session
                .api
                .nvEncEncodePicture
                .ok_or("Missing flush API")?)(
                self.session.handle, &mut eos
            ))?;
        }
        self.mux
            .finalize(self.fps, audio)
            .map_err(|e| e.to_string())
    }
}
impl Drop for Nvenc {
    fn drop(&mut self) {
        unsafe {
            if let Some(destroy) = self.session.api.nvEncDestroyBitstreamBuffer {
                let _ = destroy(self.session.handle, self.bitstream);
            }
            if let Some(unregister) = self.session.api.nvEncUnregisterResource {
                let _ = unregister(self.session.handle, self.registered);
            }
        }
    }
}
