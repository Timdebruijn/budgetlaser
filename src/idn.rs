//! IDN (ILDA Digital Network) receiver: laser show software finds the projector with
//! an IDN-Hello scan and streams IDN-Stream channel messages to UDP 7255 (IDN-RT).
//! Wire format follows ILDA IDN-Stream revision 002 (2025) and the IDN-Hello draft of
//! 2022-03-27. Everything is big-endian.

use crate::point::Point;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PORT: u16 = 7255;
/// IDN-Hello: a link without packets closes after 1 s and stops the output.
const LINK_TIMEOUT: Duration = Duration::from_secs(1);
/// Locally administered, so it cannot clash with a real projector.
const MAC: [u8; 6] = [0x02, 0x42, 0x4c, 0x49, 0x44, 0x4e];
const NAME: &[u8] = b"BudgetLaser";
/// Wave samples the renderer has not taken yet; about a second at 100k samples/s.
const MAX_PENDING: usize = 100_000;

const PING_REQUEST: u8 = 0x08;
const PING_RESPONSE: u8 = 0x09;
const GROUP_REQUEST: u8 = 0x0C;
const GROUP_RESPONSE: u8 = 0x0D;
const SCAN_REQUEST: u8 = 0x10;
const SCAN_RESPONSE: u8 = 0x11;
const SERVICEMAP_REQUEST: u8 = 0x12;
const SERVICEMAP_RESPONSE: u8 = 0x13;
const RT_MESSAGE: u8 = 0x40;
const RT_MESSAGE_ACK: u8 = 0x41;
const RT_CLOSE: u8 = 0x44;
const RT_CLOSE_ACK: u8 = 0x45;
const RT_ABORT: u8 = 0x46;
const RT_ACKNOWLEDGE: u8 = 0x47;

const ACK_OK: u8 = 0x00;
const ACK_EMPTY_CLOSE: u8 = 0xEB;
const ACK_OCCUPIED: u8 = 0xEC;
const ACK_INVALID: u8 = 0xEE;
/// Acknowledge input event: a new connection since the last acknowledge.
const EVENT_NEW: u16 = 1 << 0;
/// Acknowledge pipeline event: the message's channel is closed.
const EVENT_CHANNEL_CLOSED: u16 = 1 << 1;

const SERVICE_LASER: u8 = 0x80;
const MODE_CONTINUOUS: u8 = 0x01;
const MODE_DISCRETE: u8 = 0x02;

const CHUNK_VOID: u8 = 0x00;
const CHUNK_WAVE: u8 = 0x01;
const CHUNK_FRAME: u8 = 0x02;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Field {
    Ignore,
    X,
    Y,
    /// A colour line, as weights on the R, G and B the renderer draws.
    Color([f32; 3]),
    /// A colour line whose wavelength comes from the preceding prefix octet.
    DynamicColor,
    WavelengthPrefix,
    /// Hint octet in format 0 or 1 (1 adds the shutter bit).
    Hint(u8),
}

/// Sample layout built from a channel's tag dictionary: one field per descriptor
/// tag, widened by the precision tags that follow it.
#[derive(Debug, PartialEq)]
struct Decoder {
    fields: Vec<(Field, usize)>,
    size: usize,
}

