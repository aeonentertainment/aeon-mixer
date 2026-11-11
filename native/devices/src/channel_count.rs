use std::ops::RangeInclusive;

/// The channel count to open an ALSA device at, so that it carries at least `least` channels: two
/// where that is enough and the device takes it, else the fewest it takes that are. `range` is the
/// device's minimum and maximum, and `takes` asks whether it takes a count, since a device may
/// take some counts in its range and not others. None when no count reaches `least`.
pub fn channel_count(
    least: u32,
    range: RangeInclusive<u32>,
    takes: impl Fn(u32) -> bool,
) -> Option<u32> {
    let (min, max) = range.into_inner();
    if least <= 2 && takes(2) {
        return Some(2);
    }
    (least.max(min)..=max).find(|&count| takes(count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_where_it_carries_the_channel() {
        let takes = |count| count == 2 || count == 8;
        assert_eq!(channel_count(1, 1..=8, takes), Some(2));
        assert_eq!(channel_count(2, 1..=8, takes), Some(2));
    }

    #[test]
    fn a_later_channel_opens_the_fewest_counts_that_reach_it() {
        let takes = |count| count == 2 || count == 8;
        assert_eq!(channel_count(5, 2..=8, takes), Some(8));
        assert_eq!(channel_count(3, 1..=10, |count| count != 2), Some(3));
    }

    #[test]
    fn a_device_with_no_stereo_opens_at_its_least() {
        assert_eq!(channel_count(1, 1..=1, |count| count == 1), Some(1));
        assert_eq!(channel_count(1, 4..=4, |count| count == 4), Some(4));
    }

    #[test]
    fn none_when_the_channel_is_past_every_count() {
        assert_eq!(channel_count(9, 2..=8, |count| count == 2 || count == 8), None);
    }
}
