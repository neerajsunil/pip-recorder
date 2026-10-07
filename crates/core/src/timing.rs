//! Media clock helpers. All timestamps are in 100 ns units.

/// Frame pacing uses a wall-clock grid, never frame arrival counts.
pub fn frame_time(index: u64, fps: u32) -> i64 {
    ((u128::from(index) * 10_000_000) / u128::from(fps)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_clock_has_no_accumulating_rounding_drift() {
        assert_eq!(frame_time(60 * 3600, 60), 36_000_000_000);
        assert_eq!(frame_time(30 * 3600, 30), 36_000_000_000);
        assert!(frame_time(1, 60) < frame_time(2, 60));
    }
}