impl Decoder {
    fn new(tags: &[u8]) -> Option<Self> {
        let mut fields: Vec<(Field, usize)> = Vec::new();
        let (mut skip, mut second_head) = (0, false);
        for t in tags
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&t| u16::from_be_bytes(t))
        {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let field = match t >> 8 {
                0x00 => {
                    skip = t & 0xF; // void, skips its suffix words
                    continue;
                }
                // Break starts the next scan head; one head is drawn, so the octets
                // of the others are counted but ignored.
                0x10 => {
                    second_head = true;
                    continue;
                }
                0x11 => continue, // coordinate and colour space modifier
                0x40 if t == 0x4010 => {
                    fields.last_mut()?.1 += 1; // precision widens the previous field
                    continue;
                }
                0x41 if t & 0xF0 == 0 && t & 0xF <= 1 => Field::Hint((t & 0xF) as u8),
                0x42 if t == 0x4200 => Field::X,
                0x42 if t == 0x4210 => Field::Y,
                0x50..=0x53 => match t & 0x3FF {
                    0 => Field::DynamicColor,
                    nm => Field::Color(line(nm as f32)),
                },
                0x5C if t == 0x5C00 => Field::WavelengthPrefix,
                // NOP, Z, second scanners, intensity (legacy only), beam brush and
                // unknown tags of a defined category: one octet the decoder skips.
                0x40..=0x42 | 0x5C => Field::Ignore,
                // An undefined category and subcategory: format unknown, give up.
                _ => return None,
            };
            fields.push((if second_head { Field::Ignore } else { field }, 1));
        }
        let has = |f: fn(&Field) -> bool| fields.iter().any(|(k, _)| f(k));
        if !has(|k| *k == Field::X)
            || !has(|k| *k == Field::Y)
            || !has(|k| matches!(k, Field::Color(_) | Field::DynamicColor))
        {
            return None;
        }
        let size = fields.iter().map(|(_, n)| n).sum();
        Some(Self { fields, size })
    }

    fn point(&self, sample: &[u8]) -> Point {
        let (mut x, mut y, mut rgb) = (0.0, 0.0, [0f32; 3]);
        let (mut scale, mut shutter, mut nm) = (1.0, true, 0.0);
        let mut at = 0;
        for &(field, n) in &self.fields {
            // Values are left aligned: the first octet is the most significant.
            let bits = 8 * n.min(4) as u32;
            let val = sample[at..at + n.min(4)]
                .iter()
                .fold(0u64, |v, &o| (v << 8) | o as u64);
            let raw = (val << (32 - bits)) as u32;
            at += n;
            let unsigned = val as f32 / ((1u64 << bits) - 1) as f32;
            match field {
                Field::X => x = raw as i32 as f32 / 2f32.powi(31),
                Field::Y => y = raw as i32 as f32 / 2f32.powi(31),
                Field::Color(w) => add(&mut rgb, w, unsigned),
                Field::DynamicColor => add(&mut rgb, line(nm), unsigned),
                // 10 integer bits of wavelength at the top of the word.
                Field::WavelengthPrefix => nm = raw as f32 / (1u32 << 22) as f32,
                Field::Hint(format) => {
                    // CSCL divides all colours by 4 to the power of the scale.
                    scale = 4f32.powi((raw >> 30) as i32);
                    shutter = format == 0 || raw & (1 << 24) != 0;
                }
                Field::Ignore => {}
            }
        }
        let rgb = rgb.map(|c| (c / scale).min(1.0));
        if !shutter || rgb == [0.0; 3] {
            Point::blank(x, y)
        } else {
            Point::lit(x, y, rgb)
        }
    }
}

fn add(rgb: &mut [f32; 3], w: [f32; 3], v: f32) {
    for (c, w) in rgb.iter_mut().zip(w) {
        *c += w * v;
    }
}

/// The renderer's primary closest to a laser wavelength in nm.
// ponytail: nearest-primary bins; a real gamut transform if colour accuracy matters.
fn line(nm: f32) -> [f32; 3] {
    match nm {
        ..480.0 => [0.0, 0.0, 1.0],
        ..510.0 => [0.0, 1.0, 1.0],
        ..560.0 => [0.0, 1.0, 0.0],
        ..600.0 => [1.0, 1.0, 0.0],
        _ => [1.0, 0.0, 0.0],
    }
}

/// An open channel, routed to one laser service in one graphic mode.
struct Channel {
    /// Index of the service it draws on (service ID - 1).
    service: usize,
    mode: u8,
    /// Service data match bits; data chunks must carry the same ones.
    sdm: u8,
    decoder: Option<Decoder>,
    /// Discrete mode: the frame drawn every render frame until the next one.
    frame: Vec<Point>,
    once: bool,
}

/// A client link (one program) and its channels.
struct Client {
    addr: SocketAddr,
    /// When it last sent.
    last: Instant,
    new_link: bool,
    channels: Vec<Option<Channel>>,
}

/// Wave samples of one service.
#[derive(Default)]
struct Output {
    /// Received since the renderer last took them.
    pending: Vec<Point>,
    last: Option<Point>,
}

/// Receiver state shared by the network thread and the render loop. Each laser
/// service is drawn on its own; one client at a time may route channels to it.
pub struct Idn {
    clients: Vec<Client>,
    outputs: Vec<Output>,
}

impl Default for Idn {
    fn default() -> Self {
        Self::new(1)
    }
}

impl Idn {
    /// A unit with `services` laser services, IDs 1 to `services`.
    pub fn new(services: usize) -> Self {
        Self {
            clients: Vec::new(),
            outputs: (0..services).map(|_| Output::default()).collect(),
        }
    }

    /// Drops clients that sent nothing for the link timeout, with their channels;
    /// wave samples already received still draw.
    fn expire(&mut self, now: Instant) {
        self.clients.retain(|c| {
            let live = now.saturating_duration_since(c.last) <= LINK_TIMEOUT;
            if !live {
                log::info!("IDN client {} disconnected", c.addr);
            }
            live
        });
    }

    /// The client that routes a channel to `service`, if any.
    fn owner(&self, service: usize) -> Option<SocketAddr> {
        self.clients
            .iter()
            .find(|c| c.channels.iter().flatten().any(|ch| ch.service == service))
            .map(|c| c.addr)
    }

