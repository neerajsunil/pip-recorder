//! AV1 / AVC / HEVC MP4 muxing with optional stereo AAC audio.
//! AV1 sample entry follows https://aomediacodec.github.io/av1-isobmff/.
use fastrecorder_core::Codec;
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(crate) struct Av1Mp4 {
    file: File,
    mdat: u64,
    sizes: Vec<u32>,
    timestamps: Vec<u64>,
    keys: Vec<u32>,
    width: u32,
    height: u32,
    configuration: Vec<u8>,
    codec: Codec,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn atom(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() + 8);
    bytes.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(data);
    bytes
}
fn full(kind: &[u8; 4], version_flags: u32, data: &[u8]) -> Vec<u8> {
    atom(
        kind,
        &[version_flags.to_be_bytes().as_slice(), data].concat(),
    )
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}
fn matrix() -> Vec<u8> {
    words(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000])
}

type Obu<'a> = (u8, &'a [u8], &'a [u8]);
fn obus(data: &[u8]) -> io::Result<Vec<Obu<'_>>> {
    let mut offset = 0;
    let mut result = Vec::new();
    while offset < data.len() {
        let start = offset;
        let header = data[offset];
        offset += 1;
        if header & 0x81 != 0 || header & 2 == 0 {
            return Err(invalid("Expected AV1 low-overhead OBUs with size fields"));
        }
        if header & 4 != 0 {
            offset += 1;
        }
        let mut length = 0usize;
        let mut complete = false;
        for shift in (0..56).step_by(7) {
            let byte = *data
                .get(offset)
                .ok_or_else(|| invalid("Truncated OBU length"))?;
            offset += 1;
            length |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                complete = true;
                break;
            }
        }
        if !complete {
            return Err(invalid("Invalid OBU length"));
        }
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| invalid("Truncated OBU"))?;
        result.push(((header >> 3) & 15, &data[start..end], &data[offset..end]));
        offset = end;
    }
    Ok(result)
}
struct Bits<'a> {
    data: &'a [u8],
    offset: usize,
}
impl Bits<'_> {
    fn read(&mut self, count: usize) -> io::Result<u32> {
        let mut value = 0;
        for _ in 0..count {
            let byte = *self
                .data
                .get(self.offset / 8)
                .ok_or_else(|| invalid("Truncated sequence header"))?;
            value = (value << 1) | u32::from((byte >> (7 - self.offset % 8)) & 1);
            self.offset += 1;
        }
        Ok(value)
    }
    fn skip(&mut self, count: usize) -> io::Result<()> {
        for _ in 0..count {
            self.read(1)?;
        }
        Ok(())
    }
}
fn configuration(data: &[u8]) -> io::Result<Vec<u8>> {
    let (_, obu, payload) = obus(data)?
        .into_iter()
        .find(|(kind, _, _)| *kind == 1)
        .ok_or_else(|| invalid("NVENC returned no AV1 sequence header"))?;
    let mut bits = Bits {
        data: payload,
        offset: 0,
    };
    let profile = bits.read(3)?;
    if profile != 0 {
        return Err(invalid("Only AV1 Main profile is configured"));
    }
    bits.skip(1)?; // still_picture
    let reduced = bits.read(1)? != 0;
    let level = if reduced {
        bits.read(5)?
    } else {
        if bits.read(1)? != 0 {
            bits.skip(64)?; // timing_info
            if bits.read(1)? != 0 {
                let mut leading = 0;
                while bits.read(1)? == 0 {
                    leading += 1;
                    if leading > 31 {
                        return Err(invalid("Invalid timing UVLC"));
                    }
                }
                bits.skip(leading)?;
            }
            if bits.read(1)? != 0 {
                bits.skip(47)?;
            } // decoder_model_info
        }
        bits.skip(1)?; // initial_display_delay_present_flag
        bits.skip(5)?; // operating_points_cnt_minus_1
        bits.skip(12)?; // first operating_point_idc
        bits.read(5)?
    };
    let tier = if !reduced && level > 7 {
        bits.read(1)?
    } else {
        0
    };
    // Encoder is explicitly configured for 8-bit, non-monochrome, 4:2:0, unknown chroma position.
    let mut config = vec![
        0x81,
        ((profile << 5) | level) as u8,
        ((tier << 7) | 0x0c) as u8,
        0,
    ];
    config.extend_from_slice(obu);
    Ok(config)
}

