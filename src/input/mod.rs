//! DMX sources. Each runs in its own blocking thread and hands every received
//! universe to one merger; the merged result goes into a lock-free triple
//! buffer, so the render loop never waits.

pub mod artnet;
pub mod sacn;
pub mod usbpro;

use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use triple_buffer::{Input, Output, TripleBuffer};

/// UDP socket on `port` that can share it with another Art-Net/sACN app on the same host
/// (QLC+ next to the simulator). Broadcast and multicast reach every sharer; unicast reaches one.
pub fn bind_shared(port: u16) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
    Ok(sock.into())
}

pub type Universe = [u8; 512];

/// Priority for sources without one (Art-Net, USB Pro): the sACN default, so they
/// HTP-merge with default-priority sACN senders.
pub const DEFAULT_PRIORITY: u8 = 100;
/// More senders than this on one universe is a misconfiguration; extra ones are ignored.
const MAX_SENDERS: usize = 8;

/// One sender of DMX data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SenderId {
    ArtNet(IpAddr),
    Sacn([u8; 16]),
    UsbPro,
}

impl std::fmt::Display for SenderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SenderId::ArtNet(ip) => write!(f, "Art-Net {ip}"),
            SenderId::Sacn(cid) => {
                write!(f, "sACN ")?;
                for (i, b) in cid.iter().enumerate() {
                    if matches!(i, 4 | 6 | 8 | 10) {
                        write!(f, "-")?;
                    }
                    write!(f, "{b:02x}")?;
                }
                Ok(())
            }
            SenderId::UsbPro => write!(f, "DMX USB Pro"),
        }
    }
}

impl SenderId {
    /// How long a silent sender keeps contributing to the merge.
    fn timeout(&self) -> Duration {
        match self {
            // Art-Net merge timeout per the Art-Net 4 spec.
            SenderId::ArtNet(_) => Duration::from_secs(10),
            // E1.31 network data loss timeout.
            SenderId::Sacn(_) | SenderId::UsbPro => Duration::from_millis(2500),
        }
    }
}

struct Slot {
    id: SenderId,
    priority: u8,
    data: Universe,
    seen: Instant,
}

/// Highest priority among live senders wins; senders sharing that priority merge HTP.
fn merge(slots: &[Slot], now: Instant, out: &mut Universe) -> bool {
    let live = || {
        slots
            .iter()
            .filter(|s| now.duration_since(s.seen) <= s.id.timeout())
    };
    let Some(top) = live().map(|s| s.priority).max() else {
        return false;
    };
    out.fill(0);
    for s in live().filter(|s| s.priority == top) {
        for (o, &v) in out.iter_mut().zip(&s.data) {
            *o = (*o).max(v);
        }
    }
    true
}

struct Merger {
    slots: Vec<Slot>,
    out: Input<Universe>,
    full_warned: bool,
}

impl Merger {
    fn publish(&mut self, now: Instant) {
        self.slots
            .retain(|s| now.duration_since(s.seen) <= s.id.timeout());
        if merge(&self.slots, now, self.out.input_buffer_mut()) {
            self.out.publish();
        }
    }
}

/// Handle a source thread uses to deliver data. Cloned once per source.
// ponytail: one mutex for all sources; fine at DMX rates (<= 44 Hz per sender).
#[derive(Clone)]
pub struct Sink(Arc<Mutex<Merger>>);

impl Sink {
    pub fn send(&self, id: SenderId, priority: u8, data: &[u8]) {
        let now = Instant::now();
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let slot = match m.slots.iter().position(|s| s.id == id) {
            Some(i) => &mut m.slots[i],
            None if m.slots.len() >= MAX_SENDERS => {
                if !m.full_warned {
                    log::warn!("more than {MAX_SENDERS} DMX senders; ignoring {id}");
                    m.full_warned = true;
                }
                return;
            }
            None => {
                log::info!("new DMX sender {id}, priority {priority}");
                m.slots.push(Slot {
                    id,
                    priority,
                    data: [0; 512],
                    seen: now,
                });
                m.slots.last_mut().unwrap()
            }
        };
        let n = data.len().min(512);
        slot.data[..n].copy_from_slice(&data[..n]);
        slot.data[n..].fill(0);
        slot.priority = priority;
        slot.seen = now;
        m.publish(now);
    }

    /// Sender announced it stops (sACN stream terminated): drop it right away.
    pub fn stop(&self, id: SenderId) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let before = m.slots.len();
        m.slots.retain(|s| s.id != id);
        if m.slots.len() != before {
            log::info!("DMX sender {id} terminated");
            m.publish(Instant::now());
        }
    }
}

pub struct Receiver(Output<Universe>);

impl Receiver {
    /// Latest universe and whether it is new since the last call.
    pub fn latest(&mut self) -> (bool, &Universe) {
        let fresh = self.0.update();
        (fresh, self.0.output_buffer())
    }
}

pub fn channel() -> (Sink, Receiver) {
    let (i, o) = TripleBuffer::new(&[0u8; 512]).split();
    let merger = Merger {
        slots: Vec::new(),
        out: i,
        full_warned: false,
    };
    (Sink(Arc::new(Mutex::new(merger))), Receiver(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(id: SenderId, priority: u8, first: u8, second: u8, seen: Instant) -> Slot {
        let mut data = [0; 512];
        data[0] = first;
        data[1] = second;
        Slot {
            id,
            priority,
            data,
            seen,
        }
    }

    #[test]
    fn priority_then_htp_then_timeout() {
        let now = Instant::now();
        let a = SenderId::Sacn([1; 16]);
        let b = SenderId::Sacn([2; 16]);
        let art = SenderId::ArtNet(Ipv4Addr::new(10, 0, 0, 9).into());
        let mut out = [0; 512];

        // Equal priority: highest value per channel.
        let slots = [slot(a, 100, 10, 200, now), slot(art, 100, 50, 20, now)];
        assert!(merge(&slots, now, &mut out));
        assert_eq!(out[..2], [50, 200]);

        // Higher priority wins outright, even with lower values.
        let slots = [slot(a, 100, 255, 255, now), slot(b, 150, 1, 2, now)];
        merge(&slots, now, &mut out);
        assert_eq!(out[..2], [1, 2]);

        // A silent high-priority sACN sender stops counting after 2.5 s.
        let later = now + Duration::from_secs(3);
        let slots = [slot(b, 150, 1, 2, now), slot(a, 100, 7, 8, later)];
        merge(&slots, later, &mut out);
        assert_eq!(out[..2], [7, 8]);

        // Art-Net senders get the longer 10 s merge timeout.
        let slots = [slot(art, 100, 9, 9, now)];
        assert!(merge(&slots, later, &mut out));
        assert!(!merge(&slots, now + Duration::from_secs(11), &mut out));
    }

    #[test]
    fn sink_publishes_and_drops_terminated_senders() {
        let (sink, mut rx) = channel();
        let a = SenderId::Sacn([1; 16]);
        let b = SenderId::Sacn([2; 16]);
        sink.send(a, 100, &[10, 10]);
        sink.send(b, 120, &[3]);
        let (fresh, u) = rx.latest();
        assert!(fresh);
        assert_eq!(u[..2], [3, 0]);
        sink.stop(b);
        assert_eq!(rx.latest().1[..2], [10, 10]);
        assert_eq!(
            SenderId::Sacn([0xab; 16]).to_string(),
            "sACN abababab-abab-abab-abab-abababababab"
        );
    }
}
