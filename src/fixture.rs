//! Fixture profile v0.1: 13 channels from a start address, plus smoothing.

use std::f32::consts::TAU;

pub const CHANNELS: usize = 13;
pub const MAX_BEAMS: u32 = 64;
const MAX_SPIN_RAD_S: f32 = 2.0 * TAU; // 2 rev/s at full speed
const MAX_STROBE_HZ: f32 = 25.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Dot,
    Line,
    Circle,
    Fan,
    Tunnel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rotation {
    /// Absolute angle in radians, counter-clockwise.
    Static(f32),
    /// Radians per second, positive = counter-clockwise.
    Spin(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub on: bool,
    pub pattern: Option<Pattern>,
    /// 0..1 of the scan field.
    pub size: f32,
    pub rotation: Rotation,
    /// -1..1
    pub pan: f32,
    pub tilt: f32,
    pub rgb: [f32; 3],
    /// 0..1 linear DMX value; gamma applied in the renderer.
    pub dimmer: f32,
    /// 0 = open, else Hz.
    pub strobe_hz: f32,
    pub beams: u32,
    /// 0..1
    pub anim: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            on: false,
            pattern: None,
            size: 0.0,
            rotation: Rotation::Static(0.0),
            pan: 0.0,
            tilt: 0.0,
            rgb: [0.0; 3],
            dimmer: 0.0,
            strobe_hz: 0.0,
            beams: 1,
            anim: 0.0,
        }
    }
}

fn unit(v: u8) -> f32 {
    v as f32 / 255.0
}

/// Decode the 13 channels starting at `ch` (slice of at least 13 bytes).
pub fn decode(ch: &[u8]) -> Params {
    let pattern = match ch[1] {
        0..=19 => Some(Pattern::Dot),
        20..=39 => Some(Pattern::Line),
        40..=59 => Some(Pattern::Circle),
        60..=79 => Some(Pattern::Fan),
        80..=99 => Some(Pattern::Tunnel),
        _ => None, // reserved
    };
    let rotation = match ch[3] {
        v @ 0..=127 => Rotation::Static(v as f32 / 127.0 * TAU),
        v @ 128..=191 => Rotation::Spin(-((v - 128) as f32 + 1.0) / 64.0 * MAX_SPIN_RAD_S),
        v => Rotation::Spin(((v - 192) as f32 + 1.0) / 64.0 * MAX_SPIN_RAD_S),
    };
    Params {
        on: ch[0] >= 10,
        pattern,
        size: 0.02 + 0.98 * unit(ch[2]),
        rotation,
        pan: unit(ch[4]) * 2.0 - 1.0,
        tilt: unit(ch[5]) * 2.0 - 1.0,
        rgb: [unit(ch[6]), unit(ch[7]), unit(ch[8])],
        dimmer: unit(ch[9]),
        strobe_hz: if ch[10] < 10 {
            0.0
        } else {
            1.0 + (ch[10] - 10) as f32 / 245.0 * (MAX_STROBE_HZ - 1.0)
        },
        beams: 1 + (unit(ch[11]) * (MAX_BEAMS - 1) as f32).round() as u32,
        anim: unit(ch[12]),
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a) % TAU;
    if d > TAU / 2.0 {
        d -= TAU;
    } else if d < -TAU / 2.0 {
        d += TAU;
    }
    a + d * t
}

/// Interpolates continuous parameters between the last two DMX frames.
/// Costs one DMX frame of latency; discrete channels jump immediately.
pub struct Smoother {
    prev: Params,
    cur: Params,
    t_cur: f64,
    interval: f64,
}

impl Smoother {
    pub fn new() -> Self {
        Self {
            prev: Params::default(),
            cur: Params::default(),
            t_cur: 0.0,
            interval: 1.0 / 44.0,
        }
    }

    pub fn push(&mut self, p: Params, now: f64) {
        self.prev = self.sample(now);
        self.cur = p;
        // ponytail: clamp guards against the first frame and stalls; no jitter filter.
        self.interval = (now - self.t_cur).clamp(0.005, 0.1);
        self.t_cur = now;
    }

    pub fn sample(&self, now: f64) -> Params {
        let t = (((now - self.t_cur) / self.interval).clamp(0.0, 1.0)) as f32;
        let (a, b) = (&self.prev, &self.cur);
        let rotation = match (a.rotation, b.rotation) {
            (Rotation::Static(x), Rotation::Static(y)) => Rotation::Static(lerp_angle(x, y, t)),
            _ => b.rotation,
        };
        Params {
            size: lerp(a.size, b.size, t),
            pan: lerp(a.pan, b.pan, t),
            tilt: lerp(a.tilt, b.tilt, t),
            rotation,
            ..*b
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_channels() {
        let mut ch = [0u8; CHANNELS];
        ch[0] = 255; // on
        ch[1] = 70; // fan
        ch[2] = 255; // full size
        ch[3] = 255; // ccw fastest
        ch[4] = 0; // left
        ch[5] = 255; // top
        ch[6] = 255;
        ch[9] = 128;
        ch[10] = 255; // fastest strobe
        ch[11] = 255; // 64 beams
        let p = decode(&ch);
        assert!(p.on);
        assert_eq!(p.pattern, Some(Pattern::Fan));
        assert_eq!(p.size, 1.0);
        assert_eq!(p.rotation, Rotation::Spin(MAX_SPIN_RAD_S));
        assert_eq!((p.pan, p.tilt), (-1.0, 1.0));
        assert_eq!(p.strobe_hz, MAX_STROBE_HZ);
        assert_eq!(p.beams, 64);
        ch[3] = 128;
        assert_eq!(decode(&ch).rotation, Rotation::Spin(-MAX_SPIN_RAD_S / 64.0));
        ch[0] = 9;
        ch[1] = 100;
        ch[10] = 0;
        ch[11] = 0;
        let p = decode(&ch);
        assert!(!p.on && p.pattern.is_none() && p.strobe_hz == 0.0 && p.beams == 1);
    }

    #[test]
    fn smoothing_interpolates_and_wraps() {
        let mut s = Smoother::new();
        let mut a = Params {
            pan: -1.0,
            rotation: Rotation::Static(0.1),
            ..Params::default()
        };
        s.push(a, 1.0);
        a.pan = 1.0;
        a.rotation = Rotation::Static(TAU - 0.1);
        s.push(a, 1.1);
        let mid = s.sample(1.15);
        assert!((mid.pan - 0.0).abs() < 1e-5);
        match mid.rotation {
            Rotation::Static(x) => assert!(x.abs() < 1e-4, "shortest path through 0, got {x}"),
            _ => panic!(),
        }
        assert_eq!(s.sample(5.0).pan, 1.0);
    }
}
