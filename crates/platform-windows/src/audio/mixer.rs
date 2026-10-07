//! Fixed-size stereo ring that endpoints accumulate into, drained as 16-bit PCM.
use super::RING_FRAMES;

pub(super) struct Mixer {
    pub(super) frames: Vec<[f32; 2]>,
    pub(super) cursor: u64,
}
impl Mixer {
    pub(super) fn new() -> Self {
        Self {
            frames: vec![[0.; 2]; RING_FRAMES],
            cursor: 0,
        }
    }
    pub(super) fn take(&mut self, count: u64) -> Vec<u8> {
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