    fn close(&mut self, from: SocketAddr) {
        if let Some(i) = self.clients.iter().position(|c| c.addr == from) {
            log::info!("IDN client {from} disconnected");
            self.clients.remove(i);
        }
    }

    /// The discrete frames on `service`, then its wave samples received since the
    /// previous call, led by the last of those as a blanked move.
    pub fn take(&mut self, service: usize, now: Instant, out: &mut Vec<Point>) {
        self.expire(now);
        out.clear();
        for c in self
            .clients
            .iter_mut()
            .flat_map(|c| c.channels.iter_mut().flatten())
        {
            if c.service == service {
                out.extend_from_slice(&c.frame);
                if c.once {
                    c.frame.clear();
                }
            }
        }
        let Some(o) = self.outputs.get_mut(service) else {
            return;
        };
        if o.pending.is_empty() {
            return;
        }
        if let Some(prev) = o.last {
            out.push(Point {
                blank: true,
                ..prev
            });
        }
        out.append(&mut o.pending);
        o.last = out.last().copied();
    }

    /// Handles one datagram and returns the reply, if any.
    fn packet(&mut self, pkt: &[u8], from: SocketAddr, now: Instant) -> Option<Vec<u8>> {
        let (&[cmd, flags, s0, s1], payload) = pkt.split_first_chunk::<4>()?;
        self.expire(now);
        // Replies copy the client group (flags) and the sequence number.
        let reply = |cmd: u8, body: &[u8]| [&[cmd, flags, s0, s1][..], body].concat();
        match cmd {
            PING_REQUEST => Some(reply(PING_RESPONSE, payload)),
            GROUP_REQUEST => {
                // All client groups are always allowed: get succeeds, set is not supported.
                let op = if payload.get(1) == Some(&0x01) {
                    0x00
                } else {
                    0xFE
                };
                Some(reply(GROUP_RESPONSE, &[4, op, 0xFF, 0xFF]))
            }
            SCAN_REQUEST => {
                let mut r = [0u8; 40];
                r[0] = 40;
                r[1] = 0x01; // IDN-Hello version 0.1, as the draft requires
                // Occupied when other clients hold every service.
                let busy =
                    (0..self.outputs.len()).all(|s| self.owner(s).is_some_and(|a| a != from));
                r[2] = 0x01 | if busy { 0x10 } else { 0 }; // realtime, occupied
                r[4] = 7; // unit ID: length, category EUI-48, MAC
                r[5] = 0x01;
                r[6..12].copy_from_slice(&MAC);
                r[20..20 + NAME.len()].copy_from_slice(NAME);
                Some(reply(SCAN_RESPONSE, &r))
            }
            SERVICEMAP_REQUEST => {
                let n = self.outputs.len();
                let mut r = vec![0u8; 4 + 24 * n];
                r[..4].copy_from_slice(&[4, 24, 0, n as u8]); // sizes, no relays
                for (i, e) in r[4..].as_chunks_mut::<24>().0.iter_mut().enumerate() {
                    // ID, type, flags (the first is the default), no relay.
                    e[..4].copy_from_slice(&[i as u8 + 1, SERVICE_LASER, (i == 0) as u8, 0]);
                    let name = format!("BudgetLaser {}", i + 1);
                    let name = if n == 1 { "BudgetLaser" } else { &name };
                    e[4..4 + name.len()].copy_from_slice(name.as_bytes());
                }
                Some(reply(SERVICEMAP_RESPONSE, &r))
            }
            RT_MESSAGE | RT_MESSAGE_ACK | RT_CLOSE | RT_CLOSE_ACK => {
                let (result, closed) = self.stream(cmd, payload, from, now);
                if cmd != RT_MESSAGE_ACK && cmd != RT_CLOSE_ACK {
                    return None;
                }
                let mut r = [0u8; 12];
                r[0] = 12;
                r[1] = result;
                let client = self.clients.iter_mut().find(|c| c.addr == from);
                if result == ACK_OK && client.is_some_and(|c| std::mem::take(&mut c.new_link)) {
                    r[2..4].copy_from_slice(&EVENT_NEW.to_be_bytes());
                }
                if closed {
                    r[4..6].copy_from_slice(&EVENT_CHANNEL_CLOSED.to_be_bytes());
                }
                Some(reply(RT_ACKNOWLEDGE, &r))
            }
            RT_ABORT => {
                self.close(from);
                None
            }
            _ => None,
        }
    }

