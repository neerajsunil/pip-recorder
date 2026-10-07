//! NVENC driver loading, session lifetime and capability queries.
use super::api::bindings::*;
use fastrecorder_core::Codec;
use std::{
    ffi::{CStr, c_void},
    ptr,
};
use windows::{Win32::Graphics::Direct3D11::*, core::Interface};

pub(super) struct Session {
    pub(super) api: NV_ENCODE_API_FUNCTION_LIST,
    pub(super) handle: *mut c_void,
    pub(super) _library: libloading::Library,
}

impl Session {
    pub(super) fn open(device: &ID3D11Device) -> Result<Self, String> {
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
    pub(super) fn check(&self, status: NVENCSTATUS) -> Result<(), String> {
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
    pub(super) fn check_operation(
        &self,
        status: NVENCSTATUS,
        operation: &str,
    ) -> Result<(), String> {
        self.check(status)
            .map_err(|error| format!("{operation}: {error}"))
    }
    pub(super) fn codec_supported(&self, codec: Codec) -> Result<(), String> {
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
    pub(super) fn capability(&self, codec: Codec, capability: NV_ENC_CAPS) -> Result<i32, String> {
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
pub(super) fn codec_guid(codec: Codec) -> GUID {
    match codec {
        Codec::Av1 => NV_ENC_CODEC_AV1_GUID,
        Codec::Hevc => NV_ENC_CODEC_HEVC_GUID,
        Codec::H264 => NV_ENC_CODEC_H264_GUID,
    }
}
pub fn probe_codec(device: &ID3D11Device, codec: Codec) -> Result<(), String> {
    Session::open(device)?.codec_supported(codec)
}
