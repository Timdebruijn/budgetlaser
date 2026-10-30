//! Point-stream format: the only contract between inputs and the renderer.
//! Follows ILDA closely so a .ild frame converts losslessly.

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
pub struct Frame {
    pub points: Vec<Point>,
}

/// A drawable primitive: segment between two lit points, or a dot (p0 == p1).
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Segment {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub rgb: [f32; 3],
    /// Multiplier on the configured line width.
    pub width: f32,
}

/// ILDA semantics: the laser state of a point applies to the move into it, so a lit
/// point draws the segment from the previous point, lit or blanked, in its own
/// colour. A blanked jump onto the same spot, or an isolated first point, is a dot.
pub fn segments(points: &[Point], out: &mut Vec<Segment>) {
    out.clear();
    for (i, p) in points.iter().enumerate() {
        if p.blank {
            continue;
        }
        let from = match i.checked_sub(1) {
            Some(j) => [points[j].x, points[j].y],
            None if points.get(1).is_some_and(|n| !n.blank) => continue, // a line starts here
            None => [p.x, p.y],
        };
        out.push(Segment {
            p0: from,
            p1: [p.x, p.y],
            rgb: [p.r, p.g, p.b],
            width: 1.0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_and_dots() {
        let c = [1.0, 1.0, 1.0];
        let pts = [
            Point::lit(0.0, 0.0, c), // isolated first point -> dot
            Point::blank(0.5, 0.5),
            Point::lit(-1.0, 0.0, c), // lit move from the blanked point -> segment
            Point::lit(1.0, 0.0, c),  // -> segment
            Point::blank(0.0, 0.0),
            Point::blank(0.0, 1.0),
            Point::lit(0.0, 1.0, c), // blanked jump onto the same spot -> dot
        ];
        let mut segs = Vec::new();
        segments(&pts, &mut segs);
        let ends: Vec<_> = segs.iter().map(|s| (s.p0, s.p1)).collect();
        assert_eq!(
            ends,
            [
                ([0.0, 0.0], [0.0, 0.0]),
                ([0.5, 0.5], [-1.0, 0.0]),
                ([-1.0, 0.0], [1.0, 0.0]),
                ([0.0, 1.0], [0.0, 1.0]),
            ]
        );
        // A line's first point adds no dot of its own.
        segments(&pts[2..4], &mut segs);
        assert_eq!(segs.len(), 1);
    }
}
