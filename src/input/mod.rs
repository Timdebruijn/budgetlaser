//! DMX sources. Each runs in its own blocking thread and publishes full
//! universes into a lock-free triple buffer; the render loop never waits.

pub mod artnet;
pub mod sacn;
pub mod usbpro;

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
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

pub struct Publisher(Input<Universe>);

impl Publisher {
    pub fn publish(&mut self, data: &[u8]) {
        let buf = self.0.input_buffer_mut();
        let n = data.len().min(512);
        buf[..n].copy_from_slice(&data[..n]);
        buf[n..].fill(0);
        self.0.publish();
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

pub fn channel() -> (Publisher, Receiver) {
    let (i, o) = TripleBuffer::new(&[0u8; 512]).split();
    (Publisher(i), Receiver(o))
}
