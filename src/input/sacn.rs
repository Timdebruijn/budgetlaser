//! sACN / E1.31 receiver: one universe via multicast on UDP 5568.

use super::{SenderId, Sink, bind_shared};
use std::net::Ipv4Addr;

pub const PORT: u16 = 5568;
const ACN_ID: &[u8; 12] = b"ASC-E1.17\0\0\0";

pub struct Packet<'a> {
    pub universe: u16,
    pub cid: [u8; 16],
    pub priority: u8,
    /// Sender stops this stream; `data` is empty.
    pub terminated: bool,
    /// DMX data without the start code.
    pub data: &'a [u8],
}

/// Parses an E1.31 data packet. Preview data and non-zero start codes are ignored.
pub fn parse(pkt: &[u8]) -> Option<Packet<'_>> {
    if pkt.len() < 126 || &pkt[4..16] != ACN_ID {
        return None;
    }
    let root_vector = u32::from_be_bytes([pkt[18], pkt[19], pkt[20], pkt[21]]);
    let frame_vector = u32::from_be_bytes([pkt[40], pkt[41], pkt[42], pkt[43]]);
    if root_vector != 0x0000_0004 || frame_vector != 0x0000_0002 || pkt[117] != 0x02 {
        return None;
    }
    let options = pkt[112];
    let mut p = Packet {
        universe: u16::from_be_bytes([pkt[113], pkt[114]]),
        cid: pkt[22..38].try_into().ok()?,
        priority: pkt[108].min(200),
        terminated: options & 0x40 != 0,
        data: &[],
    };
    if p.terminated {
        return Some(p);
    }
    if options & 0x80 != 0 {
        return None; // preview data, not for live output
    }
    let count = u16::from_be_bytes([pkt[123], pkt[124]]) as usize; // includes start code
    if count == 0 || pkt[125] != 0 {
        return None; // start code 0 only (0xDD per-address priority is not supported)
    }
    p.data = pkt.get(126..126 + (count - 1).min(512))?;
    Some(p)
}

pub fn run(universe: u16, out: Sink) -> std::io::Result<()> {
    let sock = bind_shared(PORT)?;
    let group = Ipv4Addr::new(239, 255, (universe >> 8) as u8, universe as u8);
    sock.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED)?;
    log::info!("sACN listening on UDP {PORT}, universe {universe} ({group})");
    let mut buf = [0u8; 1024];
    loop {
        let (n, _) = sock.recv_from(&mut buf)?;
        let Some(p) = parse(&buf[..n]) else { continue };
        if p.universe != universe {
            continue;
        }
        let id = SenderId::Sacn(p.cid);
        if p.terminated {
            out.stop(id);
        } else {
            out.send(id, p.priority, p.data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn packet(universe: u16, dmx: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; 126];
        p[0..2].copy_from_slice(&0x0010u16.to_be_bytes());
        p[4..16].copy_from_slice(ACN_ID);
        p[18..22].copy_from_slice(&4u32.to_be_bytes());
        p[40..44].copy_from_slice(&2u32.to_be_bytes());
        p[108] = 100; // priority
        p[113..115].copy_from_slice(&universe.to_be_bytes());
        p[117] = 0x02;
        p[118] = 0xa1;
        p[121..123].copy_from_slice(&1u16.to_be_bytes());
        p[123..125].copy_from_slice(&(dmx.len() as u16 + 1).to_be_bytes());
        p[125] = 0;
        p.extend_from_slice(dmx);
        p
    }

    #[test]
    fn parses_e131() {
        let p = packet(7, &[1, 2, 3]);
        let got = parse(&p).unwrap();
        assert_eq!(
            (got.universe, got.priority, got.data),
            (7, 100, &[1u8, 2, 3][..])
        );
        assert!(!got.terminated);
        let mut t = p.clone();
        t[112] = 0x40;
        assert!(parse(&t).unwrap().terminated);
        let mut preview = p.clone();
        preview[112] = 0x80;
        assert!(parse(&preview).is_none());
        let mut s = p.clone();
        s[125] = 0xdd;
        assert!(parse(&s).is_none());
    }
}
