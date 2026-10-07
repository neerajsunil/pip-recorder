//! One WASAPI capture or loopback endpoint, downmixed and resampled into the mixer.
use super::{AudioChannel, RATE, RING_FRAMES, devices::device_name, mixer::Mixer, qpc_time};
use windows::{
    Win32::{
        Media::{Audio::*, MediaFoundation::*},
        System::Com::*,
    },
    core::{HSTRING, Result as WinResult},
};

pub(super) struct Endpoint {
    pub(super) client: IAudioClient,
    pub(super) capture: IAudioCaptureClient,
    pub(super) rate: u32,
    pub(super) channels: usize,
    pub(super) bytes: usize,
    pub(super) float: bool,
    pub(super) weights: Vec<[f32; 2]>,
    pub(super) gain: f32,
    pub(super) name: String,
    pub(super) desktop: bool,
}
impl Endpoint {
    pub(super) fn open(id: Option<&str>, desktop: bool, volume: u32) -> Result<Self, String> {
        let create = || -> WinResult<Self> {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)?;
                let device = match id {
                    Some(id) => enumerator.GetDevice(&HSTRING::from(id))?,
                    None => enumerator.GetDefaultAudioEndpoint(
                        if desktop { eRender } else { eCapture },
                        if desktop { eConsole } else { eCommunications },
                    )?,
                };
                let name = device_name(&device)?;
                let client: IAudioClient = device.Activate(CLSCTX_INPROC_SERVER, None)?;
                let raw = client.GetMixFormat()?;
                let format = *raw;
                let mut mask = 0;
                let mut tag = format.wFormatTag;
                if tag == 0xfffe && format.cbSize >= 22 {
                    let extended = std::ptr::read_unaligned(raw.cast::<WAVEFORMATEXTENSIBLE>());
                    mask = extended.dwChannelMask;
                    let subformat = extended.SubFormat;
                    if subformat == MFAudioFormat_Float {
                        tag = 3;
                    } else if subformat == MFAudioFormat_PCM {
                        tag = 1;
                    }
                }
                let bits = format.wBitsPerSample;
                let channels = format.nChannels as usize;
                let valid = channels > 0
                    && channels <= 8
                    && format.nSamplesPerSec > 0
                    && ((tag == 3 && bits == 32) || (tag == 1 && matches!(bits, 8 | 16 | 24 | 32)))
                    && format.nBlockAlign as usize == channels * (bits as usize / 8);
                if !valid {
                    CoTaskMemFree(Some(raw.cast()));
                    return Err(windows::core::Error::new(
                        windows::core::HRESULT(0x80070057u32 as i32),
                        "Unsupported audio endpoint format",
                    ));
                }
                let initialized = client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    if desktop {
                        AUDCLNT_STREAMFLAGS_LOOPBACK
                    } else {
                        0
                    },
                    1_000_000,
                    0,
                    raw,
                    None,
                );
                CoTaskMemFree(Some(raw.cast()));
                initialized?;
                let capture: IAudioCaptureClient = client.GetService()?;
                let mut weights = Vec::with_capacity(channels);
                if channels == 1 {
                    weights.push([1., 1.]);
                } else if channels == 2 {
                    weights.extend([[1., 0.], [0., 1.]]);
                } else {
                    if mask == 0 {
                        mask = (1u32 << channels) - 1;
                    }
                    for bit in 0..32 {
                        if mask & (1 << bit) == 0 {
                            continue;
                        }
                        weights.push(match bit {
                            0 | 6 => [1., 0.],
                            1 | 7 => [0., 1.],
                            3 => [0., 0.], // LFE is excluded from stereo downmix.
                            4 | 9 => [0.5, 0.],
                            5 | 10 => [0., 0.5],
                            _ => [std::f32::consts::FRAC_1_SQRT_2; 2],
                        });
                    }
                    weights.resize(channels, [0.5; 2]);
                }
                client.Start()?;
                Ok(Self {
                    client,
                    capture,
                    rate: format.nSamplesPerSec,
                    channels,
                    bytes: bits as usize / 8,
                    float: tag == 3,
                    weights,
                    gain: volume as f32 / 100.,
                    name,
                    desktop,
                })
            }
        };
        create().map_err(|e| endpoint_error(desktop, &e))
    }
    pub(super) fn sample(&self, bytes: &[u8], frame: usize) -> [f32; 2] {
        let mut output = [0.; 2];
        for channel in 0..self.channels {
            let start = (frame * self.channels + channel) * self.bytes;
            let data = &bytes[start..start + self.bytes];
            let value = if self.float {
                f32::from_le_bytes(data.try_into().unwrap())
            } else {
                match self.bytes {
                    1 => (data[0] as f32 - 128.) / 128.,
                    2 => i16::from_le_bytes(data.try_into().unwrap()) as f32 / 32768.,
                    3 => {
                        ((i32::from(data[0])
                            | (i32::from(data[1]) << 8)
                            | (i32::from(data[2]) << 16))
                            << 8) as f32
                            / 2147483648.
                    }
                    _ => i32::from_le_bytes(data.try_into().unwrap()) as f32 / 2147483648.,
                }
            };
            let value = if value.is_finite() {
                value * self.gain
            } else {
                0.
            };
            output[0] += value * self.weights[channel][0];
            output[1] += value * self.weights[channel][1];
        }
        output
    }
    pub(super) fn drain(
        &self,
        epoch: u64,
        ring: &mut Mixer,
        feedback: &AudioChannel,
    ) -> Result<(), String> {
        for _ in 0..64 {
            unsafe {
                if self
                    .capture
                    .GetNextPacketSize()
                    .map_err(|e| endpoint_error(self.desktop, &e))?
                    == 0
                {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                let mut timestamp = 0;
                self.capture
                    .GetBuffer(
                        &mut data,
                        &mut frames,
                        &mut flags,
                        None,
                        Some(&mut timestamp),
                    )
                    .map_err(|e| endpoint_error(self.desktop, &e))?;
                let result = (|| -> Result<(), String> {
                    if frames == 0 {
                        return Ok(());
                    }
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                        return Ok(());
                    }
                    if data.is_null() {
                        return Err("Audio device returned an empty packet.".into());
                    }
                    let bytes = std::slice::from_raw_parts(
                        data,
                        frames as usize * self.channels * self.bytes,
                    );
                    let peak = (0..frames as usize).fold(0.0f32, |peak, frame| {
                        self.sample(bytes, frame)
                            .into_iter()
                            .fold(peak, |peak, sample| peak.max(sample.abs()))
                    });
                    feedback.peak(self.desktop, peak);
                    if epoch == 0 {
                        return Ok(());
                    }
                    if flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0 || timestamp == 0 {
                        timestamp = qpc_time()
                            .map_err(|e| e.to_string())?
                            .saturating_sub(u64::from(frames) * 10_000_000 / u64::from(self.rate));
                    }
                    let start = (i128::from(timestamp) - i128::from(epoch)) as f64
                        * f64::from(RATE)
                        / 10_000_000.;
                    let end = start + f64::from(frames) * f64::from(RATE) / f64::from(self.rate);
                    if start > (ring.cursor + RING_FRAMES as u64) as f64 {
                        return Err(format!(
                            "{} returned an invalid audio timestamp.",
                            self.name
                        ));
                    }
                    let first = (start.ceil().max(0.) as u64).max(ring.cursor);
                    let last = (end.ceil().max(0.) as u64).min(ring.cursor + RING_FRAMES as u64);
                    for index in first..last {
                        let position = ((index as f64 - start) * f64::from(self.rate)
                            / f64::from(RATE))
                        .max(0.);
                        let a = (position as usize).min(frames as usize - 1);
                        let b = (a + 1).min(frames as usize - 1);
                        let fraction = (position - a as f64).clamp(0., 1.) as f32;
                        let a = self.sample(bytes, a);
                        let b = self.sample(bytes, b);
                        let slot = &mut ring.frames[index as usize % RING_FRAMES];
                        for channel in 0..2 {
                            slot[channel] += a[channel] + (b[channel] - a[channel]) * fraction;
                        }
                    }
                    Ok(())
                })();
                let released = self
                    .capture
                    .ReleaseBuffer(frames)
                    .map_err(|e| e.to_string());
                result.and(released)?;
            }
        }
        Ok(())
    }
}

pub(super) fn endpoint_error(desktop: bool, error: &windows::core::Error) -> String {
    let source = if desktop {
        "Desktop audio"
    } else {
        "Microphone"
    };
    match error.code().0 as u32 {
        0x80070005 if !desktop => "Microphone access is blocked. Enable microphone access for desktop apps in Windows Settings → Privacy & security → Microphone, then refresh audio devices.".into(),
        0x80070490 | 0x88890004 | 0x88890026 => format!("{source} device is unavailable or disconnected. Choose a connected device in Settings → Audio, then refresh."),
        0x8889000a => format!("{source} device is busy. Close apps using it exclusively, then refresh audio devices."),
        0x88890010 => "Windows Audio is not running. Start the Windows Audio service, then refresh audio devices.".into(),
        _ => format!("Could not use {source}: {error}. Check the selected device in Settings → Audio."),
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}
