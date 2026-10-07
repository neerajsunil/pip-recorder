//! Media Foundation AAC-LC encoder writing raw access units to a spool file.
use super::RATE;
use fastrecorder_mp4::AAC_AUDIO_SPECIFIC_CONFIG as ASC;
use std::{fs::File, io::Write, mem::ManuallyDrop, path::Path};
use windows::{
    Win32::{Media::MediaFoundation::*, System::Com::*},
    core::Result as WinResult,
};

fn aac_media_type(kbps: u32) -> WinResult<IMFMediaType> {
    unsafe {
        let media = MFCreateMediaType()?;
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        media.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        media.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
        media.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        media.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kbps * 125)?;
        media.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 1)?;
        media.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        media.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
        let mut user_data = vec![0, 0, 0x29, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        user_data.extend(ASC);
        media.SetBlob(&MF_MT_USER_DATA, &user_data)?;
        Ok(media)
    }
}
fn byte_sample(bytes: &[u8], time: i64, duration: i64) -> WinResult<IMFSample> {
    unsafe {
        let sample = MFCreateSample()?;
        let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
        let mut output = std::ptr::null_mut();
        buffer.Lock(&mut output, None, None)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
        buffer.Unlock()?;
        buffer.SetCurrentLength(bytes.len() as u32)?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(duration.max(1))?;
        Ok(sample)
    }
}
pub(super) struct AacEncoder {
    pub(super) transform: IMFTransform,
    pub(super) file: File,
    pub(super) sizes: Vec<u32>,
    pub(super) first_time: Option<i64>,
}
impl AacEncoder {
    pub(super) fn new(path: &Path, kbps: u32) -> WinResult<Self> {
        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&AACMFTEncoder, None, CLSCTX_INPROC_SERVER)?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
            input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
            input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * 4)?;
            transform.SetInputType(0, &input, 0)?;
            transform.SetOutputType(0, &aac_media_type(kbps)?, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            let file = File::options().create_new(true).write(true).open(path)?;
            Ok(Self {
                transform,
                file,
                sizes: Vec::new(),
                first_time: None,
            })
        }
    }
    pub(super) fn drain(&mut self) -> Result<(), String> {
        unsafe {
            loop {
                let info = self
                    .transform
                    .GetOutputStreamInfo(0)
                    .map_err(|e| e.to_string())?;
                let sample = if info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0 {
                    None
                } else {
                    let sample = MFCreateSample().map_err(|e| e.to_string())?;
                    sample
                        .AddBuffer(
                            &MFCreateMemoryBuffer(info.cbSize.max(65_536))
                                .map_err(|e| e.to_string())?,
                        )
                        .map_err(|e| e.to_string())?;
                    Some(sample)
                };
                let mut output = MFT_OUTPUT_DATA_BUFFER {
                    pSample: ManuallyDrop::new(sample),
                    ..Default::default()
                };
                let mut status = 0;
                let result =
                    self.transform
                        .ProcessOutput(0, std::slice::from_mut(&mut output), &mut status);
                let sample = ManuallyDrop::take(&mut output.pSample);
                ManuallyDrop::drop(&mut output.pEvents);
                match result {
                    Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(error) => return Err(format!("AAC encoding failed: {error}")),
                    Ok(()) => {}
                }
                let sample = sample.ok_or("AAC encoder returned no sample.")?;
                self.first_time
                    .get_or_insert(sample.GetSampleTime().map_err(|e| e.to_string())?);
                let buffer = sample
                    .ConvertToContiguousBuffer()
                    .map_err(|e| e.to_string())?;
                let mut data = std::ptr::null_mut();
                let mut length = 0;
                buffer
                    .Lock(&mut data, None, Some(&mut length))
                    .map_err(|e| e.to_string())?;
                let written = if data.is_null() || length == 0 {
                    Err("AAC encoder returned an empty packet.".into())
                } else {
                    self.file
                        .write_all(std::slice::from_raw_parts(data, length as usize))
                        .map_err(|e| e.to_string())
                };
                let unlocked = buffer.Unlock().map_err(|e| e.to_string());
                written.and(unlocked)?;
                self.sizes.push(length);
            }
        }
    }
    pub(super) fn encode(&mut self, pcm: &[u8], first: u64, count: u64) -> Result<(), String> {
        let time = (first * 10_000_000 / u64::from(RATE)) as i64;
        let end = ((first + count) * 10_000_000 / u64::from(RATE)) as i64;
        let sample = byte_sample(pcm, time, end - time).map_err(|e| e.to_string())?;
        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|e| format!("AAC input failed: {e}"))?;
        }
        self.drain()
    }
    pub(super) fn finish(&mut self) -> Result<(), String> {
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .map_err(|e| e.to_string())?;
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)
                .map_err(|e| e.to_string())?;
            self.drain()?;
            self.file.sync_all().map_err(|e| e.to_string())
        }
    }
}
