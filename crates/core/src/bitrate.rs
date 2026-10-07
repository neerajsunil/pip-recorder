//! Bitrate defaults for screen content.
use crate::Codec;

/// Screen-content starting points at 30 fps. Interpolate in pixel area between
/// common resolution tiers; extrapolate gently outside them. These are product
/// defaults, not guaranteed quality or a claim about codec equivalence.
pub fn recommended_bitrate_mbps(codec: Codec, width: u32, height: u32, fps: u32) -> u32 {
    let pixels = f64::from(width.max(1)) * f64::from(height.max(1));
    let rates = match codec {
        Codec::Av1 => [5.0, 8.0, 15.0],
        Codec::Hevc => [6.0, 10.0, 18.0],
        Codec::H264 => [10.0, 15.0, 28.0],
    };
    let areas = [1920.0 * 1080.0, 2560.0 * 1440.0, 3840.0 * 2160.0];
    let rate = if pixels < areas[0] {
        rates[0] * (pixels / areas[0]).powf(0.7)
    } else if pixels > areas[2] {
        rates[2] * (pixels / areas[2]).powf(0.7)
    } else {
        let i = if pixels <= areas[1] { 0 } else { 1 };
        rates[i] + (rates[i + 1] - rates[i]) * (pixels - areas[i]) / (areas[i + 1] - areas[i])
    };
    (rate * (f64::from(fps.max(1)) / 30.0).powf(0.7))
        .round()
        .clamp(1.0, 100.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recommended_bitrate_scales_with_resolution_and_frame_rate() {
        let hd = recommended_bitrate_mbps(Codec::H264, 1920, 1080, 30);
        assert_eq!(hd, 10);
        assert!(recommended_bitrate_mbps(Codec::H264, 3840, 2160, 30) > hd);
        assert!(recommended_bitrate_mbps(Codec::H264, 1920, 1080, 60) > hd);
        assert!(recommended_bitrate_mbps(Codec::Av1, 1920, 1080, 30) < hd);
        assert_eq!(recommended_bitrate_mbps(Codec::H264, 0, 0, 0), 1);
        assert!(recommended_bitrate_mbps(Codec::H264, 15360, 8640, 240) <= 100);
    }
}