impl Av1Mp4 {
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
            Codec::Av1 => configuration(header)?,
            _ => nal_configuration(codec, header)?,
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
            keys: Vec::new(),
            width,
            height,
            configuration,
            codec,
        })
    }
    pub fn write(&mut self, packet: &[u8], timestamp: u64, key: bool) -> io::Result<()> {
        if self
            .timestamps
            .last()
            .is_some_and(|last| timestamp <= *last)
        {
            return Err(invalid("Video output timestamps must increase"));
        }
        let mut sample = Vec::with_capacity(packet.len());
        if self.codec == Codec::Av1 {
            for (kind, obu, _) in obus(packet)? {
                if kind != 2 {
                    sample.extend_from_slice(obu);
                }
            }
        } else {
            for nal in nals(packet)? {
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
        self.timestamps.push(timestamp);
        if key || self.sizes.len() == 1 {
            self.keys.push(self.sizes.len() as u32);
        }
        Ok(())
    }
    pub fn finalize(
        &mut self,
        fps: u32,
        audio: Option<&crate::audio::AudioTrack>,
    ) -> io::Result<()> {
        let audio_offset = self.file.stream_position()?;
        if let Some(audio) = audio {
            copy_audio(&mut self.file, audio)?;
        }
        let end = self.file.stream_position()?;
        self.file.seek(SeekFrom::Start(self.mdat + 8))?;
        self.file.write_all(&(end - self.mdat).to_be_bytes())?;
        self.file.seek(SeekFrom::Start(end))?;
        let durations: Vec<u32> = self
            .timestamps
            .iter()
            .enumerate()
            .map(|(i, value)| {
                self.timestamps
                    .get(i + 1)
                    .map_or(1, |next| (next - value).min(u32::MAX as u64) as u32)
            })
            .collect();
        let duration: u64 = durations.iter().map(|v| u64::from(*v)).sum();
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
        mdhd.extend(duration.to_be_bytes());
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
        let stbl = atom(
            b"stbl",
            &[stsd, full(b"stts", 0, &stts), stsc, stsz, co64, stss].concat(),
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
        let trak = atom(b"trak", &[full(b"tkhd", 0x01000003, &tkhd), mdia].concat());
        let mut movie = [full(b"mvhd", 0x01000000, &mvhd), trak].concat();
        if let Some(audio) = audio {
            movie.extend(audio_trak(audio, audio_offset, 2, duration, fps));
        }
        self.file.write_all(&atom(b"moov", &movie))?;
        self.file.sync_all()
    }
}

fn copy_audio(file: &mut File, audio: &crate::audio::AudioTrack) -> io::Result<()> {
    let mut input = File::open(&audio.path)?;
    let copied = io::copy(&mut input, file)?;
    let expected: u64 = audio.sizes.iter().map(|size| u64::from(*size)).sum();
    if copied != expected {
        return Err(invalid("Truncated AAC audio spool"));
    }
    Ok(())
}
fn descriptor(tag: u8, payload: &[u8]) -> Vec<u8> {
    let size = payload.len() as u32;
    let mut length = Vec::new();
    for shift in [21, 14, 7, 0] {
        let value = ((size >> shift) & 127) as u8;
        if !length.is_empty() || value != 0 || shift == 0 {
            length.push(value | if shift != 0 { 128 } else { 0 });
        }
    }
    [vec![tag], length, payload.to_vec()].concat()
}
fn audio_trak(
    audio: &crate::audio::AudioTrack,
    offset: u64,
    id: u32,
    movie_duration: u64,
    movie_scale: u32,
) -> Vec<u8> {
    let rate = crate::audio::RATE;
    let media_duration = audio.sizes.len() as u64 * 1024;
    let playable = audio
        .frames
        .min(media_duration.saturating_sub(audio.priming));
    let duration = movie_duration.min(playable * u64::from(movie_scale) / u64::from(rate));
    let mut tkhd = vec![0; 16];
    tkhd.extend(words(&[id, 0]));
    tkhd.extend(duration.to_be_bytes());
    tkhd.extend([0; 12]);
    tkhd.extend([1, 0, 0, 0]); // Audio volume 1.0.
    tkhd.extend(matrix());
    tkhd.extend([0; 8]);
    let mut mdhd = vec![0; 16];
    mdhd.extend(words(&[rate]));
    mdhd.extend(media_duration.to_be_bytes());
    mdhd.extend([0x55, 0xc4, 0, 0]);
    let hdlr = full(
        b"hdlr",
        0,
        &[&0u32.to_be_bytes()[..], b"soun", &[0; 12], b"Audio\0"].concat(),
    );
    let decoder = descriptor(
        4,
        &[
            vec![0x40, 0x15, 0, 0, 0],
            words(&[192_000, 192_000]),
            descriptor(5, &crate::audio::ASC),
        ]
        .concat(),
    );
    let esds = full(
        b"esds",
        0,
        &descriptor(
            3,
            &[
                (id as u16).to_be_bytes().to_vec(),
                vec![0],
                decoder,
                descriptor(6, &[2]),
            ]
            .concat(),
        ),
    );
    let mut entry = vec![0; 28];
    entry[6..8].copy_from_slice(&1u16.to_be_bytes());
    entry[16..18].copy_from_slice(&2u16.to_be_bytes());
    entry[18..20].copy_from_slice(&16u16.to_be_bytes());
    entry[24..28].copy_from_slice(&(rate << 16).to_be_bytes());
    entry.extend(esds);
    let stsd = full(b"stsd", 0, &[words(&[1]), atom(b"mp4a", &entry)].concat());
    let stts = full(b"stts", 0, &words(&[1, audio.sizes.len() as u32, 1024]));
    let stsc = full(b"stsc", 0, &words(&[1, 1, audio.sizes.len() as u32, 1]));
    let stsz = full(
        b"stsz",
        0,
        &[words(&[0, audio.sizes.len() as u32]), words(&audio.sizes)].concat(),
    );
    let co64 = full(
        b"co64",
        0,
        &[words(&[1]), offset.to_be_bytes().to_vec()].concat(),
    );
    let stbl = atom(b"stbl", &[stsd, stts, stsc, stsz, co64].concat());
    let dinf = atom(
        b"dinf",
        &full(b"dref", 0, &[words(&[1]), full(b"url ", 1, &[])].concat()),
    );
    let minf = atom(b"minf", &[full(b"smhd", 0, &[0; 4]), dinf, stbl].concat());
    let mdia = atom(
        b"mdia",
        &[full(b"mdhd", 0x01000000, &mdhd), hdlr, minf].concat(),
    );
    let edit = atom(
        b"edts",
        &full(
            b"elst",
            0x01000000,
            &[
                words(&[1]),
                duration.to_be_bytes().to_vec(),
                audio.priming.to_be_bytes().to_vec(),
                vec![0, 1, 0, 0],
            ]
            .concat(),
        ),
    );
    atom(
        b"trak",
        &[full(b"tkhd", 0x01000003, &tkhd), edit, mdia].concat(),
    )
}

/// Add AAC to a finalized Media Foundation MP4 without re-encoding video or
/// relocating its chunks. The original moov becomes a free box only after the
/// replacement metadata and appended audio have been flushed successfully.
pub(crate) fn attach_audio(path: &Path, audio: &crate::audio::AudioTrack) -> io::Result<()> {
    let mut file = File::options().read(true).write(true).open(path)?;
    let end = file.metadata()?.len();
    let mut offset = 0;
    let mut original = None;
    let mut open_ended = None;
    while offset + 8 <= end {
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0; 8];
        file.read_exact(&mut header)?;
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let (size, header_size) = if size == 1 {
            let mut extended = [0; 8];
            file.read_exact(&mut extended)?;
            (u64::from_be_bytes(extended), 16)
        } else if size == 0 {
            if end - offset > u64::from(u32::MAX) {
                return Err(invalid(
                    "Cannot append audio to an open-ended MP4 box larger than 4 GB",
                ));
            }
            open_ended = Some((offset, (end - offset) as u32));
            (end - offset, 8)
        } else {
            (u64::from(size), 8)
        };
        if size < header_size || size > end - offset {
            return Err(invalid("Invalid MP4 box size"));
        }
        if &header[4..] == b"moov" {
            if size - header_size > 64 * 1024 * 1024 {
                return Err(invalid("MP4 metadata exceeds the supported size"));
            }
            let mut movie = vec![0; (size - header_size) as usize];
            file.read_exact(&mut movie)?;
            original = Some((offset, movie));
        }
        offset += size;
    }
    let (original_offset, mut movie) =
        original.ok_or_else(|| invalid("Finalized video has no MP4 movie metadata"))?;
    let mut cursor = 0;
    let mut timing = None;
    let mut next_id = 2;
    while cursor + 8 <= movie.len() {
        let size = u32::from_be_bytes(movie[cursor..cursor + 4].try_into().unwrap()) as usize;
        if size < 8 || size > movie.len() - cursor {
            return Err(invalid("Invalid movie metadata"));
        }
        if &movie[cursor + 4..cursor + 8] == b"mvhd" {
            let body = &mut movie[cursor + 8..cursor + size];
            let (scale_offset, duration_size) = if body.first() == Some(&1) {
                (20, 8)
            } else {
                (12, 4)
            };
            if body.len() < scale_offset + 4 + duration_size + 4 {
                return Err(invalid("Truncated movie header"));
            }
            let scale =
                u32::from_be_bytes(body[scale_offset..scale_offset + 4].try_into().unwrap());
            let duration = if duration_size == 8 {
                u64::from_be_bytes(
                    body[scale_offset + 4..scale_offset + 12]
                        .try_into()
                        .unwrap(),
                )
            } else {
                u64::from(u32::from_be_bytes(
                    body[scale_offset + 4..scale_offset + 8].try_into().unwrap(),
                ))
            };
            if scale == 0 {
                return Err(invalid("Invalid movie timescale"));
            }
            let last = body.len() - 4;
            next_id = u32::from_be_bytes(body[last..].try_into().unwrap()).max(2);
            let following_id = next_id
                .checked_add(1)
                .ok_or_else(|| invalid("Invalid MP4 track ID"))?;
            body[last..].copy_from_slice(&following_id.to_be_bytes());
            timing = Some((scale, duration));
        }
        cursor += size;
    }
    let (scale, duration) = timing.ok_or_else(|| invalid("Missing movie timing"))?;
    file.seek(SeekFrom::End(0))?;
    let mdat = file.stream_position()?;
    file.write_all(&1u32.to_be_bytes())?;
    file.write_all(b"mdat")?;
    file.write_all(&0u64.to_be_bytes())?;
    copy_audio(&mut file, audio)?;
    let end = file.stream_position()?;
    file.seek(SeekFrom::Start(mdat + 8))?;
    file.write_all(&(end - mdat).to_be_bytes())?;
    file.seek(SeekFrom::Start(end))?;
    movie.extend(audio_trak(audio, mdat + 16, next_id, duration, scale));
    file.write_all(&atom(b"moov", &movie))?;
    file.sync_all()?;
    if let Some((offset, size)) = open_ended {
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&size.to_be_bytes())?;
    }
    file.seek(SeekFrom::Start(original_offset + 4))?;
    file.write_all(b"free")?;
    file.sync_all()
}

