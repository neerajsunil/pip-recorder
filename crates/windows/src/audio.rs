//! WASAPI endpoints, a bounded timestamp-aligned stereo mixer, and AAC-LC.
//! Only compressed audio is spooled; video retains its GPU input path.
use fastrecorder_core::AudioConfig;
use std::{
    fs::File,
    io::Write,
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::{Audio::*, MediaFoundation::*},
        System::{
            Com::{StructuredStorage::*, *},
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
    },
    core::{HSTRING, Result as WinResult},
};

pub(crate) const RATE: u32 = 48_000;
const BLOCK: u64 = 1024;
const RING_FRAMES: usize = 96_000;
pub(crate) const ASC: [u8; 2] = [0x11, 0x90]; // AAC-LC, 48 kHz, stereo.

#[derive(Clone)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}
pub struct AudioInventory {
    pub desktop: Vec<AudioDevice>,
    pub microphones: Vec<AudioDevice>,
}

struct Apartment;
impl Apartment {
    fn new() -> WinResult<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
struct MediaFoundation;
impl MediaFoundation {
    fn new() -> WinResult<Self> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        }
        Ok(Self)
    }
}
impl Drop for MediaFoundation {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}
fn device_name(device: &IMMDevice) -> WinResult<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ)?;
        let mut value = store.GetValue(&PKEY_Device_FriendlyName)?;
        let text = PropVariantToStringAlloc(&value);
        let _ = PropVariantClear(&mut value);
        let text = text?;
        let result = text.to_string();
        CoTaskMemFree(Some(text.0.cast()));
        Ok(result?)
    }
}
pub fn audio_inventory() -> Result<AudioInventory, String> {
    let _apartment = Apartment::new().map_err(|e| e.to_string())?;
    let enumerate = || -> WinResult<_> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)?;
            let list = |flow| -> WinResult<Vec<AudioDevice>> {
                let devices = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
                let mut result = Vec::new();
                for index in 0..devices.GetCount()? {
                    let device = devices.Item(index)?;
                    let raw = device.GetId()?;
                    let id = raw.to_string();
                    CoTaskMemFree(Some(raw.0.cast()));
                    result.push(AudioDevice {
                        id: id?,
                        name: device_name(&device)?,
                    });
                }
                result.sort_by_key(|device| device.name.to_lowercase());
                Ok(result)
            };
            Ok(AudioInventory {
                desktop: list(eRender)?,
                microphones: list(eCapture)?,
            })
        }
    };
    enumerate().map_err(|e| format!("Could not list audio devices: {e}"))
}

pub(crate) fn qpc_time() -> WinResult<u64> {
    static FREQUENCY: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let frequency = if let Some(frequency) = FREQUENCY.get() {
        *frequency
    } else {
        let mut frequency = 0;
        unsafe {
            QueryPerformanceFrequency(&mut frequency)?;
        }
        let _ = FREQUENCY.set(frequency);
        frequency
    };
    let mut ticks = 0;
    unsafe {
        QueryPerformanceCounter(&mut ticks)?;
    }
    Ok((i128::from(ticks) * 10_000_000 / i128::from(frequency)) as u64)
}

