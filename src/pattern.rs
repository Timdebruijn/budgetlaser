//! Pattern generator: unit-space shapes, then scale → rotate → translate → colour.

use crate::fixture::{Params, Pattern, Rotation};
use crate::point::{Frame, Point};
use std::f32::consts::TAU;

const CIRCLE_POINTS: usize = 96;
const ANIM_MAX_HZ: f32 = 2.0;

/// Phases accumulated per render frame so speed channels are smooth at any DMX rate.
#[derive(Default)]
pub struct Phases {
    pub angle: f32,
    pub anim: f32,
    pub strobe: f32,
}

impl Phases {
    pub fn tick(&mut self, p: &Params, dt: f32) {
        match p.rotation {
            Rotation::Static(a) => self.angle = a,
            Rotation::Spin(w) => self.angle = (self.angle + w * dt) % TAU,
        }
        self.anim = (self.anim + p.anim * ANIM_MAX_HZ * dt) % 1.0;
        self.strobe = if p.strobe_hz > 0.0 {
            (self.strobe + p.strobe_hz * dt) % 1.0
        } else {
            0.0
        };
    }
}

fn unit_shape(pattern: Pattern, beams: u32, anim: f32, animating: bool, out: &mut Vec<Point>) {
    let w = if animating { (anim * TAU).sin() } else { 0.0 }; // -1..1
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
            let r = 1.0 - 0.3 * (0.5 + 0.5 * w) * animating as u8 as f32;
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
            let r = 1.0 - 0.3 * (0.5 + 0.5 * w) * animating as u8 as f32;
            for i in 0..beams {
                let a = i as f32 / beams as f32 * TAU;
                let (s, c) = a.sin_cos();
                out.push(Point::blank(r * c, r * s));
                out.push(Point::lit(r * c, r * s, white));
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
    unit_shape(pattern, p.beams, ph.anim, p.anim > 0.0, &mut frame.points);
    let gain = p.dimmer.powf(dimmer_gamma);
    let rgb = [p.rgb[0] * gain, p.rgb[1] * gain, p.rgb[2] * gain];
    let (s, c) = ph.angle.sin_cos();
    for pt in &mut frame.points {
        let (x, y) = (pt.x * p.size, pt.y * p.size);
        pt.x = x * c - y * s + p.pan;
        pt.y = x * s + y * c + p.tilt;
        if !pt.blank {
            [pt.r, pt.g, pt.b] = rgb;
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
}
