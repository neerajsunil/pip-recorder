//! Media Foundation Sink Writer for hardware/software H.264.
use std::{os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Win32::{Media::MediaFoundation::*, System::Com::*},
    core::{Interface, PCWSTR, Result as WinResult},
};

pub(crate) fn attributes(count: u32) -> WinResult<IMFAttributes> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, count)?;
        Ok(attrs.unwrap())
    }
}

pub(crate) fn media_type(
    width: u32,
    height: u32,
    fps: u32,
    subtype: &windows::core::GUID,
) -> WinResult<IMFMediaType> {
    unsafe {
        let media = MFCreateMediaType()?;
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        if *subtype == MFVideoFormat_NV12 {
            media.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
            media.SetUINT32(&MF_MT_SAMPLE_SIZE, width * height * 3 / 2)?;
        }
        media.SetUINT64(
            &MF_MT_FRAME_SIZE,
            (u64::from(width) << 32) | u64::from(height),
        )?;
        media.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(fps) << 32) | 1)?;
        media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1_u64 << 32) | 1)?;
        media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        media.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        media.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        media.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        Ok(media)
    }
}

pub(crate) fn create_writer(
    path: &Path,
    input: &IMFMediaType,
    width: u32,
    height: u32,
    fps: u32,
    manager: Option<&IMFDXGIDeviceManager>,
    bitrate: u32,
) -> WinResult<(IMFSinkWriter, u32, String)> {
    unsafe {
        let attrs = attributes(4)?;
        attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
        attrs.SetUINT32(
            &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
            u32::from(manager.is_some()),
        )?;
        if let Some(manager) = manager {
            attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, manager)?;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let writer = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, &attrs)?;
        let output = media_type(width, height, fps, &MFVideoFormat_H264)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        let stream = writer.AddStream(&output)?;
        writer.SetInputMediaType(stream, input, None)?;
        let mut label = "Windows software H.264 encoder".to_owned();
        if manager.is_some() {
            // Verify the chosen encoder instead of labelling software as hardware.
            let extended: IMFSinkWriterEx = writer.cast()?;
            let mut transform = None;
            extended.GetTransformForStream(stream, 0, None, &mut transform)?;
            let attrs = transform.unwrap().GetAttributes()?;
            attrs.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute)?;
            let mut name = windows::core::PWSTR::null();
            let mut length = 0;
            if attrs
                .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut length)
                .is_ok()
            {
                label = name
                    .to_string()
                    .unwrap_or_else(|_| "Media Foundation hardware H.264".into());
                CoTaskMemFree(Some(name.0.cast()));
            } else {
                label = "Media Foundation hardware H.264".into();
            }
        }
        writer.BeginWriting()?;
        Ok((writer, stream, label))
    }
}
