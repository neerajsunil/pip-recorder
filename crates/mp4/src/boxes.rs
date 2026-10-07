//! ISO BMFF box builders shared by the video writer and audio track.
use std::io;

pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub(crate) fn atom(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() + 8);
    bytes.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(data);
    bytes
}
pub(crate) fn full(kind: &[u8; 4], version_flags: u32, data: &[u8]) -> Vec<u8> {
    atom(
        kind,
        &[version_flags.to_be_bytes().as_slice(), data].concat(),
    )
}
pub(crate) fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}
pub(crate) fn matrix() -> Vec<u8> {
    words(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000])
}
pub(crate) fn descriptor(tag: u8, payload: &[u8]) -> Vec<u8> {
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
