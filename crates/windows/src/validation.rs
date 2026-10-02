//! Development-only verification uses Windows' own H.264 decoder.
use std::{os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Win32::{Media::MediaFoundation::*, System::WinRT::*},
    core::{Interface, PCWSTR, Result},
};

pub struct Validation {
    pub frames: u64,
    pub seconds: f64,
    pub width: u32,
    pub height: u32,
    pub first_rgba: Vec<u8>,
}

pub fn validate_recording(path: &Path) -> Result<Validation> {
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
        let startup = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        if let Err(error) = startup {
            RoUninitialize();
            return Err(error);
        }
        let result = decode(path);
        let _ = MFShutdown();
        RoUninitialize();
        result
    }
}

fn decode(path: &Path) -> Result<Validation> {
    unsafe {
        let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attrs)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let output = MFCreateMediaType()?;
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &output)?;
        let media = reader.GetCurrentMediaType(stream)?;
        let size = media.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let width = (size >> 32) as u32;
        let height = size as u32;
        let mut report = Validation {
            frames: 0,
            seconds: 0.0,
            width,
            height,
            first_rgba: Vec::new(),
        };
        loop {
            let mut flags = 0;
            let mut time = 0;
            let mut sample = None;
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )?;
            if let Some(sample) = sample {
                report.frames += 1;
                report.seconds =
                    (time + sample.GetSampleDuration().unwrap_or(0)) as f64 / 10_000_000.0;
                if report.first_rgba.is_empty() {
                    let buffer = sample.GetBufferByIndex(0)?;
                    let mut data = std::ptr::null_mut();
                    if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
                        let mut pitch = 0;
                        buffer2d.Lock2D(&mut data, &mut pitch)?;
                        for y in 0..height as usize {
                            let row = std::slice::from_raw_parts(
                                data.offset(y as isize * pitch as isize),
                                width as usize * 4,
                            );
                            for pixel in row.as_chunks::<4>().0 {
                                report
                                    .first_rgba
                                    .extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
                            }
                        }
                        buffer2d.Unlock2D()?;
                    } else {
                        let mut length = 0;
                        buffer.Lock(&mut data, None, Some(&mut length))?;
                        // H.264 decoding can change the RGB allocation to padded
                        // macroblock dimensions on the first sample (flag 32).
                        let current = reader.GetCurrentMediaType(stream)?;
                        let current_size = current.GetUINT64(&MF_MT_FRAME_SIZE)?;
                        let allocation_height = current_size as u32;
                        let stride = current
                            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                            .unwrap_or(((current_size >> 32) as u32) * 4)
                            as i32;
                        let pitch = stride.unsigned_abs() as usize;
                        if length as usize >= pitch * allocation_height as usize {
                            let bytes = std::slice::from_raw_parts(data, length as usize);
                            for y in 0..height as usize {
                                let row = if stride < 0 {
                                    allocation_height as usize - 1 - y
                                } else {
                                    y
                                };
                                for pixel in bytes[row * pitch..row * pitch + width as usize * 4]
                                    .as_chunks::<4>()
                                    .0
                                {
                                    report
                                        .first_rgba
                                        .extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
                                }
                            }
                        }
                        buffer.Unlock()?;
                    }
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        Ok(report)
    }
}
