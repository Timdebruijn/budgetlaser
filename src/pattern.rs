//! Pattern generator: unit-space shapes, then scale → rotate → translate → colour.

use crate::fixture::{Params, Pattern, Rotation};
use crate::point::{Frame, Point};
use std::f32::consts::{PI, TAU};

const CIRCLE_POINTS: usize = 96;
const WAVE_POINTS: usize = 128;
const MAX_WAVE_CYCLES: f32 = 8.0;
const MAX_POLYGON_SIDES: u32 = 12;
const ANIM_MAX_HZ: f32 = 2.0;

/// Phases accumulated per render frame so speed channels are smooth at any DMX rate.
#[derive(Default)]
pub struct Phases {
    pub angle: f32,
    pub anim: f32,
    /// Completed animation cycles; gives twinkle dots a new spot each cycle.
    pub anim_cycles: u32,
    pub strobe: f32,
}

impl Phases {
    pub fn tick(&mut self, p: &Params, dt: f32) {
        match p.rotation {
            Rotation::Static(a) => self.angle = a,
            Rotation::Spin(w) => self.angle = (self.angle + w * dt) % TAU,
        }
        let anim = self.anim + p.anim * ANIM_MAX_HZ * dt;
        self.anim_cycles = self.anim_cycles.wrapping_add(anim as u32);
        self.anim = anim.fract();
        self.strobe = if p.strobe_hz > 0.0 {
            (self.strobe + p.strobe_hz * dt) % 1.0
        } else {
            0.0
        };
    }
}

/// Deterministic pseudo-random value in -1..1 for (index, generation, axis).
fn hash_signed(i: u32, generation: u32, axis: u32) -> f32 {
    // Seeded so that (0, 0, 0) does not hash to 0, which would pin dot 0 to the edge.
    let mut h = 0x6A09_E667
        ^ i.wrapping_mul(0x9E37_79B1)
        ^ generation.wrapping_mul(0x85EB_CA77)
        ^ axis.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h as f32 / u32::MAX as f32 * 2.0 - 1.0
}

