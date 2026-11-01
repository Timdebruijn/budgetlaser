//! Ether Dream DAC emulation: laser show software streams points over TCP 7765 and
//! finds the DAC through a UDP broadcast on 7654. Wire format and state rules follow
//! ether-dream.com/protocol.html; what the spec leaves open (broadcast port, buffer
//! size, rate limit, the 'u' and 'v' commands) comes from the j4cDAC firmware.

use crate::point::Point;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const TCP_PORT: u16 = 7765;
pub const BROADCAST_PORT: u16 = 7654;
/// The firmware buffers 1800 points and reports one less.
const BUFFER_CAPACITY: usize = 1799;
const MAX_POINT_RATE: u32 = 100_000;
const RATE_QUEUE: usize = 200;
const POINT_BYTES: usize = 18;
/// Locally administered, so it cannot clash with a real DAC.
const MAC: [u8; 6] = [0x02, 0x42, 0x4c, 0x41, 0x53, 0x52];

const ACK: u8 = b'a';
const NAK_FULL: u8 = b'F';
const NAK_INVALID: u8 = b'I';
// NAK_ESTOP ('!') answers a clear while the stop condition persists; an emulated
// DAC has no e-stop input or temperature sensor, so a clear always succeeds.

const IDLE: u8 = 0;
const PREPARED: u8 = 1;
const PLAYING: u8 = 2;

const FLAG_SHUTTER: u16 = 1 << 0;
const FLAG_UNDERFLOW: u16 = 1 << 1;
const FLAG_ESTOP: u16 = 1 << 2;
/// Light engine: e-stop from a packet or an invalid command.
const LE_ESTOP_PACKET: u16 = 1 << 0;
/// Control bit 15: take the next queued point rate when this point plays.
const RATE_CHANGE: u16 = 0x8000;

struct Queued {
    point: Point,
    rate_change: bool,
}

/// DAC state shared by the network thread and the render loop. Playback is
/// simulated lazily: every access first plays out the points that are due.
pub struct Dac {
    estop: bool,
    playback: u8,
    flags: u16,
    rate: u32,
    rates: VecDeque<u32>,
    buffer: VecDeque<Queued>,
    point_count: u32,
    clock: Instant,
    /// Seconds of playback time not yet spent on a point.
    debt: f64,
    /// Points played since the renderer last took them.
    played: Vec<Point>,
    last: Option<Point>,
}

impl Dac {
    pub fn new(now: Instant) -> Self {
        Self {
            estop: false,
            playback: IDLE,
            flags: 0,
            rate: 0,
            rates: VecDeque::new(),
            buffer: VecDeque::new(),
            point_count: 0,
            clock: now,
            debt: 0.0,
            played: Vec::new(),
            last: None,
        }
    }

