//! Art-Net receiver: ArtDmx on UDP 6454, answers ArtPoll so consoles find us.

use super::{Publisher, bind_shared};
use std::net::{SocketAddr, UdpSocket};

pub const PORT: u16 = 6454;
const ID: &[u8; 8] = b"Art-Net\0";
const OP_POLL: u16 = 0x2000;
const OP_POLL_REPLY: u16 = 0x2100;
const OP_DMX: u16 = 0x5000;

/// Returns (15-bit port-address, dmx data) for an ArtDmx packet.
pub fn parse_dmx(pkt: &[u8]) -> Option<(u16, &[u8])> {
    if pkt.len() < 18 || &pkt[..8] != ID || u16::from_le_bytes([pkt[8], pkt[9]]) != OP_DMX {
        return None;
    }
    let universe = u16::from_le_bytes([pkt[14], pkt[15]]) & 0x7fff;
    let len = u16::from_be_bytes([pkt[16], pkt[17]]) as usize;
    let data = pkt.get(18..18 + len.min(512))?;
    Some((universe, data))
}

pub fn is_poll(pkt: &[u8]) -> bool {
    pkt.len() >= 12 && &pkt[..8] == ID && u16::from_le_bytes([pkt[8], pkt[9]]) == OP_POLL
}

pub fn poll_reply(ip: [u8; 4], universe: u16) -> [u8; 239] {
    let mut r = [0u8; 239];
    r[..8].copy_from_slice(ID);
    r[8..10].copy_from_slice(&OP_POLL_REPLY.to_le_bytes());
    r[10..14].copy_from_slice(&ip);
    r[14..16].copy_from_slice(&PORT.to_le_bytes());
    r[18] = ((universe >> 8) & 0x7f) as u8; // NetSwitch
    r[19] = ((universe >> 4) & 0x0f) as u8; // SubSwitch
    let short = b"BudgetLaser";
    r[26..26 + short.len()].copy_from_slice(short);
    let long = b"BudgetLaser beamer laser simulator";
    r[44..44 + long.len()].copy_from_slice(long);
    r[173] = 1; // NumPorts (hi, lo)
    r[174] = 0x80; // PortTypes: output from Art-Net, DMX512
    r[182] = 0x80; // GoodOutput: data being transmitted
    r[190] = (universe & 0x0f) as u8; // SwOut
    r[212] = 0x08; // Status2: supports 15-bit port-address
    r
}

pub fn run(universe: u16, mut out: Publisher) -> std::io::Result<()> {
    let sock = bind_shared(PORT)?;
    sock.set_broadcast(true)?;
    log::info!("Art-Net listening on UDP {PORT}, port-address {universe}");
    let mut buf = [0u8; 1024];
    loop {
        let (n, from) = sock.recv_from(&mut buf)?;
        let pkt = &buf[..n];
        if let Some((u, data)) = parse_dmx(pkt) {
            if u == universe {
                out.publish(data);
            }
        } else if is_poll(pkt) {
            // ponytail: unicast reply to the poller; directed broadcast if a console ignores it.
            let ip = local_ip_for(from).unwrap_or([0; 4]);
            let _ = sock.send_to(&poll_reply(ip, universe), from);
        }
    }
}

fn local_ip_for(peer: SocketAddr) -> Option<[u8; 4]> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(peer).ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(a) => Some(a.ip().octets()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_artdmx() {
        let mut p = Vec::new();
        p.extend_from_slice(ID);
        p.extend_from_slice(&OP_DMX.to_le_bytes());
        p.extend_from_slice(&[0, 14, 1, 0]); // protver, seq, physical
        p.extend_from_slice(&0x0103u16.to_le_bytes()); // net 1, subnet 0, universe 3
        p.extend_from_slice(&4u16.to_be_bytes());
        p.extend_from_slice(&[10, 20, 30, 40]);
        let (u, d) = parse_dmx(&p).unwrap();
        assert_eq!(u, 0x0103);
        assert_eq!(d, &[10, 20, 30, 40]);
        assert!(parse_dmx(&p[..17]).is_none());
        let mut poll = p.clone();
        poll[8..10].copy_from_slice(&OP_POLL.to_le_bytes());
        assert!(is_poll(&poll) && !is_poll(&p));
        let r = poll_reply([10, 0, 0, 2], 0x0103);
        assert_eq!((r[18], r[19], r[190]), (1, 0, 3));
    }
}