fn unit_shape(pattern: Pattern, beams: u32, ph: &Phases, animating: bool, out: &mut Vec<Point>) {
    let anim = ph.anim;
    let w = if animating { (anim * TAU).sin() } else { 0.0 }; // -1..1
    // Shared "breathing" for closed shapes: radius pulses between 0.7 and 1.
    let breathe = 1.0 - 0.3 * (0.5 + 0.5 * w) * animating as u8 as f32;
    let white = [1.0; 3];
    match pattern {
        Pattern::Dot => out.push(Point::lit(0.0, 0.0, white)),
        Pattern::Line => {
            // Sweeps ±45° around the centre when animating.
            let a = w * TAU / 8.0;
            let (s, c) = a.sin_cos();
            out.push(Point::lit(-c, -s, white));
            out.push(Point::lit(c, s, white));
        }
        Pattern::Circle => {
            let r = breathe;
            for i in 0..=CIRCLE_POINTS {
                let a = i as f32 / CIRCLE_POINTS as f32 * TAU;
                out.push(Point::lit(r * a.cos(), r * a.sin(), white));
            }
        }
        Pattern::Fan => {
            let spread = if animating { 0.5 + 0.5 * w } else { 1.0 };
            for i in 0..beams {
                let x = if beams == 1 {
                    0.0
                } else {
                    (i as f32 / (beams - 1) as f32 * 2.0 - 1.0) * spread
                };
                out.push(Point::blank(x, 0.0));
                out.push(Point::lit(x, 0.0, white));
            }
        }
        Pattern::Tunnel => {
            let r = breathe;
            for i in 0..beams {
                let a = i as f32 / beams as f32 * TAU;
                let (s, c) = a.sin_cos();
                out.push(Point::blank(r * c, r * s));
                out.push(Point::lit(r * c, r * s, white));
            }
        }
        Pattern::Grid => {
            // Square lattice of beams; channel 12 sets roughly how many (1x1 .. 8x8).
            let side = (beams as f32).sqrt().round().max(1.0) as u32;
            for row in 0..side {
                for col in 0..side {
                    let at = |k: u32| {
                        if side == 1 {
                            0.0
                        } else {
                            (k as f32 / (side - 1) as f32 * 2.0 - 1.0) * breathe
                        }
                    };
                    out.push(Point::blank(at(col), at(row)));
                    out.push(Point::lit(at(col), at(row), white));
                }
            }
        }
        Pattern::Twinkle => {
            // Channel 12 = number of dots. While animating each dot fades in and out
            // once per cycle, staggered, and comes back at a new random spot.
            for i in 0..beams {
                let (generation, level) = if animating {
                    let s = anim + i as f32 / beams as f32;
                    (
                        ph.anim_cycles.wrapping_add(s as u32),
                        (s.fract() * PI).sin(),
                    )
                } else {
                    (0, 1.0)
                };
                let (x, y) = (hash_signed(i, generation, 0), hash_signed(i, generation, 1));
                out.push(Point::blank(x, y));
                out.push(Point::lit(x, y, [level; 3]));
            }
        }
        Pattern::Wave => {
            // Sine across the scan field; channel 12 sets 1..8 cycles, animation makes it travel.
            let cycles = 1.0 + ((beams - 1) as f32 / 63.0 * (MAX_WAVE_CYCLES - 1.0)).round();
            let travel = if animating { anim * TAU } else { 0.0 };
            for i in 0..=WAVE_POINTS {
                let u = i as f32 / WAVE_POINTS as f32;
                let y = 0.5 * (u * cycles * TAU - travel).sin();
                out.push(Point::lit(u * 2.0 - 1.0, y, white));
            }
        }
        Pattern::Polygon => {
            // Closed outline, point up; channel 12 spreads 3..12 sides over its range.
            let sides =
                3 + ((beams - 1) as f32 / 63.0 * (MAX_POLYGON_SIDES - 3) as f32).round() as u32;
            for i in 0..=sides {
                let a = PI / 2.0 + i as f32 / sides as f32 * TAU;
                out.push(Point::lit(breathe * a.cos(), breathe * a.sin(), white));
            }
        }
    }
}

