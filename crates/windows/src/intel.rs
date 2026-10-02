use crate::mp4::Av1Mp4;
use fastrecorder_core::{Codec, RecordingConfig};
use std::{
    collections::BTreeMap,
    ffi::{CStr, c_void},
    path::{Path, PathBuf},
};
use windows::{
    Win32::{Graphics::Direct3D11::ID3D11Device, Media::MediaFoundation::IMFSample},
    core::Interface,
};
unsafe extern "C" {
    fn fr_intel_open(
        device: *mut c_void,
        codec: u32,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        gop: u32,
        cbr: i32,
        probe: i32,
        error: *mut i8,
        capacity: u32,
    ) -> *mut c_void;
    fn fr_intel_encode(
        handle: *mut c_void,
        nv12: *const u8,
        width: u32,
        height: u32,
        index: u64,
        data: *mut *const u8,
        length: *mut u32,
        timestamp: *mut u64,
        key: *mut i32,
    ) -> i32;
    fn fr_intel_close(handle: *mut c_void);
}
fn codec_id(codec: Codec) -> u32 {
    match codec {
        Codec::Av1 => 0,
        Codec::Hevc => 1,
        Codec::H264 => 2,
    }
}
struct Session(*mut c_void);
impl Drop for Session {
    fn drop(&mut self) {
        unsafe { fr_intel_close(self.0) }
    }
}
#[allow(clippy::too_many_arguments)] // Keep the Intel ABI call visible in one place.
fn open(
    device: &ID3D11Device,
    codec: Codec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    gop: u32,
    cbr: bool,
    probe: bool,
) -> Result<Session, String> {
    let mut error = [0i8; 512];
    let ptr = unsafe {
        fr_intel_open(
            device.as_raw(),
            codec_id(codec),
            width,
            height,
            fps,
            bitrate,
            gop,
            cbr as i32,
            probe as i32,
            error.as_mut_ptr(),
            error.len() as u32,
        )
    };
    if ptr.is_null() {
        Err(unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned())
    } else {
        Ok(Session(ptr))
    }
}
pub(crate) fn probe(device: &ID3D11Device, codec: Codec) -> Result<(), String> {
    open(device, codec, 1920, 1080, 30, 8_000_000, 2, false, true).map(drop)
}
pub(crate) struct Intel {
    session: Session,
    mux: Option<Av1Mp4>,
    path: PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    pub codec: Codec,
    pub label: String,
    pub fallback: Option<String>,
    timestamps: BTreeMap<u64, u64>,
}
impl Intel {
    pub fn new(
        device: &ID3D11Device,
        path: &Path,
        width: u32,
        height: u32,
        config: &RecordingConfig,
        codec: Codec,
    ) -> Result<Self, String> {
        Ok(Self {
            session: open(
                device,
                codec,
                width,
                height,
                config.fps,
                config.bitrate_for(codec, width, height) * 1_000_000,
                config.keyframe_seconds,
                config.constant_bitrate,
                false,
            )?,
            mux: None,
            path: path.into(),
            width,
            height,
            fps: config.fps,
            codec,
            fallback: None,
            label: format!(
                "Intel Quick Sync · {} · oneVPL / Media SDK · system-memory NV12",
                codec.name()
            ),
            timestamps: BTreeMap::new(),
        })
    }
    fn packet(&mut self, input: *const u8, index: u64) -> Result<bool, String> {
        let mut bytes = std::ptr::null();
        let mut length = 0;
        let mut timestamp = 0;
        let mut key = 0;
        let status = unsafe {
            fr_intel_encode(
                self.session.0,
                input,
                self.width,
                self.height,
                index,
                &mut bytes,
                &mut length,
                &mut timestamp,
                &mut key,
            )
        };
        if status == 1 {
            return Ok(false);
        }
        if status != 0 {
            return Err(format!("Intel encoding failed ({status})"));
        }
        if bytes.is_null() || length == 0 {
            return Err("Intel returned an empty packet".into());
        }
        let packet = unsafe { std::slice::from_raw_parts(bytes, length as usize) };
        if self.mux.is_none() {
            self.mux = Some(
                Av1Mp4::new_codec(&self.path, self.width, self.height, self.codec, packet)
                    .map_err(|e| e.to_string())?,
            );
        }
        let index = self
            .timestamps
            .remove(&timestamp)
            .ok_or("Intel returned an unknown frame timestamp")?;
        self.mux
            .as_mut()
            .unwrap()
            .write(packet, index, key != 0)
            .map_err(|e| e.to_string())?;
        Ok(true)
    }
    pub fn write(&mut self, sample: &IMFSample, index: u64) -> Result<(), String> {
        self.timestamps
            .insert(index * 90000 / u64::from(self.fps), index);
        with_nv12(sample, |bytes| {
            if bytes.len() < self.width as usize * self.height as usize * 3 / 2 {
                return Err("Truncated Intel NV12 input frame".into());
            }
            self.packet(bytes.as_ptr(), index).map(|_| ())
        })
    }
    pub fn finalize(&mut self, audio: Option<&crate::audio::AudioTrack>) -> Result<(), String> {
        while self.packet(std::ptr::null(), 0)? {}
        self.mux
            .as_mut()
            .ok_or("Intel produced no video frames")?
            .finalize(self.fps, audio)
            .map_err(|e| e.to_string())
    }
}
pub(crate) fn with_nv12<R>(
    sample: &IMFSample,
    action: impl FnOnce(&[u8]) -> Result<R, String>,
) -> Result<R, String> {
    unsafe {
        let buffer = sample
            .ConvertToContiguousBuffer()
            .map_err(|e| e.to_string())?;
        let mut bytes = std::ptr::null_mut();
        let mut length = 0;
        buffer
            .Lock(&mut bytes, None, Some(&mut length))
            .map_err(|e| e.to_string())?;
        let result = action(std::slice::from_raw_parts(bytes, length as usize));
        let unlock = buffer.Unlock().map_err(|e| e.to_string());
        match result {
            Ok(value) => {
                unlock?;
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }
}
