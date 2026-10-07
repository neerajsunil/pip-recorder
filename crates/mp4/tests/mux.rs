//! Structural tests for the MP4 writer. They parse the produced ISO BMFF box
//! tree directly, so they run on every OS without a decoder or GPU.
use fastrecorder_core::Codec;
use fastrecorder_mp4::{AacTrack, Mp4Writer, attach_audio};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

struct TempDir(PathBuf);
impl TempDir {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("fastrecorder-mp4-{name}-{stamp}"));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Top-level boxes of `data[range]` as (fourcc, payload range).
fn boxes(data: &[u8], range: Range<usize>) -> Vec<([u8; 4], Range<usize>)> {
    let mut result = Vec::new();
    let mut offset = range.start;
    while offset + 8 <= range.end {
        let size32 = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[offset + 4..offset + 8].try_into().unwrap();
        let (size, header) = match size32 {
            1 => (
                u64::from_be_bytes(data[offset + 8..offset + 16].try_into().unwrap()) as usize,
                16,
            ),
            0 => (range.end - offset, 8),
            size => (size, 8),
        };
        assert!(size >= header && offset + size <= range.end, "bad box size");
        result.push((kind, offset + header..offset + size));
        offset += size;
    }
    assert_eq!(offset, range.end, "trailing bytes after last box");
    result
}

/// Payload range of the first box at `path`, e.g. `["moov", "trak", "mdia"]`.
fn find(data: &[u8], path: &[&str]) -> Option<Range<usize>> {
    let mut range = 0..data.len();
    for (depth, name) in path.iter().enumerate() {
        let children = boxes(data, range.clone());
        let (kind, payload) = children
            .into_iter()
            .find(|(kind, _)| kind == name.as_bytes())?;
        range = payload;
        // Sample descriptions have a fixed header before child boxes.
        let skip = match &kind {
            b"stsd" => 8,
            b"avc1" | b"hvc1" | b"av01" if depth + 1 < path.len() => 78,
            _ => 0,
        };
        range.start += skip;
    }
    Some(range)
}

fn all(data: &[u8], path: &[&str], name: &str) -> usize {
    let range = if path.is_empty() {
        0..data.len()
    } else {
        find(data, path).unwrap()
    };
    boxes(data, range)
        .iter()
        .filter(|(kind, _)| kind == name.as_bytes())
        .count()
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap())
}

const SPS: &[u8] = &[0x67, 100, 0, 31, 0xac, 0xd9, 0x40];
const PPS: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb];
const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x00, 0x33];
const SLICE: &[u8] = &[0x41, 0x9a, 0x21, 0x6c];

fn annex_b(nals: &[&[u8]]) -> Vec<u8> {
    nals.iter()
        .flat_map(|nal| [&[0u8, 0, 0, 1][..], nal].concat())
        .collect()
}

fn h264(path: &Path, frames: &[(u64, u64, bool)]) -> Mp4Writer {
    let mut writer =
        Mp4Writer::new_codec(path, 1920, 1080, Codec::H264, &annex_b(&[SPS, PPS])).unwrap();
    for &(presentation, decode, key) in frames {
        let packet = if key {
            annex_b(&[SPS, PPS, IDR])
        } else {
            annex_b(&[SLICE])
        };
        writer
            .write_timed(&packet, presentation, decode, key)
            .unwrap();
    }
    writer
}

#[test]
fn h264_layout_strips_parameter_sets_and_indexes_samples() {
    let dir = TempDir::new("h264");
    let path = dir.file("video.mp4");
    let mut writer = h264(&path, &[(0, 0, true), (1, 1, false), (2, 2, false)]);
    assert_eq!(writer.sample_count(), 3);
    writer.finalize(30, None).unwrap();
    drop(writer);
    let data = std::fs::read(&path).unwrap();

    let top: Vec<_> = boxes(&data, 0..data.len())
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    assert_eq!(top, [*b"ftyp", *b"mdat", *b"moov"]);

    let stbl = ["moov", "trak", "mdia", "minf", "stbl"];
    let stsz = find(&data, &[&stbl[..], &["stsz"]].concat()).unwrap();
    assert_eq!(u32_at(&data, stsz.start + 8), 3, "sample count");
    // SPS/PPS move to avcC; each sample keeps one length-prefixed NAL.
    assert_eq!(u32_at(&data, stsz.start + 12), 4 + IDR.len() as u32);
    assert_eq!(u32_at(&data, stsz.start + 16), 4 + SLICE.len() as u32);

    let stss = find(&data, &[&stbl[..], &["stss"]].concat()).unwrap();
    assert_eq!(u32_at(&data, stss.start + 4), 1, "one sync sample");
    assert_eq!(u32_at(&data, stss.start + 8), 1, "first sample is sync");

    let avcc = find(&data, &[&stbl[..], &["stsd", "avc1", "avcC"]].concat()).unwrap();
    assert_eq!(&data[avcc.start..avcc.start + 4], &[1, 100, 0, 31]);

    // In-order frames need neither composition offsets nor an edit list.
    assert_eq!(all(&data, &stbl, "ctts"), 0);
    assert!(find(&data, &["moov", "trak", "edts"]).is_none());
    assert_eq!(all(&data, &["moov"], "trak"), 1);
}

