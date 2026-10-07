use crate::{intel::with_nv12, mp4::Av1Mp4};
use fastrecorder_core::{Codec, RecordingConfig};
use rav1e::prelude::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use windows::Win32::Media::MediaFoundation::IMFSample;
pub(crate) struct SoftwareAv1 {
    context: Context<u8>,
    mux: Option<Av1Mp4>,
    path: PathBuf,
    width: usize,
    height: usize,
    fps: u32,
    sent: u64,
    timestamps: BTreeMap<u64, u64>,
    u: Vec<u8>,
    v: Vec<u8>,
}
impl SoftwareAv1 {
    pub fn new(
        path: &Path,
        width: u32,
        height: u32,
        recording: &RecordingConfig,
    ) -> Result<Self, String> {
        let mut encoder = EncoderConfig::with_speed_preset(8);
        encoder.width = width as usize;
        encoder.height = height as usize;
        encoder.time_base = Rational {
            num: 1,
            den: u64::from(recording.fps),
        };
        encoder.bitrate = (recording.bitrate_for(Codec::Av1, width, height) * 1_000_000) as i32;
        // Keep CPU cost bounded for real-time software capture. Hardware encoders
        // use reordered frames; rav1e retains speed 8 and eight-frame RDO analysis.
        encoder.low_latency = true;
        encoder.max_key_frame_interval = u64::from(recording.fps * recording.keyframe_seconds);
        encoder.speed_settings.rdo_lookahead_frames = 8;
        encoder.pixel_range = PixelRange::Limited;
        encoder.color_description = Some(ColorDescription {
            color_primaries: ColorPrimaries::BT709,
            transfer_characteristics: TransferCharacteristics::BT709,
            matrix_coefficients: MatrixCoefficients::BT709,
        });
        let config = Config::new()
            .with_encoder_config(encoder)
            .with_threads(std::thread::available_parallelism().map_or(2, |n| n.get().min(8)));
        let context = config.new_context().map_err(|e| e.to_string())?;
        Ok(Self {
            context,
            mux: None,
            path: path.into(),
            width: width as usize,
            height: height as usize,
            fps: recording.fps,
            sent: 0,
            timestamps: BTreeMap::new(),
            u: vec![0; (width * height / 4) as usize],
            v: vec![0; (width * height / 4) as usize],
        })
    }
    fn drain(&mut self) -> Result<(), String> {
        loop {
            match self.context.receive_packet() {
                Ok(packet) => {
                    if self.mux.is_none() {
                        self.mux = Some(
                            Av1Mp4::new_codec(
                                &self.path,
                                self.width as u32,
                                self.height as u32,
                                Codec::Av1,
                                &packet.data,
                            )
                            .map_err(|e| e.to_string())?,
                        );
                    }
                    let timestamp = self
                        .timestamps
                        .remove(&packet.input_frameno)
                        .ok_or("rav1e returned an unknown frame number")?;
                    self.mux
                        .as_mut()
                        .unwrap()
                        .write(&packet.data, timestamp, packet.frame_type == FrameType::KEY)
                        .map_err(|e| e.to_string())?;
                }
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::NeedMoreData | EncoderStatus::LimitReached) => return Ok(()),
                Err(error) => return Err(format!("rav1e: {error:?}")),
            }
        }
    }
    pub fn write(&mut self, sample: &IMFSample, index: u64) -> Result<(), String> {
        let mut frame = self.context.new_frame();
        with_nv12(sample, |bytes| {
            let luma = self.width * self.height;
            if bytes.len() < luma * 3 / 2 {
                return Err("Truncated NV12 frame".into());
            }
            frame.planes[0].copy_from_raw_u8(&bytes[..luma], self.width, 1);
            for (i, pair) in bytes[luma..luma * 3 / 2]
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
            {
                self.u[i] = pair[0];
                self.v[i] = pair[1];
            }
            frame.planes[1].copy_from_raw_u8(&self.u, self.width / 2, 1);
            frame.planes[2].copy_from_raw_u8(&self.v, self.width / 2, 1);
            Ok(())
        })?;
        self.context
            .send_frame(frame)
            .map_err(|e| format!("rav1e input: {e:?}"))?;
        self.timestamps.insert(self.sent, index);
        self.sent += 1;
        self.drain()
    }
    pub fn finalize(&mut self, audio: Option<&crate::audio::AudioTrack>) -> Result<(), String> {
        self.context
            .send_frame(None)
            .map_err(|e| format!("rav1e flush: {e:?}"))?;
        self.drain()?;
        self.mux
            .as_mut()
            .ok_or("rav1e produced no video frames")?
            .finalize(self.fps, audio)
            .map_err(|e| e.to_string())
    }
}
