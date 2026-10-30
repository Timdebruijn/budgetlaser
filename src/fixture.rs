//! Fixture profiles (13 and 17 channels, each with or without a beam width channel), plus
//! smoothing.

use serde::Deserialize;
use std::f32::consts::TAU;

/// Channel layout. Every personality keeps 1-13 identical, so console programming for
/// the 13-channel mode keeps working. 17ch appends fine channels 14-17 for size,
/// rotation, pan and tilt; 14ch and 18ch append beam width to 13ch and 17ch.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Personality {
    #[default]
    #[serde(rename = "13ch")]
    Standard,
    #[serde(rename = "14ch")]
    Width,
    #[serde(rename = "17ch")]
    Fine,
    #[serde(rename = "18ch")]
    FineWidth,
}

impl Personality {
    pub fn footprint(self) -> usize {
        match self {
            Personality::Standard => 13,
            Personality::Width => 14,
            Personality::Fine => 17,
            Personality::FineWidth => 18,
        }
    }
}

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
    Grid,
    Twinkle,
    Wave,
    Polygon,
    /// Plays an .ild file from the ILDA directory.
    Ilda,
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
    /// Raw channel 12: the ILDA file index.
    pub file: u8,
    /// 0..1
    pub anim: f32,
    /// Multiplier on `output.line_width_px`.
    pub width: f32,
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
            file: 0,
            anim: 0.0,
            width: 1.0,
        }
    }
}

fn unit(v: u8) -> f32 {
    v as f32 / 255.0
}

/// 16-bit value as 0..1: the fine byte is a fraction of one coarse step, so
/// fine = 0 gives exactly the 8-bit value.
fn unit16(coarse: u8, fine: u8) -> f32 {
    ((coarse as f32 + fine as f32 / 256.0) / 255.0).min(1.0)
}

/// Beam width channel: 0 keeps the configured line width, 1-255 scale it
/// exponentially from 0.5x to 8x (about 64 = 1x).
fn beam_width(v: u8) -> f32 {
    match v {
        0 => 1.0,
        v => 0.5 * 16f32.powf((v - 1) as f32 / 254.0),
    }
}

