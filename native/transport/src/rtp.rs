//! The RTP fixed header of RFC 3550 section 5.1: parsed on receive, written by the return
//! sender.

pub const HEADER_LEN: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub marker: bool,
    pub payload_type: u8,
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl Header {
    /// Splits a datagram into its header and payload, stepping over CSRCs, a header extension
    /// and padding. `None` for anything that is not RTP version 2 or is too short to be.
    pub fn parse(datagram: &[u8]) -> Option<(Self, &[u8])> {
        let fixed: &[u8; HEADER_LEN] = datagram.first_chunk()?;
        if fixed[0] >> 6 != 2 {
            return None;
        }
        let padded = fixed[0] & 0x20 != 0;
        let extended = fixed[0] & 0x10 != 0;
        let csrcs = usize::from(fixed[0] & 0x0f);
        let header = Self {
            marker: fixed[1] & 0x80 != 0,
            payload_type: fixed[1] & 0x7f,
            seq: u16::from_be_bytes([fixed[2], fixed[3]]),
            timestamp: u32::from_be_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]),
            ssrc: u32::from_be_bytes([fixed[8], fixed[9], fixed[10], fixed[11]]),
        };
        let mut start = HEADER_LEN + 4 * csrcs;
        if extended {
            let words = datagram.get(start + 2..start + 4)?;
            start += 4 + 4 * usize::from(u16::from_be_bytes([words[0], words[1]]));
        }
        let padding = if padded { usize::from(*datagram.last()?) } else { 0 };
        let end = datagram.len().checked_sub(padding)?;
        Some((header, datagram.get(start..end)?))
    }

    /// The twelve bytes of a header with no CSRCs, extension or padding.
    pub fn write(&self, out: &mut [u8; HEADER_LEN]) {
        out[0] = 0x80;
        out[1] = self.payload_type & 0x7f | if self.marker { 0x80 } else { 0 };
        out[2..4].copy_from_slice(&self.seq.to_be_bytes());
        out[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        out[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: Header =
        Header { marker: true, payload_type: 111, seq: 0xBEEF, timestamp: 0x01020304, ssrc: 7 };

    #[test]
    fn a_written_header_parses_back_with_its_payload() {
        let mut datagram = [0u8; HEADER_LEN + 3];
        HEADER.write((&mut datagram[..HEADER_LEN]).try_into().unwrap());
        datagram[HEADER_LEN..].copy_from_slice(&[9, 8, 7]);
        assert_eq!(datagram[..4], [0x80, 0x80 | 111, 0xBE, 0xEF]);
        assert_eq!(Header::parse(&datagram), Some((HEADER, &[9, 8, 7][..])));
    }

    #[test]
    fn csrcs_an_extension_and_padding_are_stepped_over() {
        let mut datagram = vec![0xB2, 96, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3];
        datagram.extend_from_slice(&[0xAA; 8]); // two CSRCs
        datagram.extend_from_slice(&[0xBE, 0xDE, 0, 1, 1, 2, 3, 4]); // one-word extension
        datagram.extend_from_slice(&[0x55, 0x66]);
        datagram.extend_from_slice(&[0, 0, 3]); // three bytes of padding
        let (header, payload) = Header::parse(&datagram).unwrap();
        assert_eq!(
            header,
            Header { marker: false, payload_type: 96, seq: 1, timestamp: 2, ssrc: 3 }
        );
        assert_eq!(payload, &[0x55, 0x66]);
    }

    #[test]
    fn what_is_not_rtp_is_refused() {
        assert!(Header::parse(&[0x80; 11]).is_none(), "too short");
        assert!(Header::parse(&[0x40; 12]).is_none(), "version 1");
        assert!(
            Header::parse(&[0x90, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_none(),
            "extension cut short"
        );
        let mut padded = [0x80u8; 12];
        padded[0] = 0xA0;
        padded[11] = 13;
        assert!(Header::parse(&padded).is_none(), "padding longer than the datagram");
    }
}
