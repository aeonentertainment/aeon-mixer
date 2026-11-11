/// Which of a device's streams holds `channel`, given each stream's channel count in order.
/// Channels count across streams, so a second stereo stream holds channels 2 and 3. None when
/// `channel` is past the last.
pub fn stream_holding(channels: &[u32], channel: u32) -> Option<usize> {
    let mut first = 0;
    channels.iter().position(|&count| {
        first += count;
        channel < first
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_count_across_streams() {
        let streams = [2, 1, 4];
        assert_eq!(stream_holding(&streams, 0), Some(0));
        assert_eq!(stream_holding(&streams, 1), Some(0));
        assert_eq!(stream_holding(&streams, 2), Some(1));
        assert_eq!(stream_holding(&streams, 3), Some(2));
        assert_eq!(stream_holding(&streams, 6), Some(2));
    }

    #[test]
    fn an_empty_stream_holds_nothing() {
        assert_eq!(stream_holding(&[0, 2], 0), Some(1));
    }

    #[test]
    fn none_past_the_last_channel() {
        assert_eq!(stream_holding(&[2, 2], 4), None);
        assert_eq!(stream_holding(&[], 0), None);
    }
}