#[test]
fn reordered_frames_get_composition_offsets_and_an_edit() {
    let dir = TempDir::new("bframes");
    let path = dir.file("video.mp4");
    // Decode order I P B with presentation 0 2 1.
    let mut writer = h264(&path, &[(0, 0, true), (2, 1, false), (1, 2, false)]);
    writer.finalize(30, None).unwrap();
    drop(writer);
    let data = std::fs::read(&path).unwrap();
    let stbl = ["moov", "trak", "mdia", "minf", "stbl"];
    assert_eq!(all(&data, &stbl, "ctts"), 1);
    assert!(find(&data, &["moov", "trak", "edts", "elst"]).is_some());
}

#[test]
fn invalid_timestamps_are_rejected() {
    let dir = TempDir::new("timestamps");
    let path = dir.file("video.mp4");
    let mut writer = h264(&path, &[(0, 0, true)]);
    let packet = annex_b(&[SLICE]);
    assert!(
        writer.write_timed(&packet, 1, 0, false).is_err(),
        "decode must increase"
    );
    writer.write_timed(&packet, 0, 1, false).unwrap();
    assert!(
        writer.finalize(30, None).is_err(),
        "duplicate presentation time"
    );
}

#[test]
fn av1_configuration_comes_from_the_sequence_header() {
    let dir = TempDir::new("av1");
    let path = dir.file("video.mp4");
    // Sequence header OBU: profile 0, reduced still-picture header, level 8.
    let sequence = [0x0a, 0x02, 0x0a, 0x00];
    let temporal_delimiter = [0x12, 0x00];
    let frame = [0x32, 0x03, 0xaa, 0xbb, 0xcc];
    let mut writer = Mp4Writer::new_codec(&path, 1280, 720, Codec::Av1, &sequence).unwrap();
    writer
        .write(
            &[&temporal_delimiter[..], &sequence, &frame].concat(),
            0,
            true,
        )
        .unwrap();
    writer.finalize(60, None).unwrap();
    drop(writer);
    let data = std::fs::read(&path).unwrap();
    let stbl = ["moov", "trak", "mdia", "minf", "stbl"];
    let av1c = find(&data, &[&stbl[..], &["stsd", "av01", "av1C"]].concat()).unwrap();
    assert_eq!(&data[av1c.start..av1c.start + 4], &[0x81, 0x08, 0x0c, 0x00]);
    assert_eq!(&data[av1c.start + 4..av1c.end], &sequence);
    let stsz = find(&data, &[&stbl[..], &["stsz"]].concat()).unwrap();
    // Temporal delimiters are dropped from samples.
    assert_eq!(
        u32_at(&data, stsz.start + 12),
        (sequence.len() + frame.len()) as u32
    );
}

#[test]
fn malformed_input_is_rejected() {
    let dir = TempDir::new("malformed");
    let path = dir.file("video.mp4");
    assert!(Mp4Writer::new_codec(&path, 1920, 1080, Codec::H264, &[1, 2, 3]).is_err());
    assert!(Mp4Writer::new_codec(&path, 1920, 1080, Codec::Av1, &[0x0a, 0x05, 0x00]).is_err());
    assert!(Mp4Writer::new_codec(&path, 70_000, 1080, Codec::H264, &annex_b(&[SPS, PPS])).is_err());
}

fn aac_track(dir: &TempDir, packets: usize) -> AacTrack {
    let path = dir.file("audio.aac.partial");
    let sizes = vec![7u32; packets];
    std::fs::write(&path, vec![0x21; 7 * packets]).unwrap();
    AacTrack {
        path,
        sizes,
        frames: packets as u64 * 1024,
        priming: 1024,
    }
}

#[test]
fn audio_is_interleaved_on_finalize() {
    let dir = TempDir::new("audio");
    let path = dir.file("video.mp4");
    let audio = aac_track(&dir, 4);
    let mut writer = h264(&path, &[(0, 0, true), (1, 1, false)]);
    writer.finalize(30, Some(&audio)).unwrap();
    drop(writer);
    let data = std::fs::read(&path).unwrap();
    assert_eq!(all(&data, &["moov"], "trak"), 2);
    let mvhd = find(&data, &["moov", "mvhd"]).unwrap();
    assert_eq!(u32_at(&data, mvhd.end - 4), 3, "next track ID");
    let spool = audio.path.clone();
    drop(audio);
    assert!(!spool.exists(), "dropping a track removes its spool");
}

#[test]
fn audio_attaches_to_a_finished_file_without_moving_video() {
    let dir = TempDir::new("attach");
    let path = dir.file("video.mp4");
    let mut writer = h264(&path, &[(0, 0, true), (1, 1, false)]);
    writer.finalize(30, None).unwrap();
    drop(writer);
    let before = std::fs::read(&path).unwrap();
    let video = find(&before, &["mdat"]).unwrap();

    let audio = aac_track(&dir, 3);
    attach_audio(&path, &audio).unwrap();
    let data = std::fs::read(&path).unwrap();
    assert_eq!(
        &data[video.clone()],
        &before[video],
        "video bytes untouched"
    );
    let top: Vec<_> = boxes(&data, 0..data.len())
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    assert_eq!(top, [*b"ftyp", *b"mdat", *b"free", *b"mdat", *b"moov"]);
    assert_eq!(all(&data, &["moov"], "trak"), 2);
    let mvhd = find(&data, &["moov", "mvhd"]).unwrap();
    assert_eq!(u32_at(&data, mvhd.end - 4), 3, "next track ID");
}