    fn advance(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.clock).as_secs_f64();
        self.clock = now;
        if self.playback != PLAYING {
            return;
        }
        self.debt += dt;
        // The epsilon keeps float rounding from holding back a point that is due.
        while self.debt * self.rate as f64 >= 1.0 - 1e-9 {
            let Some(q) = self.buffer.pop_front() else {
                self.stop(FLAG_UNDERFLOW);
                return;
            };
            self.debt -= 1.0 / self.rate as f64;
            if q.rate_change
                && let Some(r) = self.rates.pop_front()
            {
                self.rate = r;
            }
            self.point_count = self.point_count.wrapping_add(1);
            self.played.push(q.point);
        }
        // ponytail: drops the oldest points if nothing renders for about a second.
        if self.played.len() > MAX_POINT_RATE as usize {
            self.played
                .drain(..self.played.len() - MAX_POINT_RATE as usize);
        }
    }

    fn stop(&mut self, why: u16) {
        self.playback = IDLE;
        self.flags = why;
        self.rate = 0;
        self.rates.clear();
        self.buffer.clear();
        self.point_count = 0;
        self.debt = 0.0;
        // `played` and `last` stay: points played before the stop still reach the screen.
    }

    /// Points played since the previous call, led by the last point of that call
    /// as a blanked move, so the first new point draws on from where the beam was.
    /// Empty while the DAC is not playing.
    pub fn take(&mut self, now: Instant, out: &mut Vec<Point>) {
        self.advance(now);
        out.clear();
        if self.played.is_empty() {
            return;
        }
        if let Some(prev) = self.last {
            out.push(Point {
                blank: true,
                ..prev
            });
        }
        out.append(&mut self.played);
        self.last = out.last().copied();
    }

    fn status(&self) -> [u8; 20] {
        let mut s = [0u8; 20];
        s[0] = 0; // protocol
        s[1] = if self.estop { 3 } else { 0 };
        s[2] = self.playback;
        s[3] = 0; // source: network
        let le_flags = if self.estop { LE_ESTOP_PACKET } else { 0 };
        let shutter = if self.playback == PLAYING {
            FLAG_SHUTTER
        } else {
            0
        };
        s[4..6].copy_from_slice(&le_flags.to_le_bytes());
        s[6..8].copy_from_slice(&(self.flags | shutter).to_le_bytes());
        s[10..12].copy_from_slice(&(self.buffer.len() as u16).to_le_bytes());
        s[12..16].copy_from_slice(&self.rate.to_le_bytes());
        s[16..20].copy_from_slice(&self.point_count.to_le_bytes());
        s
    }

    fn response(&self, resp: u8, cmd: u8) -> [u8; 22] {
        let mut r = [0u8; 22];
        r[0] = resp;
        r[1] = cmd;
        r[2..].copy_from_slice(&self.status());
        r
    }

    /// Handles one complete command; `body` is everything after the command byte.
    fn command(&mut self, cmd: u8, body: &[u8], now: Instant) -> u8 {
        self.advance(now);
        let rate = || u32::from_le_bytes(body[body.len() - 4..].try_into().unwrap());
        match cmd {
            b'p' if !self.estop && self.playback == IDLE => {
                self.stop(0);
                self.played.clear();
                self.last = None;
                self.playback = PREPARED;
                ACK
            }
            b'b' if self.playback == PREPARED
                && !self.buffer.is_empty()
                && (1..=MAX_POINT_RATE).contains(&rate()) =>
            {
                self.rate = rate();
                self.playback = PLAYING;
                self.debt = 0.0;
                ACK
            }
            // Update (firmware, not in the spec): set the rate now, as Begin does.
            b'u' if self.playback != IDLE && (1..=MAX_POINT_RATE).contains(&rate()) => {
                self.rate = rate();
                ACK
            }
            b'q' if self.playback != IDLE && (1..=MAX_POINT_RATE).contains(&rate()) => {
                if self.rates.len() >= RATE_QUEUE {
                    NAK_FULL
                } else {
                    self.rates.push_back(rate());
                    ACK
                }
            }
            b'd' if self.playback != IDLE => {
                let points = body[2..].as_chunks::<POINT_BYTES>().0;
                if self.buffer.len() + points.len() > BUFFER_CAPACITY {
                    NAK_FULL
                } else {
                    self.buffer.extend(points.iter().map(parse_point));
                    ACK
                }
            }
            b's' if self.playback != IDLE => {
                self.stop(0);
                ACK
            }
            b'c' if self.estop => {
                self.estop = false;
                ACK
            }
            b'?' => ACK,
            b'p' | b'b' | b'u' | b'q' | b'd' | b's' | b'c' => NAK_INVALID,
            // 0x00, 0xFF and every unknown command byte are an emergency stop.
            _ => {
                self.estop = true;
                self.stop(FLAG_ESTOP);
                ACK
            }
        }
    }
}

fn parse_point(b: &[u8; POINT_BYTES]) -> Queued {
    let u = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let xy = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]) as f32 / 32768.0;
    let (r, g, bl) = (u(6), u(8), u(10));
    let (x, y) = (xy(2), xy(4));
    // The protocol has no blanking bit; black is a blanked move. Intensity (i) and
    // the user channels are ignored.
    let point = if (r, g, bl) == (0, 0, 0) {
        Point::blank(x, y)
    } else {
        let c = |v: u16| v as f32 / 65535.0;
        Point::lit(x, y, [c(r), c(g), c(bl)])
    };
    Queued {
        point,
        rate_change: u(0) & RATE_CHANGE != 0,
    }
}

