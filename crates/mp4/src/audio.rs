//! AAC-LC track description, interleaving and post-hoc attachment.
use crate::boxes::{atom, descriptor, full, invalid, matrix, words};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

/// Sample rate of every AAC track FastRecorder writes.
pub const AAC_SAMPLE_RATE: u32 = 48_000;
/// AudioSpecificConfig for AAC-LC, 48 kHz, stereo.
pub const AAC_AUDIO_SPECIFIC_CONFIG: [u8; 2] = [0x11, 0x90];

/// Raw AAC access units spooled to `path`, ready to be muxed.
/// The spool file is removed when the track is dropped.
pub struct AacTrack {
    pub path: PathBuf,
    pub sizes: Vec<u32>,
    pub frames: u64,
    pub priming: u64,
    /// Average bitrate in bits per second, recorded in the decoder configuration.
    pub bitrate: u32,
}
impl Drop for AacTrack {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn copy_audio(file: &mut File, audio: &AacTrack) -> io::Result<()> {
    let mut input = File::open(&audio.path)?;
    let copied = io::copy(&mut input, file)?;
    let expected: u64 = audio.sizes.iter().map(|size| u64::from(*size)).sum();
    if copied != expected {
        return Err(invalid("Truncated AAC audio spool"));
    }
    Ok(())
}
pub(crate) fn audio_trak(
    audio: &AacTrack,
    offset: u64,
    id: u32,
    movie_duration: u64,
    movie_scale: u32,
) -> Vec<u8> {
    let rate = AAC_SAMPLE_RATE;
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
            words(&[audio.bitrate, audio.bitrate]),
            descriptor(5, &AAC_AUDIO_SPECIFIC_CONFIG),
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
pub fn attach_audio(path: &Path, audio: &AacTrack) -> io::Result<()> {
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
