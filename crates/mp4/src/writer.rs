//! Progressive MP4 writer for AV1 / AVC / HEVC with an optional AAC track.
use crate::{
    AacTrack,
    audio::{audio_trak, copy_audio},
    av1,
    boxes::{atom, full, invalid, matrix, words},
    nal,
};
use fastrecorder_core::Codec;
use std::{
    fs::File,
    io::{self, Seek, SeekFrom, Write},
    path::Path,
};

pub struct Mp4Writer {
    file: File,
    mdat: u64,
    sizes: Vec<u32>,
    timestamps: Vec<u64>,
    decode_timestamps: Vec<u64>,
    keys: Vec<u32>,
    width: u32,
    height: u32,
    configuration: Vec<u8>,
    codec: Codec,
}

impl Mp4Writer {
    pub fn sample_count(&self) -> usize {
        self.sizes.len()
    }
    pub fn new_codec(
        path: &Path,
        width: u32,
        height: u32,
        codec: Codec,
        header: &[u8],
    ) -> io::Result<Self> {
        if width > u16::MAX as u32 || height > u16::MAX as u32 {
            return Err(invalid("MP4 dimensions too large"));
        }
        let configuration = match codec {
            Codec::Av1 => av1::configuration(header)?,
            _ => nal::configuration(codec, header)?,
        };
        let mut file = File::create(path)?;
        file.write_all(&atom(
            b"ftyp",
            &[
                b"isom".as_slice(),
                &0u32.to_be_bytes(),
                match codec {
                    Codec::Av1 => b"isomiso6av01mp41",
                    Codec::Hevc => b"isomiso6hvc1mp41",
                    Codec::H264 => b"isomiso6avc1mp41",
                },
            ]
            .concat(),
        ))?;
        let mdat = file.stream_position()?;
        file.write_all(&1u32.to_be_bytes())?;
        file.write_all(b"mdat")?;
        file.write_all(&0u64.to_be_bytes())?;
        Ok(Self {
            file,
            mdat,
            sizes: Vec::new(),
            timestamps: Vec::new(),
            decode_timestamps: Vec::new(),
            keys: Vec::new(),
            width,
            height,
            configuration,
            codec,
        })
    }
    pub fn write(&mut self, packet: &[u8], timestamp: u64, key: bool) -> io::Result<()> {
        self.write_timed(packet, timestamp, timestamp, key)
    }
    /// Samples arrive in decode order; presentation timestamps can be reordered.
    pub fn write_timed(
        &mut self,
        packet: &[u8],
        presentation: u64,
        decode: u64,
        key: bool,
    ) -> io::Result<()> {
        if self
            .decode_timestamps
            .last()
            .is_some_and(|last| decode <= *last)
        {
            return Err(invalid("Video decode timestamps must increase"));
        }
        let mut sample = Vec::with_capacity(packet.len());
        if self.codec == Codec::Av1 {
            for (kind, obu, _) in av1::obus(packet)? {
                if kind != 2 {
                    sample.extend_from_slice(obu);
                }
            }
        } else {
            for nal in nal::nals(packet)? {
                let kind = if self.codec == Codec::H264 {
                    nal[0] & 31
                } else {
                    (nal[0] >> 1) & 63
                };
                // avc1 / hvc1 parameter sets live in the sample description.
                if (self.codec == Codec::H264 && matches!(kind, 7..=9))
                    || (self.codec == Codec::Hevc && matches!(kind, 32..=35))
                {
                    continue;
                }
                sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                sample.extend_from_slice(nal);
            }
        }
        if sample.is_empty() {
            return Err(invalid("Empty video sample"));
        }
        self.file.write_all(&sample)?;
        self.sizes.push(
            u32::try_from(sample.len()).map_err(|_| invalid("Video sample exceeds MP4 limit"))?,
        );
        self.timestamps.push(presentation);
        self.decode_timestamps.push(decode);
        if key || self.sizes.len() == 1 {
            self.keys.push(self.sizes.len() as u32);
        }
        Ok(())
    }
    pub fn finalize(&mut self, fps: u32, audio: Option<&AacTrack>) -> io::Result<()> {
        if self.timestamps.is_empty() || fps == 0 {
            return Err(invalid("Video requires samples and a nonzero time scale"));
        }
        // A captured frame remains visible until the next PRESENTED frame,
        // even when the capture clock skips ticks. Associate that duration with
        // its sample, then build the MP4 decode clock in packet order. Using
        // submission-order gaps as stts durations would make B-frames overlap
        // or leave holes on the display timeline after a skipped capture frame.
        let mut presentation_order: Vec<usize> = (0..self.timestamps.len()).collect();
        presentation_order.sort_unstable_by_key(|index| self.timestamps[*index]);
        let mut durations = vec![1u32; self.timestamps.len()];
        for pair in presentation_order.windows(2) {
            let delta = self.timestamps[pair[1]] - self.timestamps[pair[0]];
            if delta == 0 {
                return Err(invalid("Duplicate video presentation timestamp"));
            }
            durations[pair[0]] = u32::try_from(delta)
                .map_err(|_| invalid("Video sample duration exceeds MP4 limit"))?;
        }
        let mut decode_clock = 0u64;
        let mut decode_times = Vec::with_capacity(durations.len());
        for delta in &durations {
            decode_times.push(decode_clock);
            decode_clock = decode_clock
                .checked_add(u64::from(*delta))
                .ok_or_else(|| invalid("Video decode duration overflow"))?;
        }
        // Keep ctts unsigned for player compatibility. Shift presentation times
        // enough to represent reordered frames, then remove that artificial
        // delay with a video edit list. Audio retains its original shared clock.
        let shift = self
            .timestamps
            .iter()
            .zip(&decode_times)
            .map(|(presentation, decode)| decode.saturating_sub(*presentation))
            .max()
            .unwrap_or(0);
        let offsets: Vec<u32> = self
            .timestamps
            .iter()
            .zip(&decode_times)
            .map(|(presentation, decode)| {
                let offset = presentation
                    .checked_add(shift)
                    .and_then(|pts| pts.checked_sub(*decode))
                    .ok_or_else(|| invalid("Invalid video composition timestamp"))?;
                u32::try_from(offset)
                    .map_err(|_| invalid("Video composition offset exceeds MP4 limit"))
            })
            .collect::<io::Result<_>>()?;
        let duration = self
            .timestamps
            .iter()
            .max()
            .unwrap()
            .checked_add(1)
            .ok_or_else(|| invalid("Video duration overflow"))?;
        let media_duration = decode_clock.max(
            duration
                .checked_add(shift)
                .ok_or_else(|| invalid("Video duration overflow"))?,
        );
        let audio_offset = self.file.stream_position()?;
        if let Some(audio) = audio {
            copy_audio(&mut self.file, audio)?;
        }
        let end = self.file.stream_position()?;
        self.file.seek(SeekFrom::Start(self.mdat + 8))?;
        self.file.write_all(&(end - self.mdat).to_be_bytes())?;
        self.file.seek(SeekFrom::Start(end))?;
        let mut mvhd = vec![0; 16];
        mvhd.extend(words(&[fps]));
        mvhd.extend(duration.to_be_bytes());
        mvhd.extend(words(&[0x10000]));
        mvhd.extend([1, 0, 0, 0]);
        mvhd.extend([0; 8]);
        mvhd.extend(matrix());
        mvhd.extend([0; 24]);
        mvhd.extend(words(&[if audio.is_some() { 3 } else { 2 }]));
        let mut tkhd = vec![0; 16];
        tkhd.extend(words(&[1, 0]));
        tkhd.extend(duration.to_be_bytes());
        tkhd.extend([0; 16]);
        tkhd.extend(matrix());
        tkhd.extend(words(&[self.width << 16, self.height << 16]));
        let mut mdhd = vec![0; 16];
        mdhd.extend(words(&[fps]));
        mdhd.extend(media_duration.to_be_bytes());
        mdhd.extend([0x55, 0xc4, 0, 0]);
        let hdlr = full(
            b"hdlr",
            0,
            &[&0u32.to_be_bytes()[..], b"vide", &[0; 12], b"Video\0"].concat(),
        );
        let mut visual = vec![0; 78];
        visual[6..8].copy_from_slice(&1u16.to_be_bytes());
        visual[24..26].copy_from_slice(&(self.width as u16).to_be_bytes());
        visual[26..28].copy_from_slice(&(self.height as u16).to_be_bytes());
        visual[28..32].copy_from_slice(&0x00480000u32.to_be_bytes());
        visual[32..36].copy_from_slice(&0x00480000u32.to_be_bytes());
        visual[40..42].copy_from_slice(&1u16.to_be_bytes());
        visual[74..76].copy_from_slice(&24u16.to_be_bytes());
        visual[76..78].copy_from_slice(&0xffffu16.to_be_bytes());
        let (config_box, sample_entry) = match self.codec {
            Codec::Av1 => (b"av1C", b"av01"),
            Codec::Hevc => (b"hvcC", b"hvc1"),
            Codec::H264 => (b"avcC", b"avc1"),
        };
        visual.extend(atom(config_box, &self.configuration));
        visual.extend(atom(
            b"colr",
            &[b"nclx".as_slice(), &[0, 1, 0, 1, 0, 1, 0]].concat(),
        ));
        let stsd = full(
            b"stsd",
            0,
            &[words(&[1]), atom(sample_entry, &visual)].concat(),
        );
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for delta in durations {
            match runs.last_mut() {
                Some((count, old)) if *old == delta => *count += 1,
                _ => runs.push((1, delta)),
            }
        }
        let mut stts = words(&[runs.len() as u32]);
        for (count, delta) in runs {
            stts.extend(words(&[count, delta]));
        }
        let stsz = full(
            b"stsz",
            0,
            &[words(&[0, self.sizes.len() as u32]), words(&self.sizes)].concat(),
        );
        let stss = full(
            b"stss",
            0,
            &[words(&[self.keys.len() as u32]), words(&self.keys)].concat(),
        );
        let stsc = full(b"stsc", 0, &words(&[1, 1, self.sizes.len() as u32, 1]));
        let co64 = full(
            b"co64",
            0,
            &[words(&[1]), (self.mdat + 16).to_be_bytes().to_vec()].concat(),
        );
        let mut composition = Vec::new();
        if offsets.iter().any(|offset| *offset != 0) {
            let mut runs: Vec<(u32, u32)> = Vec::new();
            for offset in offsets {
                match runs.last_mut() {
                    Some((count, old)) if *old == offset => *count += 1,
                    _ => runs.push((1, offset)),
                }
            }
            let mut ctts = words(&[runs.len() as u32]);
            for (count, offset) in runs {
                ctts.extend(words(&[count, offset]));
            }
            composition = full(b"ctts", 0, &ctts);
        }
        let stbl = atom(
            b"stbl",
            &[
                stsd,
                full(b"stts", 0, &stts),
                composition,
                stsc,
                stsz,
                co64,
                stss,
            ]
            .concat(),
        );
        let dinf = atom(
            b"dinf",
            &full(b"dref", 0, &[words(&[1]), full(b"url ", 1, &[])].concat()),
        );
        let minf = atom(b"minf", &[full(b"vmhd", 1, &[0; 8]), dinf, stbl].concat());
        let mdia = atom(
            b"mdia",
            &[full(b"mdhd", 0x01000000, &mdhd), hdlr, minf].concat(),
        );
        let edit = if shift != 0 {
            let media_time =
                i64::try_from(shift).map_err(|_| invalid("Video edit time exceeds MP4 limit"))?;
            atom(
                b"edts",
                &full(
                    b"elst",
                    0x01000000,
                    &[
                        words(&[1]),
                        duration.to_be_bytes().to_vec(),
                        media_time.to_be_bytes().to_vec(),
                        words(&[0x10000]),
                    ]
                    .concat(),
                ),
            )
        } else {
            Vec::new()
        };
        let trak = atom(
            b"trak",
            &[full(b"tkhd", 0x01000003, &tkhd), edit, mdia].concat(),
        );
        let mut movie = [full(b"mvhd", 0x01000000, &mvhd), trak].concat();
        if let Some(audio) = audio {
            movie.extend(audio_trak(audio, audio_offset, 2, duration, fps));
        }
        self.file.write_all(&atom(b"moov", &movie))?;
        self.file.sync_all()
    }
}