/// Decode one fixture; `ch` is its footprint (13, 14, 17 or 18 bytes).
pub fn decode(ch: &[u8]) -> Params {
    // Fine channels 14-17: size, rotation, pan, tilt. Zero without them.
    let fine = |i: usize| if ch.len() >= 17 { ch[13 + i] } else { 0 };
    // Beam width is the last channel of the 14- and 18-channel modes.
    let width = match ch.len() {
        14 | 18 => beam_width(ch[ch.len() - 1]),
        _ => 1.0,
    };
    let pattern = match ch[1] {
        0..=19 => Some(Pattern::Dot),
        20..=39 => Some(Pattern::Line),
        40..=59 => Some(Pattern::Circle),
        60..=79 => Some(Pattern::Fan),
        80..=99 => Some(Pattern::Tunnel),
        100..=119 => Some(Pattern::Grid),
        120..=139 => Some(Pattern::Twinkle),
        140..=159 => Some(Pattern::Wave),
        160..=179 => Some(Pattern::Polygon),
        180..=199 => Some(Pattern::Ilda),
        _ => None, // reserved
    };
    let rotation = match ch[3] {
        v @ 0..=127 => {
            Rotation::Static(((v as f32 + fine(1) as f32 / 256.0) / 127.0).min(1.0) * TAU)
        }
        v @ 128..=191 => Rotation::Spin(-((v - 128) as f32 + 1.0) / 64.0 * MAX_SPIN_RAD_S),
        v => Rotation::Spin(((v - 192) as f32 + 1.0) / 64.0 * MAX_SPIN_RAD_S),
    };
    Params {
        on: ch[0] >= 10,
        pattern,
        size: 0.02 + 0.98 * unit16(ch[2], fine(0)),
        rotation,
        pan: unit16(ch[4], fine(2)) * 2.0 - 1.0,
        tilt: unit16(ch[5], fine(3)) * 2.0 - 1.0,
        rgb: [unit(ch[6]), unit(ch[7]), unit(ch[8])],
        dimmer: unit(ch[9]),
        strobe_hz: if ch[10] < 10 {
            0.0
        } else {
            1.0 + (ch[10] - 10) as f32 / 245.0 * (MAX_STROBE_HZ - 1.0)
        },
        beams: 1 + (unit(ch[11]) * (MAX_BEAMS - 1) as f32).round() as u32,
        file: ch[11],
        anim: unit(ch[12]),
        width,
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
            width: lerp(a.width, b.width, t),
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
        let mut ch = [0u8; 13];
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
        for (v, want) in [
            (110, Some(Pattern::Grid)),
            (120, Some(Pattern::Twinkle)),
            (159, Some(Pattern::Wave)),
            (179, Some(Pattern::Polygon)),
            (180, Some(Pattern::Ilda)),
            (199, Some(Pattern::Ilda)),
            (200, None),
        ] {
            let mut c = ch;
            c[1] = v;
            assert_eq!(decode(&c).pattern, want, "channel 2 = {v}");
        }
        assert_eq!(p.size, 1.0);
        assert_eq!(p.rotation, Rotation::Spin(MAX_SPIN_RAD_S));
        assert_eq!((p.pan, p.tilt), (-1.0, 1.0));
        assert_eq!(p.strobe_hz, MAX_STROBE_HZ);
        assert_eq!((p.beams, p.file), (64, 255));
        ch[3] = 128;
        assert_eq!(decode(&ch).rotation, Rotation::Spin(-MAX_SPIN_RAD_S / 64.0));
        ch[0] = 9;
        ch[1] = 200; // reserved
        ch[10] = 0;
        ch[11] = 0;
        let p = decode(&ch);
        assert!(!p.on && p.pattern.is_none() && p.strobe_hz == 0.0 && p.beams == 1);
    }

    #[test]
    fn fine_channels() {
        let mut ch = [0u8; 17];
        ch[4] = 128; // pan coarse
        ch[15] = 128; // pan fine: half a coarse step
        ch[3] = 10; // static angle
        ch[14] = 255;
        let coarse_only = decode(&ch[..13]);
        let fine = decode(&ch);
        let step = 2.0 / 255.0; // one coarse pan step in -1..1
        assert!((fine.pan - coarse_only.pan - step / 2.0).abs() < 1e-6);
        let (Rotation::Static(a), Rotation::Static(b)) = (coarse_only.rotation, fine.rotation)
        else {
            panic!()
        };
        assert!(b > a && b - a < TAU / 127.0);
        // 13-channel decode equals 17-channel decode with fine = 0.
        ch[13..].fill(0);
        assert_eq!(decode(&ch), decode(&ch[..13]));
        // Full scale never exceeds 1.
        ch[2] = 255;
        ch[13] = 255;
        assert_eq!(decode(&ch).size, 1.0);
    }

    #[test]
    fn beam_width_channel() {
        let mut ch = [0u8; 18];
        ch[13] = 7; // size fine in 18ch, beam width in 14ch
        assert_eq!(decode(&ch[..13]).width, 1.0);
        assert_eq!(decode(&ch[..17]).width, 1.0);
        let w14 = decode(&ch[..14]);
        assert_eq!(
            w14.size,
            decode(&ch[..13]).size,
            "14ch has no fine channels"
        );
        assert!((w14.width - beam_width(7)).abs() < 1e-6);
        assert_eq!(decode(&ch).width, 1.0, "18ch width 0 keeps the config");
        for (v, want) in [(1, 0.5), (255, 8.0)] {
            ch[17] = v;
            assert!((decode(&ch).width - want).abs() < 1e-4, "ch 18 = {v}");
        }
        assert!((beam_width(64) - 1.0).abs() < 0.02);
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