struct Endpoint {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    rate: u32,
    channels: usize,
    bytes: usize,
    float: bool,
    weights: Vec<[f32; 2]>,
    gain: f32,
    name: String,
}
impl Endpoint {
    fn open(id: Option<&str>, desktop: bool, volume: u32) -> Result<Self, String> {
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
                })
            }
        };
        create().map_err(|e| {
            format!(
                "Could not open {} audio: {e}",
                if desktop { "desktop" } else { "microphone" }
            )
        })
    }
    fn sample(&self, bytes: &[u8], frame: usize) -> [f32; 2] {
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
    fn drain(&self, epoch: u64, ring: &mut Mixer) -> Result<(), String> {
        for _ in 0..64 {
            unsafe {
                if self
                    .capture
                    .GetNextPacketSize()
                    .map_err(|e| e.to_string())?
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
                    .map_err(|e| format!("{} disconnected or capture failed: {e}", self.name))?;
                let result = (|| -> Result<(), String> {
                    if epoch == 0 || frames == 0 {
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
impl Drop for Endpoint {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

struct Mixer {
    frames: Vec<[f32; 2]>,
    cursor: u64,
}
impl Mixer {
    fn new() -> Self {
        Self {
            frames: vec![[0.; 2]; RING_FRAMES],
            cursor: 0,
        }
    }
    fn take(&mut self, count: u64) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(count as usize * 4);
        for _ in 0..count {
            let slot = &mut self.frames[self.cursor as usize % RING_FRAMES];
            for value in *slot {
                pcm.extend_from_slice(
                    &((value.clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes(),
                );
            }
            *slot = [0.; 2];
            self.cursor += 1;
        }
        pcm
    }
}

pub(crate) fn aac_media_type() -> WinResult<IMFMediaType> {
    unsafe {
        let media = MFCreateMediaType()?;
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        media.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        media.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
        media.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        media.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?;
        media.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 1)?;
        media.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        media.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
        let mut user_data = vec![0, 0, 0x29, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        user_data.extend(ASC);
        media.SetBlob(&MF_MT_USER_DATA, &user_data)?;
        Ok(media)
    }
}
pub(crate) fn byte_sample(bytes: &[u8], time: i64, duration: i64) -> WinResult<IMFSample> {
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
struct AacEncoder {
    transform: IMFTransform,
    file: File,
    sizes: Vec<u32>,
    first_time: Option<i64>,
}
impl AacEncoder {
    fn new(path: &Path) -> WinResult<Self> {
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
            transform.SetOutputType(0, &aac_media_type()?, 0)?;
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
    fn drain(&mut self) -> Result<(), String> {
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
    fn encode(&mut self, pcm: &[u8], first: u64, count: u64) -> Result<(), String> {
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
    fn finish(&mut self) -> Result<(), String> {
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

pub(crate) struct AudioTrack {
    pub path: PathBuf,
    pub sizes: Vec<u32>,
    pub frames: u64,
    pub priming: u64,
}
impl Drop for AudioTrack {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
pub(crate) struct AudioCompletion {
    pub track: Option<AudioTrack>,
    pub error: Option<String>,
}
pub(crate) struct AudioRecording {
    epoch: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    end: Arc<AtomicU64>,
    failure: Arc<Mutex<Option<String>>>,
    worker: Option<JoinHandle<AudioCompletion>>,
    path: PathBuf,
    pub description: String,
}
impl AudioRecording {
    pub fn start(config: AudioConfig, path: PathBuf) -> Result<Self, String> {
        let epoch = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let end = Arc::new(AtomicU64::new(0));
        let failure = Arc::new(Mutex::new(None));
        let (worker_epoch, worker_stop, worker_end, worker_failure) =
            (epoch.clone(), stop.clone(), end.clone(), failure.clone());
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let spool_path = path.clone();
        let worker = thread::Builder::new()
            .name("fastrecorder-audio".into())
            .spawn(move || {
                let setup = (|| -> Result<_, String> {
                    let apartment = Apartment::new().map_err(|e| e.to_string())?;
                    let media = MediaFoundation::new().map_err(|e| e.to_string())?;
                    let mut endpoints = Vec::new();
                    if config.desktop {
                        endpoints.push(Endpoint::open(
                            config.desktop_device.as_deref(),
                            true,
                            config.desktop_volume,
                        )?);
                    }
                    if config.microphone {
                        endpoints.push(Endpoint::open(
                            config.microphone_device.as_deref(),
                            false,
                            config.microphone_volume,
                        )?);
                    }
                    let encoder = AacEncoder::new(&path)
                        .map_err(|e| format!("Could not initialize AAC audio: {e}"))?;
                    Ok((apartment, media, endpoints, encoder))
                })();
                let (apartment, media, endpoints, mut encoder) = match setup {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = sender.send(Err(error.clone()));
                        return AudioCompletion {
                            track: None,
                            error: Some(error),
                        };
                    }
                };
                let description = format!(
                    "{} → stereo AAC-LC · 48 kHz · 192 kbps",
                    endpoints
                        .iter()
                        .map(|endpoint| endpoint.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" + ")
                );
                if sender.send(Ok(description)).is_err() {
                    worker_stop.store(true, Ordering::Release);
                }
                let mut ring = Mixer::new();
                let result = (|| -> Result<(), String> {
                    while !worker_stop.load(Ordering::Acquire) {
                        let epoch = worker_epoch.load(Ordering::Acquire);
                        for endpoint in &endpoints {
                            endpoint.drain(epoch, &mut ring)?;
                        }
                        if epoch != 0 {
                            let due = qpc_time().map_err(|e| e.to_string())?.saturating_sub(epoch)
                                * u64::from(RATE)
                                / 10_000_000;
                            let ready = due.saturating_sub(4_800); // 100 ms margin for late endpoint packets.
                            while ring.cursor + BLOCK <= ready {
                                let first = ring.cursor;
                                encoder.encode(&ring.take(BLOCK), first, BLOCK)?;
                            }
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    let epoch = worker_epoch.load(Ordering::Acquire);
                    for endpoint in &endpoints {
                        endpoint.drain(epoch, &mut ring)?;
                    }
                    let end = worker_end.load(Ordering::Acquire);
                    while ring.cursor < end {
                        let first = ring.cursor;
                        let count = BLOCK.min(end - first);
                        encoder.encode(&ring.take(count), first, count)?;
                    }
                    Ok(())
                })();
                let mut error = result.err();
                if let Some(message) = &error {
                    *worker_failure.lock().unwrap() = Some(message.clone());
                }
                if let Err(message) = encoder.finish() {
                    error =
                        Some(error.map_or(message.clone(), |prior| format!("{prior}; {message}")));
                    *worker_failure.lock().unwrap() = error.clone();
                }
                let priming = encoder
                    .first_time
                    .filter(|time| *time < 0)
                    .map_or(0, |time| {
                        ((-i128::from(time)) * i128::from(RATE) / 10_000_000) as u64
                    });
                let sizes = std::mem::take(&mut encoder.sizes);
                drop(encoder);
                drop(endpoints);
                drop(media);
                drop(apartment);
                let track = if sizes.is_empty() {
                    if worker_end.load(Ordering::Acquire) > 0 && error.is_none() {
                        error = Some("AAC encoder produced no audio packets.".into());
                    }
                    let _ = std::fs::remove_file(&path);
                    None
                } else {
                    Some(AudioTrack {
                        path,
                        sizes,
                        frames: ring.cursor,
                        priming,
                    })
                };
                AudioCompletion { track, error }
            })
            .map_err(|e| e.to_string())?;
        match receiver.recv().map_err(|e| e.to_string())? {
            Ok(description) => Ok(Self {
                epoch,
                stop,
                end,
                failure,
                worker: Some(worker),
                path: spool_path,
                description,
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
    pub fn begin(&self) -> Result<(), String> {
        let time = qpc_time().map_err(|e| e.to_string())?;
        let _ = self
            .epoch
            .compare_exchange(0, time, Ordering::Release, Ordering::Relaxed);
        Ok(())
    }
    pub fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap().clone().or_else(|| {
            self.worker
                .as_ref()
                .filter(|worker| worker.is_finished())
                .map(|_| "Audio capture stopped unexpectedly.".into())
        })
    }
    pub fn finish(&mut self, frames: u64) -> AudioCompletion {
        self.end.store(frames, Ordering::Release);
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap_or_else(|_| {
            let _ = std::fs::remove_file(&self.path);
            AudioCompletion {
                track: None,
                error: Some("Audio capture thread failed.".into()),
            }
        })
    }
}
impl Drop for AudioRecording {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.finish(0);
        }
    }
}
