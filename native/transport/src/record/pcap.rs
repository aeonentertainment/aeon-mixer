//! Classic pcap, as libpcap 2.4 writes it: microsecond times, an Ethernet link layer, little
//! endian. A slot's datagram goes behind made-up Ethernet, IPv4 and UDP headers, since that is
//! the one shape Wireshark, tcpdump and libwebrtc's `rtp_file_reader.cc` all read.

use std::net::Ipv4Addr;
use std::time::Duration;

/// The microsecond magic, written little endian.
pub const MAGIC_MICROS: u32 = 0xa1b2_c3d4;
/// The nanosecond magic another tool may have written.
pub const MAGIC_NANOS: u32 = 0xa1b2_3c4d;
pub const LINKTYPE_ETHERNET: u32 = 1;
pub const GLOBAL_HEADER_LEN: usize = 24;
pub const RECORD_HEADER_LEN: usize = 16;
pub const ETHERNET_LEN: usize = 14;
pub const IPV4_LEN: usize = 20;
pub const UDP_LEN: usize = 8;
/// What goes in front of a datagram in the file.
pub const FRAMING_LEN: usize = RECORD_HEADER_LEN + ETHERNET_LEN + IPV4_LEN + UDP_LEN;

const SNAPLEN: u32 = 65_535;
const ETHERTYPE_IPV4: u16 = 0x0800;
const PROTOCOL_UDP: u8 = 17;

pub fn global_header() -> [u8; GLOBAL_HEADER_LEN] {
    let mut out = [0u8; GLOBAL_HEADER_LEN];
    out[0..4].copy_from_slice(&MAGIC_MICROS.to_le_bytes());
    out[4..6].copy_from_slice(&2u16.to_le_bytes());
    out[6..8].copy_from_slice(&4u16.to_le_bytes());
    // thiszone and sigfigs stay zero
    out[16..20].copy_from_slice(&SNAPLEN.to_le_bytes());
    out[20..24].copy_from_slice(&LINKTYPE_ETHERNET.to_le_bytes());
    out
}

/// One datagram as a pcap record, appended to `out`: `at` since the Unix epoch, from `source`
/// to `port` on the unspecified address, which is all a socket bound on every interface knows.
pub fn append_record(
    out: &mut Vec<u8>,
    at: Duration,
    source: (Ipv4Addr, u16),
    port: u16,
    datagram: &[u8],
) {
    let captured = (ETHERNET_LEN + IPV4_LEN + UDP_LEN + datagram.len()) as u32;
    out.extend((at.as_secs() as u32).to_le_bytes());
    out.extend(at.subsec_micros().to_le_bytes());
    out.extend(captured.to_le_bytes());
    out.extend(captured.to_le_bytes());

    out.extend([0u8; 12]);
    out.extend(ETHERTYPE_IPV4.to_be_bytes());

    let mut ip = [0u8; IPV4_LEN];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&((IPV4_LEN + UDP_LEN + datagram.len()) as u16).to_be_bytes());
    ip[6] = 0x40; // don't fragment
    ip[8] = 64;
    ip[9] = PROTOCOL_UDP;
    ip[12..16].copy_from_slice(&source.0.octets());
    let checksum = ipv4_checksum(&ip);
    ip[10..12].copy_from_slice(&checksum.to_be_bytes());
    out.extend(ip);

    out.extend(source.1.to_be_bytes());
    out.extend(port.to_be_bytes());
    out.extend(((UDP_LEN + datagram.len()) as u16).to_be_bytes());
    out.extend([0u8; 2]); // no UDP checksum, which IPv4 allows
    out.extend(datagram);
}

/// RFC 791's header checksum: the ones' complement of the ones' complement sum of its words.
fn ipv4_checksum(header: &[u8; IPV4_LEN]) -> u16 {
    let sum: u32 =
        header.as_chunks::<2>().0.iter().map(|w| u32::from(u16::from_be_bytes(*w))).sum();
    let folded = (sum & 0xffff) + (sum >> 16);
    !(((folded & 0xffff) + (folded >> 16)) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field where a reader looks for it, and an IPv4 header Wireshark will not flag.
    #[test]
    fn a_record_carries_its_datagram_behind_headers_that_check() {
        let mut out = Vec::new();
        let at = Duration::new(1_700_000_000, 123_456_789);
        append_record(&mut out, at, (Ipv4Addr::new(192, 168, 1, 20), 50_000), 40_123, &[9, 8, 7]);
        assert_eq!(out.len(), FRAMING_LEN + 3);
        assert_eq!(&out[0..4], &1_700_000_000u32.to_le_bytes());
        assert_eq!(&out[4..8], &123_456u32.to_le_bytes(), "microseconds, truncated");
        assert_eq!(&out[8..12], &45u32.to_le_bytes());
        let ip: &[u8; IPV4_LEN] = out[30..50].try_into().unwrap();
        assert_eq!(ipv4_checksum(ip), 0, "the checksum makes the header sum to all ones");
        assert_eq!(&ip[12..16], &[192, 168, 1, 20]);
        assert_eq!(&out[50..52], &50_000u16.to_be_bytes());
        assert_eq!(&out[52..54], &40_123u16.to_be_bytes());
        assert_eq!(&out[54..56], &11u16.to_be_bytes());
        assert_eq!(&out[58..], &[9, 8, 7]);
        assert_eq!(&global_header()[0..4], &[0xd4, 0xc3, 0xb2, 0xa1]);
    }
}
