//! Annex B NAL parsing and `avcC` / `hvcC` construction.
use crate::boxes::invalid;
use fastrecorder_core::Codec;
use std::io;

pub(crate) fn nals(bytes: &[u8]) -> io::Result<Vec<&[u8]>> {
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
pub(crate) fn configuration(codec: Codec, header: &[u8]) -> io::Result<Vec<u8>> {
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
