//! Point-stream format: the only contract between inputs and the renderer.
//! Follows ILDA closely so a .ild frame converts losslessly later.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// -1..1, origin centre, +y up (ILDA convention).
    pub x: f32,
    pub y: f32,
    /// 0..1 colour and intensity.
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Move without drawing (ILDA blanking bit).
    pub blank: bool,
}

impl Point {
    pub fn lit(x: f32, y: f32, rgb: [f32; 3]) -> Self {
        Self {
            x,
            y,
            r: rgb[0],
            g: rgb[1],
            b: rgb[2],
            blank: false,
        }
    }
    pub fn blank(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            r: 0.0,
            g: 0.0,
            b: 0.0,
            blank: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // point_rate/timestamp are read by ILDA sources (v0.3)
pub struct Frame {
    pub points: Vec<Point>,
    /// Points per second; informative for DMX patterns, timing for ILDA later.
    pub point_rate: u32,
    pub timestamp: f64,
}

/// A drawable primitive: segment between two lit points, or a dot (p0 == p1).
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Segment {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub rgb: [f32; 3],
    pub _pad: f32,
}

/// Segment between consecutive lit points; an isolated lit point becomes a dot.
pub fn segments(points: &[Point], out: &mut Vec<Segment>) {
    out.clear();
    for (i, p) in points.iter().enumerate() {
        if p.blank {
            continue;
        }
        let prev_lit = i > 0 && !points[i - 1].blank;
        let next_lit = i + 1 < points.len() && !points[i + 1].blank;
        if prev_lit {
            let q = points[i - 1];
            // Colour of a segment is the colour of its end point (ILDA).
            out.push(Segment {
                p0: [q.x, q.y],
                p1: [p.x, p.y],
                rgb: [p.r, p.g, p.b],
                _pad: 0.0,
            });
        } else if !next_lit {
            out.push(Segment {
                p0: [p.x, p.y],
                p1: [p.x, p.y],
                rgb: [p.r, p.g, p.b],
                _pad: 0.0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_and_dots() {
        let c = [1.0, 1.0, 1.0];
        let pts = [
            Point::lit(0.0, 0.0, c), // isolated -> dot
            Point::blank(0.5, 0.5),
            Point::lit(-1.0, 0.0, c), // start of line
            Point::lit(1.0, 0.0, c),  // end of line -> one segment
            Point::blank(0.0, 0.0),
            Point::blank(0.0, 1.0),
            Point::lit(0.0, 1.0, c), // isolated at end -> dot
        ];
        let mut segs = Vec::new();
        segments(&pts, &mut segs);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].p0, segs[0].p1);
        assert_eq!((segs[1].p0, segs[1].p1), ([-1.0, 0.0], [1.0, 0.0]));
        assert_eq!(segs[2].p0, [0.0, 1.0]);
    }
}
