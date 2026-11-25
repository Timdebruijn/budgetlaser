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
    /// Layer of the per-fixture mask texture; set by the renderer.
    pub layer: u32,
}

/// ILDA semantics: the laser state of a point applies to the move into it, so a lit
/// point draws the segment from the previous point, lit or blanked, in its own
/// colour. A blanked jump onto the same spot, or an isolated first point, is a dot.
pub fn segments(points: &[Point], out: &mut Vec<Segment>) {
    out.clear();
    out.extend((0..points.len()).filter_map(|i| segment(points, i)));
}

/// The segment point `i` draws, if any.
fn segment(points: &[Point], i: usize) -> Option<Segment> {
    let p = points[i];
    if p.blank {
        return None;
    }
    let from = match i.checked_sub(1) {
        Some(j) => [points[j].x, points[j].y],
        None if points.get(1).is_some_and(|n| !n.blank) => return None, // a line starts here
        None => [p.x, p.y],
    };
    Some(Segment {
        p0: from,
        p1: [p.x, p.y],
        rgb: [p.r, p.g, p.b],
        width: 1.0,
        layer: 0,
    })
}

/// A scanner tracing the figure as a loop, `n` points per call: the segments of
/// the points it passes, from `pos` on, and never more than one whole pass. `pos`
/// carries the position, fractions included, to the next call.
pub fn scan(points: &[Point], n: f64, pos: &mut f64, out: &mut Vec<Segment>) {
    out.clear();
    let len = points.len();
    if len == 0 {
        return;
    }
    let start = *pos % len as f64; // the figure may have shrunk since the last call
    let end = start + n;
    let first = start as usize;
    let count = (end as usize - first).min(len);
    *pos = end % len as f64;
    out.extend((first..first + count).filter_map(|i| segment(points, i % len)));
}

#[cfg(test)]
mod tests {
    #[test]
    fn scan_passes_part_of_the_loop_per_call() {
        // A lit square, 4 points; segment() of point 0 is the line start, so 3 segments.
        let sq: Vec<Point> = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
            .iter()
            .map(|&[x, y]| Point::lit(x, y, [1.0; 3]))
            .collect();
        let (mut all, mut out) = (Vec::new(), Vec::new());
        segments(&sq, &mut all);
        let mut pos = 0.0;
        // Fast enough for a whole pass: the same segments, once.
        scan(&sq, 10.0, &mut pos, &mut out);
        assert_eq!(out, all);
        // 1.5 points per call: points 2, then 3 and 0 (the line start, no segment).
        pos = 2.0;
        scan(&sq, 1.5, &mut pos, &mut out);
        assert_eq!(out, [all[1]]);
        assert_eq!(pos, 3.5);
        scan(&sq, 1.5, &mut pos, &mut out);
        assert_eq!(out, [all[2]]);
        assert_eq!(pos, 1.0);
        // Too slow to reach the next point: nothing this call.
        scan(&sq, 0.5, &mut pos, &mut out);
        assert!(out.is_empty());
        scan(&[], 5.0, &mut pos, &mut out);
        assert!(out.is_empty());
    }

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