/// Builds the point-stream frame for one render frame.
pub fn build(p: &Params, ph: &Phases, dimmer_gamma: f32, frame: &mut Frame) {
    frame.points.clear();
    let Some(pattern) = p.pattern else { return };
    let strobe_open = p.strobe_hz == 0.0 || ph.strobe < 0.5;
    if !p.on || !strobe_open {
        return;
    }
    unit_shape(pattern, p.beams, ph, p.anim > 0.0, &mut frame.points);
    let gain = p.dimmer.powf(dimmer_gamma);
    let rgb = [p.rgb[0] * gain, p.rgb[1] * gain, p.rgb[2] * gain];
    let (s, c) = ph.angle.sin_cos();
    for pt in &mut frame.points {
        let (x, y) = (pt.x * p.size, pt.y * p.size);
        pt.x = x * c - y * s + p.pan;
        pt.y = x * s + y * c + p.tilt;
        if !pt.blank {
            // Shapes emit per-point intensity (white = full); colour and dimmer scale it.
            pt.r *= rgb[0];
            pt.g *= rgb[1];
            pt.b *= rgb[2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_chain() {
        let p = Params {
            on: true,
            pattern: Some(Pattern::Line),
            size: 0.5,
            rotation: Rotation::Static(TAU / 4.0),
            pan: 0.25,
            tilt: -0.25,
            rgb: [1.0, 0.5, 0.0],
            dimmer: 1.0,
            ..Params::default()
        };
        let mut ph = Phases::default();
        ph.tick(&p, 0.016);
        let mut f = Frame::default();
        build(&p, &ph, 2.2, &mut f);
        assert_eq!(f.points.len(), 2);
        // (-1,0) scaled 0.5, rotated 90° ccw -> (0,-0.5), shifted -> (0.25,-0.75)
        let a = f.points[0];
        assert!(
            (a.x - 0.25).abs() < 1e-5 && (a.y + 0.75).abs() < 1e-5,
            "{a:?}"
        );
        assert_eq!((a.r, a.g, a.b), (1.0, 0.5, 0.0));

        let off = Params { on: false, ..p };
        build(&off, &ph, 2.2, &mut f);
        assert!(f.points.is_empty());

        let tunnel = Params {
            pattern: Some(Pattern::Tunnel),
            beams: 8,
            ..p
        };
        build(&tunnel, &ph, 2.2, &mut f);
        assert_eq!(f.points.iter().filter(|q| !q.blank).count(), 8);
    }

    fn shape(pattern: Pattern, beams: u32, ph: &Phases, animating: bool) -> Vec<Point> {
        let mut out = Vec::new();
        unit_shape(pattern, beams, ph, animating, &mut out);
        out
    }

    fn lit(points: &[Point]) -> Vec<Point> {
        points.iter().copied().filter(|p| !p.blank).collect()
    }

    #[test]
    fn new_patterns() {
        let still = Phases::default();
        let in_field = |p: &Point| p.x.abs() <= 1.0 && p.y.abs() <= 1.0;

        // Grid: nearest square of the beam count, corners on the edge of the field.
        assert_eq!(lit(&shape(Pattern::Grid, 64, &still, false)).len(), 64);
        assert_eq!(lit(&shape(Pattern::Grid, 10, &still, false)).len(), 9);
        assert_eq!(lit(&shape(Pattern::Grid, 1, &still, false))[0].x, 0.0);
        assert_eq!(lit(&shape(Pattern::Grid, 4, &still, false))[0].x, -1.0);

        // Polygon: closed outline, 3..12 sides spread over the beam range.
        let tri = shape(Pattern::Polygon, 1, &still, false);
        assert_eq!(tri.len(), 4);
        assert!((tri[0].x - tri[3].x).abs() < 1e-5 && (tri[0].y - tri[3].y).abs() < 1e-5);
        assert_eq!(shape(Pattern::Polygon, 64, &still, false).len(), 13);
        assert_eq!(shape(Pattern::Polygon, 32, &still, false).len(), 8);

        // Wave: one continuous line across the field, amplitude 0.5.
        let wave = shape(Pattern::Wave, 64, &still, false);
        assert_eq!(wave.len(), WAVE_POINTS + 1);
        assert!(wave.iter().all(|p| !p.blank && p.y.abs() <= 0.5 + 1e-5));
        assert_eq!((wave[0].x, wave[WAVE_POINTS].x), (-1.0, 1.0));

        // Twinkle: N dots inside the field, stable within a cycle, moved after one.
        let mut ph = Phases {
            anim: 0.1,
            ..Phases::default()
        };
        let a = lit(&shape(Pattern::Twinkle, 20, &ph, true));
        assert_eq!(a.len(), 20);
        assert!(a.iter().all(|p| in_field(p) && (0.0..=1.0).contains(&p.r)));
        assert!(a.iter().all(|p| p.x.abs() < 1.0));
        ph.anim = 0.15;
        assert_eq!(a[0].x, lit(&shape(Pattern::Twinkle, 20, &ph, true))[0].x);
        ph.anim_cycles += 1;
        assert_ne!(a[0].x, lit(&shape(Pattern::Twinkle, 20, &ph, true))[0].x);
        // Without animation every dot is at full level.
        assert!(
            lit(&shape(Pattern::Twinkle, 20, &still, false))
                .iter()
                .all(|p| p.r == 1.0)
        );
    }

    #[test]
    fn anim_cycles_count_wraps() {
        let p = Params {
            anim: 1.0,
            ..Params::default()
        };
        let mut ph = Phases::default();
        for _ in 0..125 {
            ph.tick(&p, 0.01); // 2 Hz at full speed: 2.5 cycles
        }
        assert_eq!(ph.anim_cycles, 2);
        assert!((ph.anim - 0.5).abs() < 1e-3, "{}", ph.anim);
    }
}