    /// An IDN-RT packet: (acknowledge result, whether the message's channel is closed;
    /// false when there was no message to process).
    fn stream(&mut self, cmd: u8, msg: &[u8], from: SocketAddr, now: Instant) -> (u8, bool) {
        let known = self.clients.iter().any(|c| c.addr == from);
        let closing = cmd == RT_CLOSE || cmd == RT_CLOSE_ACK;
        if msg.is_empty() && closing && !known {
            return (ACK_EMPTY_CLOSE, false);
        }
        // Malformed messages are dropped before they open a connection.
        let total = match msg {
            [] => 0,
            [a, b, ..] => u16::from_be_bytes([*a, *b]) as usize,
            _ => return (ACK_INVALID, false),
        };
        if !msg.is_empty() && (total < 8 || total > msg.len()) {
            return (ACK_INVALID, false);
        }
        if !known {
            log::info!("IDN client {from} connected");
            self.clients.push(Client {
                addr: from,
                last: now,
                new_link: true,
                channels: (0..64).map(|_| None).collect(),
            });
        }
        let c = self.clients.iter().position(|c| c.addr == from).unwrap();
        self.clients[c].last = now;
        let (result, closed) = if msg.is_empty() {
            (ACK_OK, false)
        } else {
            match self.message(c, &msg[..total]) {
                None => (ACK_OCCUPIED, true),
                Some(open) => (ACK_OK, !open),
            }
        };
        if closing {
            self.close(from);
        }
        if result == ACK_OCCUPIED && self.clients[c].channels.iter().all(Option::is_none) {
            // A client that only asked for a held service is no link of its own.
            self.clients.remove(c);
        }
        (result, closed)
    }

    /// One channel message of client `c`; returns whether its channel is open
    /// afterwards, or None when it routes to a service another client holds.
    fn message(&mut self, c: usize, msg: &[u8]) -> Option<bool> {
        let content = u16::from_be_bytes([msg[2], msg[3]]);
        if content & 0x8000 == 0 {
            return Some(false); // not a channel message
        }
        let cclf = content & 0x4000 != 0;
        let cnl = ((content >> 8) & 0x3F) as usize;
        let chunk = content as u8;
        let mut rest = &msg[8..]; // after the timestamp
        if chunk >= 0xC0 {
            // ponytail: frames split into IDN fragments are dropped; software relies on
            // IP fragmentation as the spec recommends. Reassemble if one shows up.
            return Some(self.clients[c].channels[cnl].is_some());
        }
        let mut close = false;
        if cclf {
            let Some((&[scwc, cfl, service, mode], tail)) = rest.split_first_chunk::<4>() else {
                return Some(false);
            };
            let Some((tags, tail)) = tail.split_at_checked(scwc as usize * 4) else {
                return Some(false);
            };
            rest = tail;
            close = cfl & 0x02 != 0;
            if cfl & 0x01 != 0 {
                // Routing: open (or re-route) the channel. Service 0 is the default
                // (the first); other services and modes route to nothing, so their
                // data is ignored.
                let index = (service as usize).saturating_sub(1);
                let laser =
                    index < self.outputs.len() && matches!(mode, MODE_CONTINUOUS | MODE_DISCRETE);
                if laser && self.owner(index).is_some_and(|a| a != self.clients[c].addr) {
                    return None;
                }
                let decoder = Decoder::new(tags).filter(|_| laser);
                let sdm = (cfl >> 4) & 0x03;
                let ch = &mut self.clients[c].channels[cnl];
                // A repeated configuration keeps the channel's frame.
                let same = |ch: &Channel| {
                    ch.service == index && ch.mode == mode && ch.sdm == sdm && ch.decoder == decoder
                };
                if !ch.as_ref().is_some_and(same) {
                    *ch = Some(Channel {
                        service: index,
                        mode,
                        sdm,
                        decoder,
                        frame: Vec::new(),
                        once: false,
                    });
                }
            }
        }
        if chunk != CHUNK_VOID {
            self.data(c, cnl, chunk, rest);
        }
        let ch = &mut self.clients[c].channels[cnl];
        if close {
            *ch = None;
        }
        Some(ch.is_some())
    }

    fn data(&mut self, c: usize, cnl: usize, chunk: u8, data: &[u8]) {
        let slot = &mut self.clients[c].channels[cnl];
        let Some(ch) = slot else {
            return; // closed channel
        };
        let Some((&[flags, ..], samples)) = data.split_first_chunk::<4>() else {
            return;
        };
        if (flags >> 4) & 0x03 != ch.sdm {
            // Data from another configuration: void the channel until the next one.
            *slot = None;
            return;
        }
        let Some(dec) = &ch.decoder else { return };
        if samples.len() % dec.size != 0 {
            return;
        }
        let points = samples.chunks_exact(dec.size).map(|s| dec.point(s));
        match (ch.mode, chunk) {
            (MODE_CONTINUOUS, CHUNK_WAVE) => {
                let pending = &mut self.outputs[ch.service].pending;
                pending.extend(points);
                if pending.len() > MAX_PENDING {
                    pending.drain(..pending.len() - MAX_PENDING);
                }
            }
            (MODE_DISCRETE, CHUNK_FRAME) => {
                ch.frame.clear();
                ch.frame.extend(points);
                // The first sample is the invisible start point.
                if let Some(p) = ch.frame.first_mut() {
                    *p = Point::blank(p.x, p.y);
                }
                ch.once = flags & 0x01 != 0;
            }
            _ => {} // chunk type not valid for the mode
        }
    }
}

