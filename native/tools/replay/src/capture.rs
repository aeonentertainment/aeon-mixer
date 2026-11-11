//! Classic pcap in, UDP datagrams out. Reads what a slot's recording writes and what tcpdump or
//! an older Wireshark write: either magic in either byte order, and the link layers a capture of
//! one RTP stream comes on. pcapng is refused with the command that converts it.

use std::net::Ipv4Addr;

use transport::record::pcap::{MAGIC_MICROS, MAGIC_NANOS};

/// One UDP datagram as captured, its time in microseconds since the Unix epoch.
#[derive(Debug)]
pub struct Datagram {
    pub at_micros: i64,
    pub source: (Ipv4Addr, u16),
    pub port: u16,
    pub bytes: Vec<u8>,
}

const LINKTYPE_NULL: u32 = 0;
const LINKTYPE_ETHERNET: u32 = 1;
const LINKTYPE_RAW: u32 = 101;
const LINKTYPE_LOOP: u32 = 108;
const LINKTYPE_LINUX_SLL: u32 = 113;
const LINKTYPE_IPV4: u32 = 228;
const LINKTYPES: [u32; 6] = [
    LINKTYPE_NULL,
    LINKTYPE_ETHERNET,
    LINKTYPE_RAW,
    LINKTYPE_LOOP,
    LINKTYPE_LINUX_SLL,
    LINKTYPE_IPV4,
];
const PCAPNG: u32 = 0x0a0d_0d0a;

/// Every IPv4 UDP datagram in `file`, in file order. Anything else on the link is skipped.
pub fn read(file: &[u8]) -> Result<Vec<Datagram>, String> {
    let magic = file.first_chunk::<4>().ok_or("not a pcap file: under four bytes")?;
    if u32::from_le_bytes(*magic) == PCAPNG {
        return Err("a pcapng file: convert it with `editcap -F pcap IN.pcapng OUT.pcap`".into());
    }
    let (big_endian, nanos) = match (u32::from_le_bytes(*magic), u32::from_be_bytes(*magic)) {
        (MAGIC_MICROS, _) => (false, false),
        (MAGIC_NANOS, _) => (false, true),
        (_, MAGIC_MICROS) => (true, false),
        (_, MAGIC_NANOS) => (true, true),
        _ => return Err("not a pcap file: no pcap magic".into()),
    };
    let word = |at: usize| -> Option<u32> {
        let bytes = *file.get(at..at + 4)?.first_chunk::<4>()?;
        Some(if big_endian { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) })
    };
    let linktype = word(20).ok_or("a pcap file cut short in its header")? & 0x0fff_ffff;
    if !LINKTYPES.contains(&linktype) {
        return Err(format!("link type {linktype} is not one this reads"));
    }
    let mut datagrams = Vec::new();
    let mut at = 24;
    while let (Some(seconds), Some(fraction), Some(captured)) =
        (word(at), word(at + 4), word(at + 8))
    {
        let start = at + 16;
        let Some(frame) = file.get(start..start + captured as usize) else { break };
        at = start + captured as usize;
        let micros = if nanos { i64::from(fraction) / 1000 } else { i64::from(fraction) };
        let Some(ip) = ip_of(frame, linktype) else { continue };
        if let Some((source, port, bytes)) = udp_of(ip) {
            let at_micros = i64::from(seconds) * 1_000_000 + micros;
            datagrams.push(Datagram { at_micros, source, port, bytes: bytes.to_vec() });
        }
    }
    Ok(datagrams)
}

/// The IPv4 packet inside one link-layer frame.
fn ip_of(frame: &[u8], linktype: u32) -> Option<&[u8]> {
    match linktype {
        // The address family, in the capturing machine's byte order: AF_INET is 2 everywhere
        LINKTYPE_NULL | LINKTYPE_LOOP => {
            let family = frame.first_chunk::<4>()?;
            (u32::from_le_bytes(*family) == 2 || u32::from_be_bytes(*family) == 2)
                .then(|| &frame[4..])
        }
        LINKTYPE_ETHERNET => {
            let mut at = 12;
            let mut ethertype = u16::from_be_bytes(*frame.get(at..)?.first_chunk()?);
            // One 802.1Q tag, as a switch port on a VLAN adds
            if ethertype == 0x8100 {
                at += 4;
                ethertype = u16::from_be_bytes(*frame.get(at..)?.first_chunk()?);
            }
            (ethertype == 0x0800).then(|| &frame[at + 2..])
        }
        LINKTYPE_LINUX_SLL => {
            let protocol = u16::from_be_bytes(*frame.get(14..)?.first_chunk()?);
            (protocol == 0x0800).then(|| &frame[16..])
        }
        LINKTYPE_RAW | LINKTYPE_IPV4 => Some(frame),
        _ => None,
    }
}

/// An unfragmented IPv4 UDP datagram: its source, its destination port and its payload.
fn udp_of(ip: &[u8]) -> Option<((Ipv4Addr, u16), u16, &[u8])> {
    let first = *ip.first()?;
    if first >> 4 != 4 || *ip.get(9)? != 17 {
        return None;
    }
    let fragment = u16::from_be_bytes(*ip.get(6..)?.first_chunk()?);
    if fragment & 0x3fff != 0 {
        return None;
    }
    let header = usize::from(first & 0x0f) * 4;
    let total = usize::from(u16::from_be_bytes(*ip.get(2..)?.first_chunk()?));
    let source = Ipv4Addr::from(*ip.get(12..)?.first_chunk::<4>()?);
    let udp = ip.get(header..total.min(ip.len()))?;
    let source_port = u16::from_be_bytes(*udp.first_chunk()?);
    let port = u16::from_be_bytes(*udp.get(2..)?.first_chunk()?);
    let length = usize::from(u16::from_be_bytes(*udp.get(4..)?.first_chunk()?));
    Some(((source, source_port), port, udp.get(8..length.min(udp.len()))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tcpdump on macOS's loopback: big endian, nanoseconds, a BSD null header in host order.
    #[test]
    fn a_capture_another_tool_wrote_reads_the_same_datagram() {
        let mut file = Vec::new();
        file.extend(MAGIC_NANOS.to_be_bytes());
        file.extend([0, 2, 0, 4]);
        file.extend([0u8; 8]);
        file.extend(65_535u32.to_be_bytes());
        file.extend(LINKTYPE_NULL.to_be_bytes());
        let mut frame = 2u32.to_le_bytes().to_vec();
        let mut ip = [0u8; 20];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&31u16.to_be_bytes());
        ip[9] = 17;
        ip[12..16].copy_from_slice(&[127, 0, 0, 1]);
        frame.extend(ip);
        frame.extend(5004u16.to_be_bytes());
        frame.extend(40_000u16.to_be_bytes());
        frame.extend(11u16.to_be_bytes());
        frame.extend([0, 0, 7, 8, 9]);
        file.extend(10u32.to_be_bytes());
        file.extend(250_000_999u32.to_be_bytes());
        file.extend((frame.len() as u32).to_be_bytes());
        file.extend((frame.len() as u32).to_be_bytes());
        file.extend(frame);

        let datagrams = read(&file).unwrap();
        assert_eq!(datagrams.len(), 1);
        let datagram = &datagrams[0];
        assert_eq!(datagram.at_micros, 10_250_000);
        assert_eq!(datagram.source, (Ipv4Addr::LOCALHOST, 5004));
        assert_eq!(datagram.port, 40_000);
        assert_eq!(datagram.bytes, [7, 8, 9]);
        assert!(read(&PCAPNG.to_le_bytes()).unwrap_err().contains("editcap"));
    }
}