/// Starts the TCP server and the discovery broadcast.
pub fn spawn(dac: Arc<Mutex<Dac>>) -> std::io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", TCP_PORT))?;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_broadcast(true)?;
    log::info!("Ether Dream DAC listening on TCP {TCP_PORT}, broadcasting on UDP {BROADCAST_PORT}");
    let d = dac.clone();
    std::thread::Builder::new()
        .name("etherdream-bcast".into())
        .spawn(move || {
            let mut failing = false;
            loop {
                let pkt = broadcast(&d.lock().unwrap().status());
                match sock.send_to(&pkt, ("255.255.255.255", BROADCAST_PORT)) {
                    // macOS refuses until the user allows local network access.
                    Err(e) if !failing => {
                        log::warn!(
                            "Ether Dream broadcast failed, software cannot discover the DAC: {e}"
                        );
                        failing = true;
                    }
                    Err(_) => {}
                    Ok(_) if failing => {
                        log::info!("Ether Dream broadcast works again");
                        failing = false;
                    }
                    Ok(_) => {}
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })?;
    std::thread::Builder::new()
        .name("etherdream".into())
        .spawn(move || {
            // ponytail: one client at a time; a second one waits in the backlog
            // instead of being refused.
            for conn in listener.incoming() {
                let conn = match conn {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("Ether Dream accept: {e}");
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                };
                let peer = conn.peer_addr().map(|a| a.to_string()).unwrap_or_default();
                log::info!("Ether Dream client {peer} connected");
                match serve(conn, &dac) {
                    Ok(()) => log::info!("Ether Dream client {peer} disconnected"),
                    Err(e) => log::warn!("Ether Dream client {peer}: {e}"),
                }
            }
        })?;
    Ok(())
}

fn broadcast(status: &[u8; 20]) -> [u8; 36] {
    let mut b = [0u8; 36];
    b[..6].copy_from_slice(&MAC);
    b[6..8].copy_from_slice(&0u16.to_le_bytes()); // hardware revision
    b[8..10].copy_from_slice(&2u16.to_le_bytes()); // software revision, as the firmware
    b[10..12].copy_from_slice(&(BUFFER_CAPACITY as u16).to_le_bytes());
    b[12..16].copy_from_slice(&MAX_POINT_RATE.to_le_bytes());
    b[16..].copy_from_slice(status);
    b
}

fn serve(mut conn: TcpStream, dac: &Mutex<Dac>) -> std::io::Result<()> {
    conn.set_nodelay(true)?;
    // The DAC greets a new client with a status reply, as if it had sent a ping.
    let hello = dac.lock().unwrap().response(ACK, b'?');
    conn.write_all(&hello)?;
    let mut body = Vec::new();
    loop {
        let mut cmd = [0u8];
        match conn.read_exact(&mut cmd) {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            r => r?,
        }
        let cmd = cmd[0];
        if cmd == b'v' {
            // Version query (firmware, not in the spec): 32-byte NUL-padded string.
            let mut v = [0u8; 32];
            let s = concat!("BudgetLaser ", env!("CARGO_PKG_VERSION"));
            v[..s.len()].copy_from_slice(s.as_bytes());
            conn.write_all(&v)?;
            continue;
        }
        let len = match cmd {
            b'b' | b'u' => 6,
            b'q' => 4,
            b'd' => 2,
            _ => 0,
        };
        body.resize(len, 0);
        conn.read_exact(&mut body)?;
        if cmd == b'd' {
            let n = u16::from_le_bytes([body[0], body[1]]) as usize;
            body.resize(2 + n * POINT_BYTES, 0);
            conn.read_exact(&mut body[2..])?;
        }
        let reply = {
            let mut d = dac.lock().unwrap();
            let resp = d.command(cmd, &body, Instant::now());
            d.response(resp, cmd)
        };
        conn.write_all(&reply)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: i16, y: i16, rgb: u16, control: u16) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&control.to_le_bytes());
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
        for v in [rgb, rgb, rgb, 0, 0, 0] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p
    }

    fn data(points: &[Vec<u8>]) -> Vec<u8> {
        let mut b = (points.len() as u16).to_le_bytes().to_vec();
        b.extend(points.concat());
        b
    }

    fn begin(rate: u32) -> Vec<u8> {
        let mut b = 0u16.to_le_bytes().to_vec(); // low water mark, unused
        b.extend_from_slice(&rate.to_le_bytes());
        b
    }

    #[test]
    fn stream_plays_at_point_rate_then_underflows() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut d = Dac::new(t0);
        let pts: Vec<_> = (0..10).map(|i| point(i * 1000, 0, 65535, 0)).collect();

        assert_eq!(d.command(b'd', &data(&pts), t0), NAK_INVALID); // idle
        assert_eq!(d.command(b'b', &begin(1000), t0), NAK_INVALID); // not prepared
        assert_eq!(d.command(b'p', &[], t0), ACK);
        assert_eq!(d.command(b'b', &begin(1000), t0), NAK_INVALID); // empty buffer
        assert_eq!(d.command(b'd', &data(&pts), t0), ACK);
        assert_eq!(d.status()[10], 10); // buffer fullness
        assert_eq!(d.command(b'b', &begin(0), t0), NAK_INVALID);
        assert_eq!(d.command(b'b', &begin(1000), t0), ACK);
        assert_eq!(d.status()[2], PLAYING);

        // 1000 points/s: 4 ms plays 4 points; the next take leads with the last one blanked.
        let mut out = Vec::new();
        d.take(at(4), &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[3].x, 3000.0 / 32768.0);
        d.take(at(6), &mut out);
        assert_eq!(out.len(), 3);
        assert!(out[0].blank && out[0].x == out_x(3) && !out[1].blank);

        // The buffer runs dry: the last points still draw, then idle with the
        // underflow flag and nothing more to draw.
        d.take(at(20), &mut out);
        assert_eq!(out.len(), 5);
        assert_eq!(d.status()[2], IDLE);
        assert_eq!(
            u16::from_le_bytes([d.status()[6], d.status()[7]]),
            FLAG_UNDERFLOW
        );
        d.take(at(30), &mut out);
        assert!(out.is_empty());
        // Prepare clears the flag.
        assert_eq!(d.command(b'p', &[], at(30)), ACK);
        assert_eq!(d.status()[6], 0);
    }

    fn out_x(i: i16) -> f32 {
        (i * 1000) as f32 / 32768.0
    }

    #[test]
    fn buffer_full_rate_change_and_estop() {
        let t0 = Instant::now();
        let mut d = Dac::new(t0);
        assert_eq!(d.command(b'p', &[], t0), ACK);
        let many = vec![point(0, 0, 0, 0); BUFFER_CAPACITY];
        assert_eq!(d.command(b'd', &data(&many), t0), ACK);
        assert_eq!(d.command(b'd', &data(&many[..1]), t0), NAK_FULL); // all or nothing
        assert_eq!(d.command(b'd', &data(&[]), t0), ACK);
        assert!(d.buffer[0].point.blank);

        // A queued rate takes effect when a point with bit 15 plays.
        assert_eq!(d.command(b's', &[], t0), ACK);
        assert_eq!(d.command(b'p', &[], t0), ACK);
        let pts = [
            point(0, 0, 1, RATE_CHANGE),
            point(0, 0, 1, 0),
            point(0, 0, 1, 0),
        ];
        d.command(b'd', &data(&pts), t0);
        assert_eq!(d.command(b'q', &begin(2000)[2..], t0), ACK);
        d.command(b'b', &begin(1000), t0);
        let mut out = Vec::new();
        d.take(t0 + Duration::from_micros(1500), &mut out); // 1 point at 1000, then 2000/s
        assert_eq!((out.len(), d.rate), (2, 2000));

        // E-stop from 0xFF or any unknown byte: idle, prepare refused until cleared.
        assert_eq!(d.command(0xFF, &[], t0), ACK);
        assert_eq!((d.status()[1], d.status()[2]), (3, IDLE));
        assert_eq!(d.command(b'p', &[], t0), NAK_INVALID);
        assert_eq!(d.command(b'c', &[], t0), ACK);
        assert_eq!(d.command(b'c', &[], t0), NAK_INVALID);
        assert_eq!(d.command(b'p', &[], t0), ACK);
        assert_eq!(d.command(b'x', &[], t0), ACK);
        assert_eq!(d.status()[1], 3);
    }

    #[test]
    fn wire_layout() {
        let t0 = Instant::now();
        let mut d = Dac::new(t0);
        let b = broadcast(&d.status());
        assert_eq!((&b[..6], b[8]), (&MAC[..], 2));
        assert_eq!(u16::from_le_bytes([b[10], b[11]]), 1799);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 100_000);

        d.command(b'p', &[], t0);
        d.command(b'd', &data(&vec![point(0, 0, 1, 0); 5]), t0);
        d.command(b'b', &begin(1000), t0);
        d.advance(t0 + Duration::from_millis(2));
        let r = d.response(ACK, b'?');
        assert_eq!((r.len(), r[0], r[1]), (22, b'a', b'?'));
        let s = &r[2..];
        assert_eq!(
            (s[2], u16::from_le_bytes([s[6], s[7]])),
            (PLAYING, FLAG_SHUTTER)
        );
        assert_eq!(u16::from_le_bytes([s[10], s[11]]), 3); // buffered
        assert_eq!(u32::from_le_bytes(s[12..16].try_into().unwrap()), 1000); // rate
        assert_eq!(u32::from_le_bytes(s[16..20].try_into().unwrap()), 2); // played

        let mut p = point(-32768, 16384, 0, 0);
        p[6..12].copy_from_slice(&[0xff, 0xff, 0, 0x80, 0, 0]); // r = 1, g = 0.5, b = 0
        let q = parse_point(&p.try_into().unwrap());
        assert_eq!((q.point.x, q.point.y, q.point.blank), (-1.0, 0.5, false));
        assert_eq!((q.point.r, q.point.b), (1.0, 0.0));
        assert!((q.point.g - 0.5).abs() < 1e-4);
    }

    #[test]
    fn tcp_framing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let dac = Arc::new(Mutex::new(Dac::new(Instant::now())));
        let d = dac.clone();
        std::thread::spawn(move || serve(listener.accept().unwrap().0, &d));
        let mut c = TcpStream::connect(addr).unwrap();
        let reply = |c: &mut TcpStream, send: &[u8]| {
            c.write_all(send).unwrap();
            let mut r = [0u8; 22];
            c.read_exact(&mut r).unwrap();
            (r[0], r[1])
        };
        assert_eq!(reply(&mut c, &[]), (ACK, b'?')); // greeting
        assert_eq!(reply(&mut c, b"p"), (ACK, b'p'));
        let pts = vec![point(0, 0, 1, 0); 3];
        assert_eq!(
            reply(&mut c, &[&b"d"[..], &data(&pts)].concat()),
            (ACK, b'd')
        );
        assert_eq!(
            reply(&mut c, &[&b"b"[..], &begin(1000)].concat()),
            (ACK, b'b')
        );
        assert_eq!(
            reply(&mut c, &[&b"u"[..], &begin(2000)].concat()),
            (ACK, b'u')
        );
        assert_eq!(
            reply(&mut c, &[&b"q"[..], &0u32.to_le_bytes()].concat()),
            (NAK_INVALID, b'q')
        );
        c.write_all(b"v").unwrap();
        let mut v = [0u8; 32];
        c.read_exact(&mut v).unwrap();
        assert!(v.starts_with(b"BudgetLaser "));
        assert_eq!(reply(&mut c, b"?"), (ACK, b'?'));
        assert_eq!(dac.lock().unwrap().rate, 2000);
        assert_eq!(reply(&mut c, b"x"), (ACK, b'x')); // unknown byte: e-stop
        assert!(dac.lock().unwrap().estop);
    }
}