/// Starts the IDN-Hello server thread on UDP 7255.
pub fn spawn(idn: Arc<Mutex<Idn>>) -> std::io::Result<()> {
    let sock = UdpSocket::bind(("0.0.0.0", PORT))?;
    log::info!("IDN listening on UDP {PORT}");
    std::thread::Builder::new()
        .name("idn".into())
        .spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                let (n, from) = match sock.recv_from(&mut buf) {
                    Ok(r) => r,
                    // Windows reports a reply to a closed client port on the next receive.
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
                    Err(e) => {
                        log::warn!("IDN receive: {e}");
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                };
                let reply = idn.lock().unwrap().packet(&buf[..n], from, Instant::now());
                if let Some(r) = reply
                    && let Err(e) = sock.send_to(&r, from)
                {
                    log::warn!("IDN reply to {from}: {e}");
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// X, Y 16 bit and R 638 nm, G 532 nm, B 460 nm 8 bit: the IDTF set most
    /// software sends, padded with a void tag to whole 32-bit words.
    const TAGS: [u16; 8] = [
        0x4200, 0x4010, 0x4210, 0x4010, 0x527E, 0x5214, 0x51CC, 0x0000,
    ];

    fn sample(x: i16, y: i16, rgb: [u8; 3]) -> Vec<u8> {
        [&x.to_be_bytes()[..], &y.to_be_bytes(), &rgb].concat()
    }

    /// An IDN-RT packet with one channel message on channel 3.
    fn packet(cmd: u8, seq: u16, mode: u8, chunk: u8, flags: u8, samples: &[Vec<u8>]) -> Vec<u8> {
        packet_cfl(cmd, seq, 0x01, mode, chunk, flags, samples)
    }

    /// As packet(), with the configuration flags (routing, close, SDM) spelled out.
    fn packet_cfl(
        cmd: u8,
        seq: u16,
        cfl: u8,
        mode: u8,
        chunk: u8,
        flags: u8,
        samples: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut m = vec![0, 0];
        let config = mode != 0;
        m.extend_from_slice(
            &(0x8000u16 | (config as u16) << 14 | 3 << 8 | chunk as u16).to_be_bytes(),
        );
        m.extend_from_slice(&1234u32.to_be_bytes()); // timestamp
        if config {
            m.extend_from_slice(&[(TAGS.len() / 2) as u8, cfl, 0, mode]);
            m.extend(TAGS.iter().flat_map(|t| t.to_be_bytes()));
        }
        m.extend_from_slice(&[flags, 0, 0x03, 0xE8]); // duration 1000 µs
        m.extend(samples.concat());
        let n = m.len() as u16;
        m[..2].copy_from_slice(&n.to_be_bytes());
        [&[cmd, 0, (seq >> 8) as u8, seq as u8][..], &m].concat()
    }

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, 2], port))
    }

    #[test]
    fn decoder_layout_and_values() {
        let tags: Vec<u8> = TAGS.iter().flat_map(|t| t.to_be_bytes()).collect();
        let d = Decoder::new(&tags).unwrap();
        assert_eq!(d.size, 7);
        let p = d.point(&sample(i16::MIN, 16384, [255, 0, 128]));
        assert_eq!((p.x, p.y), (-1.0, 0.5));
        assert_eq!((p.r, p.g), (1.0, 0.0));
        assert!((p.b - 128.0 / 255.0).abs() < 1e-6);
        assert!(d.point(&sample(0, 0, [0, 0, 0])).blank);

        // Hint format 1 with the shutter closed blanks a lit sample; colour scale 1
        // divides by 4. Yellow (577 nm) feeds red and green.
        let tags: Vec<u8> = [0x4101u16, 0x4200, 0x4210, 0x5241]
            .iter()
            .flat_map(|t| t.to_be_bytes())
            .collect();
        let d = Decoder::new(&tags).unwrap();
        assert!(d.point(&[0x00, 0, 0, 255]).blank);
        let p = d.point(&[0x41, 0, 0, 255]);
        assert!(!p.blank && p.r == 0.25 && p.g == 0.25 && p.b == 0.0);

        // A wavelength from the sample: 0x9F << 2 = 636 nm is red.
        let tags: Vec<u8> = [0x4200u16, 0x4210, 0x5C00, 0x5000]
            .iter()
            .flat_map(|t| t.to_be_bytes())
            .collect();
        let p = Decoder::new(&tags).unwrap().point(&[0, 0, 0x9F, 255]);
        assert_eq!((p.r, p.g, p.b), (1.0, 0.0, 0.0));

        // Void skips its suffix word (0x4200 here is a suffix, not X); NOP, Z and
        // intensity each take one octet; a second head after Break is skipped.
        let tags: Vec<u8> = [
            0x0001u16, 0x4200, 0x4000, 0x4200, 0x4210, 0x4220, 0x5C10, 0x527E, 0x1000, 0x4200,
            0x4010, 0x5214,
        ]
        .iter()
        .flat_map(|t| t.to_be_bytes())
        .collect();
        let d = Decoder::new(&tags).unwrap();
        assert_eq!(d.size, 9);
        let p = d.point(&[0xFF, 0x40, 0xC0, 0x7F, 0, 255, 0x80, 0, 255]);
        assert_eq!((p.x, p.y), (0.5, -0.5));
        assert_eq!((p.r, p.g), (1.0, 0.0)); // the second head's green is ignored
        // 0x4110 is an unknown hint format: an ignored octet, so the shutter stays open.
        let tags: Vec<u8> = [0x4110u16, 0x4200, 0x4210, 0x527E]
            .iter()
            .flat_map(|t| t.to_be_bytes())
            .collect();
        assert!(!Decoder::new(&tags).unwrap().point(&[0, 0, 0, 255]).blank);

        // No colour, or a tag of an undefined category: no decoder.
        assert!(Decoder::new(&[0x42, 0x00, 0x42, 0x10]).is_none());
        assert!(Decoder::new(&[0x42, 0x00, 0x42, 0x10, 0x52, 0x7E, 0x60, 0x00]).is_none());
    }

    #[test]
    fn hello_scan_service_map_and_ping() {
        let mut idn = Idn::default();
        let now = Instant::now();
        let r = idn
            .packet(&[SCAN_REQUEST, 0x05, 0x12, 0x34], addr(1), now)
            .unwrap();
        assert_eq!(r.len(), 44);
        assert_eq!(&r[..4], &[SCAN_RESPONSE, 0x05, 0x12, 0x34]);
        assert_eq!(&r[4..7], &[40, 0x01, 0x01]);
        assert_eq!(&r[8..16], &[7, 1, 0x02, 0x42, 0x4c, 0x49, 0x44, 0x4e]);
        assert_eq!(&r[24..35], b"BudgetLaser");

        let r = idn
            .packet(&[SERVICEMAP_REQUEST, 0, 0, 1], addr(1), now)
            .unwrap();
        assert_eq!(r.len(), 32);
        assert_eq!(&r[4..12], &[4, 24, 0, 1, 1, SERVICE_LASER, 0x01, 0]);

        let r = idn
            .packet(&[PING_REQUEST, 0, 0, 9, 1, 2, 3], addr(1), now)
            .unwrap();
        assert_eq!(r, [PING_RESPONSE, 0, 0, 9, 1, 2, 3]);
        assert!(idn.packet(&[PING_REQUEST, 0, 0], addr(1), now).is_none()); // short

        let r = idn
            .packet(&[GROUP_REQUEST, 0, 0, 2, 16, 0x01], addr(1), now)
            .unwrap();
        assert_eq!(r, [GROUP_RESPONSE, 0, 0, 2, 4, 0x00, 0xFF, 0xFF]);

        // While another client holds the only service, the scan reports it occupied.
        idn.packet(
            &packet(RT_MESSAGE, 1, MODE_CONTINUOUS, CHUNK_WAVE, 0, &[]),
            addr(2),
            now,
        );
        let r = idn.packet(&[SCAN_REQUEST, 0, 0, 3], addr(1), now).unwrap();
        assert_eq!(r[6], 0x11);
        let r = idn.packet(&[SCAN_REQUEST, 0, 0, 3], addr(2), now).unwrap();
        assert_eq!(r[6], 0x01);
    }

    #[test]
    fn services_each_have_their_own_client() {
        let mut idn = Idn::new(2);
        let t0 = Instant::now();
        let r = idn
            .packet(&[SERVICEMAP_REQUEST, 0, 0, 1], addr(1), t0)
            .unwrap();
        assert_eq!(r.len(), 4 + 4 + 2 * 24);
        assert_eq!(&r[4..8], &[4, 24, 0, 2]);
        assert_eq!(&r[8..12], &[1, SERVICE_LASER, 0x01, 0]);
        assert_eq!(&r[32..36], &[2, SERVICE_LASER, 0x00, 0]);
        assert_eq!(&r[36..49], b"BudgetLaser 2");

        // Byte 14 of a configured packet is the service ID.
        let on = |service: u8, seq: u16, x: i16| {
            let mut p = packet(
                RT_MESSAGE_ACK,
                seq,
                MODE_CONTINUOUS,
                CHUNK_WAVE,
                0,
                &[sample(x, 0, [255, 0, 0]), sample(x, 100, [255, 0, 0])],
            );
            p[14] = service;
            p
        };
        assert_eq!(idn.packet(&on(1, 1, 1000), addr(1), t0).unwrap()[5], ACK_OK);
        assert_eq!(idn.packet(&on(2, 1, 2000), addr(2), t0).unwrap()[5], ACK_OK);
        // Service 1 is held: a third client is refused there and not kept as a link.
        assert_eq!(
            idn.packet(&on(1, 1, 3000), addr(3), t0).unwrap()[5],
            ACK_OCCUPIED
        );
        assert_eq!(idn.clients.len(), 2);
        // Both held: the unit reports occupied to anyone else.
        let r = idn.packet(&[SCAN_REQUEST, 0, 0, 3], addr(3), t0).unwrap();
        assert_eq!(r[6], 0x11);

        let mut out = Vec::new();
        idn.take(0, t0, &mut out);
        assert_eq!(out[0].x, 1000.0 / 32768.0);
        idn.take(1, t0, &mut out);
        assert_eq!(out[0].x, 2000.0 / 32768.0);
        idn.take(2, t0, &mut out); // no such service
        assert!(out.is_empty());
        // One client times out; the other keeps its service.
        idn.packet(&on(2, 2, 2000), addr(2), t0 + Duration::from_millis(900));
        idn.take(0, t0 + Duration::from_millis(1500), &mut out);
        assert_eq!(idn.clients.len(), 1);
        assert_eq!(idn.clients[0].addr, addr(2));
    }

    #[test]
    fn wave_stream_one_client_and_timeout() {
        let mut idn = Idn::default();
        let t0 = Instant::now();
        let pts: Vec<_> = (0..20)
            .map(|i| sample(i * 1000, 0, [255, 255, 255]))
            .collect();

        // Configured wave message with an acknowledge request: new connection.
        let r = idn
            .packet(
                &packet(RT_MESSAGE_ACK, 7, MODE_CONTINUOUS, CHUNK_WAVE, 0, &pts),
                addr(1),
                t0,
            )
            .unwrap();
        assert_eq!(&r[..4], &[RT_ACKNOWLEDGE, 0, 0, 7]);
        assert_eq!(&r[4..10], &[12, ACK_OK, 0, EVENT_NEW as u8, 0, 0]);
        // A second client is refused while the first streams.
        let r = idn
            .packet(
                &packet(RT_MESSAGE_ACK, 1, MODE_CONTINUOUS, CHUNK_WAVE, 0, &pts),
                addr(2),
                t0,
            )
            .unwrap();
        assert_eq!(r[5], ACK_OCCUPIED);
        // Data without configuration on the open channel.
        assert!(
            idn.packet(&packet(RT_MESSAGE, 8, 0, CHUNK_WAVE, 0, &pts), addr(1), t0)
                .is_none()
        );

        let mut out = Vec::new();
        idn.take(0, t0, &mut out);
        assert_eq!(out.len(), 40);
        assert_eq!(out[39].x, 19000.0 / 32768.0);
        // Next take: the last sample leads as a blanked move.
        idn.packet(&packet(RT_MESSAGE, 9, 0, CHUNK_WAVE, 0, &pts), addr(1), t0);
        idn.take(0, t0, &mut out);
        assert_eq!(out.len(), 21);
        assert!(out[0].blank && !out[1].blank);

        // A wrong SCM voids the channel; its data no longer draws.
        idn.packet(
            &packet(RT_MESSAGE, 10, 0, CHUNK_WAVE, 0x10, &pts),
            addr(1),
            t0,
        );
        idn.packet(&packet(RT_MESSAGE, 11, 0, CHUNK_WAVE, 0, &pts), addr(1), t0);
        idn.take(0, t0, &mut out);
        assert!(out.is_empty());

        // After the link timeout, another client may connect.
        let later = t0 + Duration::from_millis(1500);
        let r = idn
            .packet(
                &packet(RT_MESSAGE_ACK, 1, MODE_CONTINUOUS, CHUNK_WAVE, 0, &pts),
                addr(2),
                later,
            )
            .unwrap();
        assert_eq!(&r[5..8], &[ACK_OK, 0, EVENT_NEW as u8]);
        // Malformed: total size larger than the datagram.
        let mut bad = packet(RT_MESSAGE_ACK, 2, 0, CHUNK_WAVE, 0, &pts);
        bad[4] = 0xFF;
        assert_eq!(idn.packet(&bad, addr(2), later).unwrap()[5], ACK_INVALID);
    }

    #[test]
    fn sdm_link_timeout_close_bit_abort_and_fragments() {
        let mut idn = Idn::default();
        let t0 = Instant::now();
        let pts: Vec<_> = (0..20).map(|_| sample(0, 0, [255, 0, 0])).collect();
        let mut out = Vec::new();

        // SDM 01 in the configuration: data must carry SCM 01, else the channel is void.
        idn.packet(
            &packet_cfl(RT_MESSAGE, 1, 0x11, MODE_CONTINUOUS, CHUNK_WAVE, 0x10, &pts),
            addr(1),
            t0,
        );
        idn.take(0, t0, &mut out);
        assert_eq!(out.len(), 20);
        let r = idn
            .packet(
                &packet(RT_MESSAGE_ACK, 2, 0, CHUNK_WAVE, 0x00, &pts),
                addr(1),
                t0,
            )
            .unwrap();
        assert_eq!(r[9], EVENT_CHANNEL_CLOSED as u8);

        // A fragment (sequel type 0xC0) is dropped; a truncated configuration too.
        idn.packet(
            &packet(RT_MESSAGE, 3, MODE_CONTINUOUS, CHUNK_WAVE, 0, &pts),
            addr(1),
            t0,
        );
        idn.take(0, t0, &mut out);
        idn.packet(&packet(RT_MESSAGE, 4, 0, 0xC0, 0, &pts), addr(1), t0);
        idn.take(0, t0, &mut out);
        assert!(out.is_empty());
        let mut short = packet(RT_MESSAGE, 5, MODE_CONTINUOUS, CHUNK_WAVE, 0, &[]);
        short[8 + 4] = 200; // SCWC far beyond the message
        idn.packet(&short, addr(1), t0);

        // Nothing for over a second: take() closes the link and blanks the output.
        idn.packet(
            &packet(RT_MESSAGE, 6, MODE_DISCRETE, CHUNK_FRAME, 0, &pts),
            addr(1),
            t0,
        );
        idn.take(0, t0, &mut out);
        assert_eq!(out.len(), 20);
        idn.take(0, t0 + Duration::from_millis(1500), &mut out);
        assert!(out.is_empty() && idn.clients.is_empty());

        // The close bit closes the channel after its data; abort drops the link.
        let t1 = t0 + Duration::from_secs(2);
        idn.packet(
            &packet_cfl(RT_MESSAGE, 7, 0x03, MODE_DISCRETE, CHUNK_FRAME, 0, &pts),
            addr(1),
            t1,
        );
        assert!(idn.clients[0].channels[3].is_none() && !idn.clients.is_empty());
        idn.packet(
            &packet(RT_MESSAGE, 8, MODE_DISCRETE, CHUNK_FRAME, 0, &pts),
            addr(1),
            t1,
        );
        idn.packet(&[RT_ABORT, 0, 0, 9], addr(2), t1); // not the client: ignored
        assert!(idn.clients[0].channels[3].is_some());
        idn.packet(&[RT_ABORT, 0, 0, 9], addr(1), t1);
        idn.take(0, t1, &mut out);
        assert!(out.is_empty() && idn.clients.is_empty());
    }

    #[test]
    fn discrete_frames_repeat_once_and_close() {
        let mut idn = Idn::default();
        let t0 = Instant::now();
        let frame: Vec<_> = (0..4).map(|i| sample(i * 1000, 0, [0, 255, 0])).collect();
        idn.packet(
            &packet(RT_MESSAGE, 1, MODE_DISCRETE, CHUNK_FRAME, 0, &frame),
            addr(1),
            t0,
        );
        let mut out = Vec::new();
        for _ in 0..2 {
            idn.take(0, t0, &mut out);
            assert_eq!(out.len(), 4); // repeated until the next frame
            assert!(out[0].blank && !out[1].blank && out[1].g == 1.0);
        }
        // Once: drawn a single time.
        idn.packet(
            &packet(RT_MESSAGE, 2, 0, CHUNK_FRAME, 0x01, &frame),
            addr(1),
            t0,
        );
        idn.take(0, t0, &mut out);
        assert_eq!(out.len(), 4);
        idn.take(0, t0, &mut out);
        assert!(out.is_empty());
        // A wave chunk is not valid in discrete mode.
        idn.packet(
            &packet(RT_MESSAGE, 3, 0, CHUNK_WAVE, 0, &frame),
            addr(1),
            t0,
        );
        idn.take(0, t0, &mut out);
        assert!(out.is_empty());

        // Close with acknowledge: channel closed, link released.
        idn.packet(
            &packet(RT_MESSAGE, 4, 0, CHUNK_FRAME, 0, &frame),
            addr(1),
            t0,
        );
        let r = idn.packet(&[RT_CLOSE_ACK, 0, 0, 5], addr(1), t0).unwrap();
        assert_eq!(r[5], ACK_OK);
        idn.take(0, t0, &mut out);
        assert!(out.is_empty() && idn.clients.is_empty());
        let r = idn.packet(&[RT_CLOSE_ACK, 0, 0, 6], addr(1), t0).unwrap();
        assert_eq!(r[5], ACK_EMPTY_CLOSE);
    }
}
