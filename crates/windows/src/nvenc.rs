//! Direct driver NVENC with synchronous GPU input and bounded resources.
#![allow(clippy::field_reassign_with_default)] // Zero reserved ABI fields before filling driver parameters.
use crate::{mp4::Av1Mp4, nvenc_api::bindings::*};
use fastrecorder_core::Codec;
use std::{
    collections::BTreeSet,
    ffi::{CStr, c_void},
    path::Path,
    ptr,
    rc::Rc,
    time::{Duration, Instant},
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
        let name = match status {
            NVENCSTATUS::NV_ENC_ERR_OUT_OF_MEMORY => "out of memory",
            NVENCSTATUS::NV_ENC_ERR_INVALID_PARAM => "invalid parameter",
            NVENCSTATUS::NV_ENC_ERR_INVALID_CALL => "invalid call sequence",
            NVENCSTATUS::NV_ENC_ERR_INVALID_DEVICE | NVENCSTATUS::NV_ENC_ERR_DEVICE_NOT_EXIST => {
                "device unavailable"
            }
            NVENCSTATUS::NV_ENC_ERR_INVALID_VERSION => "incompatible API structure version",
            NVENCSTATUS::NV_ENC_ERR_LOCK_BUSY | NVENCSTATUS::NV_ENC_ERR_ENCODER_BUSY => {
                "encoder busy"
            }
            NVENCSTATUS::NV_ENC_ERR_UNSUPPORTED_PARAM => "unsupported parameter",
            _ => "driver error",
        };
        Err(format!(
            "NVENC {name} (status {}){}",
            status.0,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ))
    }
    fn check_operation(&self, status: NVENCSTATUS, operation: &str) -> Result<(), String> {
        self.check(status)
            .map_err(|error| format!("{operation}: {error}"))
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
    fn capability(&self, codec: Codec, capability: NV_ENC_CAPS) -> Result<i32, String> {
        let mut params = NV_ENC_CAPS_PARAM::default();
        params.version = NV_ENC_CAPS_PARAM_VER;
        params.capsToQuery = capability;
        let mut value = 0;
        unsafe {
            self.check((self
                .api
                .nvEncGetEncodeCaps
                .ok_or("Missing NVENC capability API")?)(
                self.handle,
                codec_guid(codec),
                &mut params,
                &mut value,
            ))?;
        }
        Ok(value)
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
    session: Rc<Session>,
    slots: Vec<Slot>,
    context: ID3D11DeviceContext,
    submitted: u64,
    received: u64,
    presentations: BTreeSet<u64>,
    eos: bool,
    finalization_started: bool,
    mux: Av1Mp4,
    width: u32,
    height: u32,
    fps: u32,
    pub label: String,
    pub codec: Codec,
    pub fallback: Option<String>,
}

// Own each resource from allocation onward, including partial initialization.
// The shared session outlives all of its registered textures and output buffers.
struct Slot {
    session: Rc<Session>,
    input: ID3D11Texture2D,
    registered: NV_ENC_REGISTERED_PTR,
    bitstream: NV_ENC_OUTPUT_PTR,
    mapped: NV_ENC_INPUT_PTR,
    timestamp: u64,
}
impl Slot {
    fn new(
        session: Rc<Session>,
        device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let mut slot = Self {
            input: crate::native::texture(
                device,
                width,
                height,
                DXGI_FORMAT_NV12,
                D3D11_USAGE_DEFAULT,
                (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
                0,
            )
            .map_err(|e| e.to_string())?,
            session,
            registered: ptr::null_mut(),
            bitstream: ptr::null_mut(),
            mapped: ptr::null_mut(),
            timestamp: 0,
        };
        unsafe {
            let s = &slot.session;
            let mut resource = NV_ENC_REGISTER_RESOURCE::default();
            resource.version = NV_ENC_REGISTER_RESOURCE_VER;
            resource.resourceType = NV_ENC_INPUT_RESOURCE_TYPE::NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
            resource.resourceToRegister = slot.input.as_raw();
            resource.width = width;
            resource.height = height;
            resource.bufferFormat = NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_NV12;
            resource.bufferUsage = NV_ENC_BUFFER_USAGE::NV_ENC_INPUT_IMAGE;
            s.check((s
                .api
                .nvEncRegisterResource
                .ok_or("Missing GPU resource API")?)(
                s.handle, &mut resource
            ))?;
            slot.registered = resource.registeredResource;
            let mut output = NV_ENC_CREATE_BITSTREAM_BUFFER::default();
            output.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
            s.check((s
                .api
                .nvEncCreateBitstreamBuffer
                .ok_or("Missing output buffer API")?)(
                s.handle, &mut output
            ))?;
            slot.bitstream = output.bitstreamBuffer;
        }
        Ok(slot)
    }
    fn unmap(&mut self) -> Result<(), String> {
        if self.mapped.is_null() {
            return Ok(());
        }
        unsafe {
            self.session.check((self
                .session
                .api
                .nvEncUnmapInputResource
                .ok_or("Missing unmap API")?)(
                self.session.handle, self.mapped
            ))?;
        }
        self.mapped = ptr::null_mut();
        Ok(())
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        let _ = self.unmap();
        unsafe {
            if !self.bitstream.is_null()
                && let Some(destroy) = self.session.api.nvEncDestroyBitstreamBuffer
            {
                let _ = destroy(self.session.handle, self.bitstream);
            }
            if !self.registered.is_null()
                && let Some(unregister) = self.session.api.nvEncUnregisterResource
            {
                let _ = unregister(self.session.handle, self.registered);
            }
        }
    }
}

fn reference_mode(config: &mut NV_ENC_CONFIG, codec: Codec, mode: NV_ENC_BFRAME_REF_MODE) {
    match codec {
        Codec::Av1 => config.encodeCodecConfig.av1Config.useBFramesAsRef = mode,
        Codec::Hevc => config.encodeCodecConfig.hevcConfig.useBFramesAsRef = mode,
        Codec::H264 => config.encodeCodecConfig.h264Config.useBFramesAsRef = mode,
    }
}

fn buffer_count(frame_interval: i32, lookahead_depth: u16) -> usize {
    // Leave room for both the B-frame grouping and lookahead's output pipeline.
    // Match the conservative synchronous buffering used by OBS NVENC: four
    // surfaces per P interval, plus five pipeline slots when lookahead applies.
    let minimum = (frame_interval as usize * 4).max(4);
    if lookahead_depth == 0 {
        minimum
    } else {
        minimum.max(frame_interval as usize + lookahead_depth as usize + 5)
    }
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
        Self::new_with_limits(
            device,
            context,
            path,
            width,
            height,
            fps,
            bitrate,
            preset_index,
            keyframe_seconds,
            constant_bitrate,
            quality_qp,
            codec,
            16,
            2,
        )
    }
    #[allow(clippy::too_many_arguments)] // Limits apply only to bounded resource-allocation retries.
    fn new_with_limits(
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
        lookahead_limit: u16,
        b_frame_limit: i32,
    ) -> Result<Self, String> {
        let session = Rc::new(Session::open(device)?);
        session.codec_supported(codec)?;
        let mut adjustments = Vec::new();
        let mut capability = |cap| {
            session.capability(codec, cap).unwrap_or_else(|error| {
                adjustments.push(format!("Optional NVENC capability query failed: {error}"));
                0
            })
        };
        let supported_b_frames = capability(NV_ENC_CAPS::NV_ENC_CAPS_NUM_MAX_BFRAMES).clamp(0, 2);
        let lookahead_supported = capability(NV_ENC_CAPS::NV_ENC_CAPS_SUPPORT_LOOKAHEAD) > 0;
        let b_ref_capability = capability(NV_ENC_CAPS::NV_ENC_CAPS_SUPPORT_BFRAME_REF_MODE);
        // Bound our NV12 ring's estimated memory to 192 MiB (encoder-internal
        // memory is additional). Prefer 16 analysis frames, reduce to 8 at 4K.
        let surface_bytes = u64::from(width) * u64::from(height) * 3 / 2;
        let affordable_slots = (192 * 1024 * 1024 / surface_bytes.max(1)).min(32) as usize;
        if affordable_slots < 4 {
            return Err("NVENC input surfaces exceed the recording memory bound".into());
        }
        let b_frames = supported_b_frames
            .min(b_frame_limit)
            .min((affordable_slots / 4).saturating_sub(1) as i32);
        let lookahead = if !lookahead_supported || lookahead_limit == 0 {
            0
        } else {
            [16, 8]
                .into_iter()
                .find(|depth| {
                    *depth <= lookahead_limit
                        && buffer_count(b_frames + 1, *depth) <= affordable_slots
                })
                .unwrap_or(0)
        };
        if b_frames < 2 {
            adjustments.push(format!(
                "Using {b_frames} B-frames for {} within driver and buffer limits.",
                codec.name()
            ));
        }
        if lookahead == 0 {
            adjustments.push(if lookahead_supported {
                    "Lookahead disabled to limit the GPU buffer footprint or after an allocation fallback.".into()
            } else {
                "Driver does not expose lookahead for this codec.".into()
            });
        }
        let b_ref = if b_frames < 2 {
            NV_ENC_BFRAME_REF_MODE::NV_ENC_BFRAME_REF_MODE_DISABLED
        } else {
            match b_ref_capability {
                1 => NV_ENC_BFRAME_REF_MODE::NV_ENC_BFRAME_REF_MODE_EACH,
                2 => NV_ENC_BFRAME_REF_MODE::NV_ENC_BFRAME_REF_MODE_MIDDLE,
                _ => NV_ENC_BFRAME_REF_MODE::NV_ENC_BFRAME_REF_MODE_DISABLED,
            }
        };
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
            config.frameIntervalP = b_frames + 1;
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
            config
                .rcParams
                .set_enableLookahead(u32::from(lookahead != 0));
            config.rcParams.lookaheadDepth = lookahead;
            config.rcParams.set_disableIadapt(0);
            config.rcParams.set_disableBadapt(0);
            config.rcParams.vbvBufferSize = bitrate.saturating_mul(4);
            config.rcParams.vbvInitialDelay = bitrate.saturating_mul(4);
            if let Some(qp) = quality_qp {
                config.rcParams.rateControlMode = NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_CONSTQP;
                config.rcParams.constQP = NV_ENC_QP {
                    qpInterP: qp,
                    qpInterB: qp,
                    qpIntra: qp,
                };
                config.rcParams.averageBitRate = 0;
                config.rcParams.maxBitRate = 0;
                config.rcParams.vbvBufferSize = 0;
                config.rcParams.vbvInitialDelay = 0;
            }
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
            reference_mode(&mut config, codec, b_ref);
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
            let mut reference_active =
                b_ref != NV_ENC_BFRAME_REF_MODE::NV_ENC_BFRAME_REF_MODE_DISABLED;
            loop {
                init.encodeConfig = &mut config;
                let status = initialize(session.handle, &mut init);
                if status == NVENCSTATUS::NV_ENC_SUCCESS {
                    break;
                }
                if !matches!(
                    status,
                    NVENCSTATUS::NV_ENC_ERR_UNSUPPORTED_PARAM
                        | NVENCSTATUS::NV_ENC_ERR_INVALID_PARAM
                        | NVENCSTATUS::NV_ENC_ERR_OUT_OF_MEMORY
                ) {
                    session.check(status)?;
                }
                // Remove one enhancement at a time, retaining all others. A
                // successful initialization is the final check after cap queries.
                let adjustment = if status == NVENCSTATUS::NV_ENC_ERR_OUT_OF_MEMORY
                    && config.rcParams.lookaheadDepth != 0
                {
                    config.rcParams.lookaheadDepth = if config.rcParams.lookaheadDepth > 8 {
                        8
                    } else {
                        0
                    };
                    config
                        .rcParams
                        .set_enableLookahead(u32::from(config.rcParams.lookaheadDepth != 0));
                    "Driver could not allocate the enhanced encoder; retrying with less lookahead."
                } else if reference_active {
                    reference_mode(
                        &mut config,
                        codec,
                        NV_ENC_BFRAME_REF_MODE::NV_ENC_BFRAME_REF_MODE_DISABLED,
                    );
                    reference_active = false;
                    "Driver rejected the enhanced configuration; retrying without B-frame references."
                } else if config.rcParams.enableLookahead() != 0 {
                    config.rcParams.set_enableLookahead(0);
                    config.rcParams.lookaheadDepth = 0;
                    "Driver rejected the enhanced configuration; retrying without lookahead."
                } else if config.frameIntervalP > 1 {
                    config.frameIntervalP = 1;
                    "Driver rejected the enhanced configuration; retrying without B-frames."
                } else if config.rcParams.enableAQ() != 0
                    || config.rcParams.multiPass != NV_ENC_MULTI_PASS::NV_ENC_MULTI_PASS_DISABLED
                {
                    config.rcParams.set_enableAQ(0);
                    config.rcParams.multiPass = NV_ENC_MULTI_PASS::NV_ENC_MULTI_PASS_DISABLED;
                    "Driver rejected the enhanced configuration; retrying HQ without AQ / multipass."
                } else {
                    return Err(session.check(status).unwrap_err());
                };
                adjustments.push(adjustment.into());
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
            // Input textures stay mapped until their queued output retires.
            let count = buffer_count(config.frameIntervalP, config.rcParams.lookaheadDepth);
            let slots = (0..count)
                .map(|_| Slot::new(session.clone(), device, width, height))
                .collect::<Result<Vec<_>, _>>();
            let slots = match slots {
                Ok(slots) => slots,
                Err(error) => {
                    let depth = config.rcParams.lookaheadDepth;
                    if depth == 0 && config.frameIntervalP == 1 {
                        return Err(error);
                    }
                    // Release the failed session before retrying: encoder-internal
                    // allocations also consume VRAM. Never switch codec here.
                    drop(mux);
                    drop(session);
                    let next_depth = if depth > 8 { 8 } else { 0 };
                    let next_b_frames = if depth != 0 {
                        config.frameIntervalP - 1
                    } else {
                        0
                    };
                    let mut encoder = Self::new_with_limits(
                        device,
                        context,
                        path,
                        width,
                        height,
                        fps,
                        bitrate,
                        preset_index,
                        keyframe_seconds,
                        constant_bitrate,
                        quality_qp,
                        codec,
                        next_depth,
                        next_b_frames,
                    )
                    .map_err(|retry| format!("{error}; smaller NVENC buffer retry: {retry}"))?;
                    let previous = encoder.fallback.take().unwrap_or_default();
                    encoder.fallback = Some(format!(
                        "NVENC buffer allocation failed; reduced buffering while retaining codec / HQ / rate control. {error} {previous}"
                    ));
                    return Ok(encoder);
                }
            };
            Ok(Self {
                session,
                slots,
                context: context.clone(),
                submitted: 0,
                received: 0,
                presentations: BTreeSet::new(),
                eos: false,
                finalization_started: false,
                mux,
                width,
                height,
                fps,
                codec,
                fallback: (!adjustments.is_empty()).then(|| adjustments.join(" ")),
                label: format!(
                    "NVIDIA NVENC · {} · P{preset_index} HQ · {} · {} · {} B-frames · lookahead {} · B-reference {}",
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
                    ),
                    config.frameIntervalP - 1,
                    config.rcParams.lookaheadDepth,
                    if reference_active { "on" } else { "off" },
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
        if self.eos {
            return Err("Cannot submit NVENC frames after flushing".into());
        }
        if self.submitted - self.received >= self.slots.len() as u64 {
            return Err("NVENC exhausted its bounded frame queue".into());
        }
        unsafe {
            let position = (self.submitted % self.slots.len() as u64) as usize;
            let slot = &mut self.slots[position];
            if !slot.mapped.is_null() {
                return Err("NVENC tried to reuse an in-flight input".into());
            }
            self.context
                .CopySubresourceRegion(&slot.input, 0, 0, 0, 0, texture, subresource, None);
            let mut mapped = NV_ENC_MAP_INPUT_RESOURCE::default();
            mapped.version = NV_ENC_MAP_INPUT_RESOURCE_VER;
            mapped.registeredResource = slot.registered;
            let s = &self.session;
            s.check((s.api.nvEncMapInputResource.ok_or("Missing map API")?)(
                s.handle,
                &mut mapped,
            ))?;
            slot.mapped = mapped.mappedResource;
            slot.timestamp = index;
            let mut pic = NV_ENC_PIC_PARAMS::default();
            pic.version = NV_ENC_PIC_PARAMS_VER;
            pic.inputBuffer = slot.mapped;
            pic.bufferFmt = NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_NV12;
            pic.inputWidth = self.width;
            pic.inputHeight = self.height;
            pic.outputBitstream = slot.bitstream;
            pic.pictureStruct = NV_ENC_PIC_STRUCT::NV_ENC_PIC_STRUCT_FRAME;
            pic.inputTimeStamp = index;
            pic.frameIdx = self.submitted as u32;
            pic.inputDuration = 1;
            let status =
                (s.api.nvEncEncodePicture.ok_or("Missing encode API")?)(s.handle, &mut pic);
            if !matches!(
                status,
                NVENCSTATUS::NV_ENC_SUCCESS | NVENCSTATUS::NV_ENC_ERR_NEED_MORE_INPUT
            ) {
                let error = s.check(status);
                let _ = slot.unmap();
                return error;
            }
            self.submitted += 1;
            if !self.presentations.insert(index) {
                return Err("Duplicate NVENC input timestamp".into());
            }
            // NEED_MORE_INPUT means accepted/buffered, not an encoding failure.
            // It describes this submission, not readiness of the oldest output.
            // B-frame grouping can return it after earlier packets are ready;
            // retire the oldest queued buffer regardless of the newest status.
            if self.submitted - self.received == self.slots.len() as u64 {
                self.drain_one()?;
            }
            Ok(())
        }
    }
    fn drain_one(&mut self) -> Result<(), String> {
        let position = (self.received % self.slots.len() as u64) as usize;
        let slot = &mut self.slots[position];
        let s = &self.session;
        unsafe {
            let mut lock = NV_ENC_LOCK_BITSTREAM::default();
            lock.version = NV_ENC_LOCK_BITSTREAM_VER;
            lock.outputBitstream = slot.bitstream;
            // This session uses synchronous encoding, so every output read
            // must wait for completion. During recording the ring reserves
            // the pipeline delay; at Stop, EOS releases the pending tail first.
            // Nonblocking probes can expose an unfinished/reused buffer.
            lock.set_doNotWait(0);
            let lock_output = s.api.nvEncLockBitstream.ok_or("Missing output API")?;
            let unlock_output = s.api.nvEncUnlockBitstream.ok_or("Missing unlock API")?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let status = lock_output(s.handle, &mut lock);
                if status != NVENCSTATUS::NV_ENC_ERR_LOCK_BUSY {
                    s.check_operation(
                        status,
                        if self.eos {
                            "Reading a final encoded frame"
                        } else {
                            "Reading an encoded frame"
                        },
                    )?;
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "NVENC output timed out after five seconds ({} pending frames, {} buffers; {}).",
                        self.submitted - self.received,
                        self.slots.len(),
                        self.label,
                    ));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let written = if lock.bitstreamBufferPtr.is_null() || lock.bitstreamSizeInBytes == 0 {
                Err("NVENC returned an empty output packet".into())
            } else if !self.presentations.remove(&lock.outputTimeStamp) {
                Err(format!(
                    "NVENC returned an unknown or duplicate presentation timestamp: PTS {}, display frame {}, output {} of {}, input PTS {}, pending PTS {:?}..{:?}, EOS {}, codec {}",
                    lock.outputTimeStamp,
                    lock.frameIdxDisplay,
                    self.received,
                    self.submitted,
                    slot.timestamp,
                    self.presentations.first(),
                    self.presentations.last(),
                    self.eos,
                    self.codec.name(),
                ))
            } else {
                let packet = std::slice::from_raw_parts(
                    lock.bitstreamBufferPtr.cast::<u8>(),
                    lock.bitstreamSizeInBytes as usize,
                );
                self.mux
                    .write_timed(
                        packet,
                        lock.outputTimeStamp,
                        slot.timestamp,
                        lock.pictureType == NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_IDR,
                    )
                    .map_err(|e| e.to_string())
            };
            let unlocked = s.check_operation(
                unlock_output(s.handle, slot.bitstream),
                "Unlocking an encoded frame",
            );
            let unmapped = slot
                .unmap()
                .map_err(|error| format!("Releasing an encoded frame's input: {error}"));
            self.received += 1;
            written.and(unlocked).and(unmapped)
        }
    }
    fn flush(&mut self) -> Result<(), String> {
        if !self.eos {
            // Signal EOS before reading the remaining output, including short
            // recordings that never reached the ring's output-delay threshold.
            // Lookahead owns these buffers until the driver finishes them.
            unsafe {
                self.context.Flush();
                let mut eos = NV_ENC_PIC_PARAMS::default();
                eos.version = NV_ENC_PIC_PARAMS_VER;
                eos.encodePicFlags = NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_EOS.0 as u32;
                self.session.check_operation(
                    (self
                        .session
                        .api
                        .nvEncEncodePicture
                        .ok_or("Missing flush API")?)(
                        self.session.handle, &mut eos
                    ),
                    "Flushing the encoder at Stop",
                )?;
                self.eos = true;
            }
        }
        while self.received < self.submitted {
            self.drain_one()?;
        }
        if !self.presentations.is_empty() {
            return Err("NVENC did not flush every submitted frame".into());
        }
        Ok(())
    }
    pub fn finalize(
        &mut self,
        audio: Option<&crate::audio::AudioTrack>,
    ) -> Result<Option<String>, String> {
        self.finalization_started = true;
        let flush_error = self.flush().err();
        if self.mux.sample_count() == 0
            && let Some(error) = flush_error
        {
            return Err(error);
        }
        self.mux
            .finalize(self.fps, audio)
            .map_err(|error| match &flush_error {
                Some(flush) => format!("{flush}; completing the saved MP4: {error}"),
                None => format!("Completing the saved MP4: {error}"),
            })?;
        Ok(flush_error.map(|error| format!(
            "The encoder could not finish every buffered frame. Saved the completed portion; the ending may be shorter. {error}"
        )))
    }
}
impl Drop for Nvenc {
    fn drop(&mut self) {
        // Normal finalization already drained everything. On an early error,
        // signal EOS before releasing any resources still owned by the encoder.
        if !self.eos && !self.finalization_started {
            let _ = self.flush();
        }
    }
}