fn nals(bytes: &[u8]) -> io::Result<Vec<&[u8]>> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let length = if bytes[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            i += 1;
            continue;
        };
        starts.push((i, i + length));
        i += length;
    }
    if starts.is_empty() {
        return Err(invalid("Encoder returned no Annex B NAL units"));
    }
    Ok(starts
        .iter()
        .enumerate()
        .filter_map(|(i, (_, start))| {
            let mut end = starts.get(i + 1).map_or(bytes.len(), |v| v.0);
            while end > *start && bytes[end - 1] == 0 {
                end -= 1;
            }
            (end > *start).then_some(&bytes[*start..end])
        })
        .collect())
}
fn nal_configuration(codec: Codec, header: &[u8]) -> io::Result<Vec<u8>> {
    let nals = nals(header)?;
    let find = |kind: u8| -> io::Result<&[u8]> {
        nals.iter()
            .copied()
            .find(|v| {
                if codec == Codec::H264 {
                    v[0] & 31 == kind
                } else {
                    (v[0] >> 1) & 63 == kind
                }
            })
            .ok_or_else(|| invalid("Missing codec parameter set"))
    };
    let append = |out: &mut Vec<u8>, nal: &[u8]| -> io::Result<()> {
        let len = u16::try_from(nal.len()).map_err(|_| invalid("Parameter set too large"))?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
        Ok(())
    };
    if codec == Codec::H264 {
        let sps = find(7)?;
        let pps = find(8)?;
        if sps.len() < 4 {
            return Err(invalid("Truncated AVC SPS"));
        }
        let mut out = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
        append(&mut out, sps)?;
        out.push(1);
        append(&mut out, pps)?;
        if matches!(sps[1], 100 | 110 | 122 | 144) {
            out.extend_from_slice(&[0xfd, 0xf8, 0xf8, 0]);
        }
        return Ok(out);
    }
    let sps = find(33)?;
    // Remove emulation-prevention bytes before extracting profile_tier_level.
    let mut rbsp = Vec::new();
    let mut zeros = 0;
    for &b in &sps[2..] {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    if rbsp.len() < 13 {
        return Err(invalid("Truncated HEVC SPS"));
    }
    let layers = ((rbsp[0] >> 1) & 7) + 1;
    let nested = rbsp[0] & 1;
    let mut out = vec![1];
    out.extend_from_slice(&rbsp[1..13]);
    // Main, 8-bit 4:2:0, four-byte NAL lengths, unknown spatial segmentation.
    out.extend_from_slice(&[
        0xf0,
        0,
        0xfc,
        0xfd,
        0xf8,
        0xf8,
        0,
        0,
        (layers << 3) | (nested << 2) | 3,
        3,
    ]);
    for kind in [32, 33, 34] {
        out.push(0x80 | kind);
        out.extend_from_slice(&1u16.to_be_bytes());
        append(&mut out, find(kind)?)?;
    }
    Ok(out)
}
