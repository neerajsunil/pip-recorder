//! AV1 low-overhead OBU parsing and `av1C` construction.
//! See https://aomediacodec.github.io/av1-isobmff/.
use crate::boxes::invalid;
use std::io;

pub(crate) type Obu<'a> = (u8, &'a [u8], &'a [u8]);
pub(crate) fn obus(data: &[u8]) -> io::Result<Vec<Obu<'_>>> {
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
pub(crate) fn configuration(data: &[u8]) -> io::Result<Vec<u8>> {
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
