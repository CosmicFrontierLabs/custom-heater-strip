//! Parallel offsetting of a trace centerline — the engine behind the
//! bifilar "out and back" construction: offset a base path ±p/2, join the
//! far ends with a cap, and both terminals land adjacent at the start.
//!
//! This is not a general polygon offsetter: it assumes the well-behaved
//! paths our own generators emit (no cusps, arc radii > |d|), which lets
//! joins be simple — tangent for G1 corners, line-intersection miters for
//! the rectangular/mitered corner styles.

use crate::{PathSeg, Point};

/// Offset every segment of `path` by `d` perpendicular to travel.
/// Positive `d` offsets toward the left normal `(dy, -dx)` of travel
/// (radially outward for positive-sweep arcs in our y-down frame).
pub fn offset_path(path: &[PathSeg], d: f64) -> Vec<PathSeg> {
    let mut out: Vec<PathSeg> = Vec::with_capacity(path.len());
    for seg in path {
        let off = offset_seg(seg, d);
        if let Some(mut prev) = out.pop() {
            let mut extra = Vec::new();
            join(&mut prev, &off, &mut extra);
            out.push(prev);
            out.extend(extra);
        }
        match off {
            OffsetSeg::Line { a, b } => out.push(PathSeg::Line { a, b }),
            OffsetSeg::Arc(seg) => out.push(seg),
        }
    }
    out
}

enum OffsetSeg {
    Line { a: Point, b: Point },
    Arc(PathSeg),
}

impl OffsetSeg {
    fn start(&self) -> Point {
        match self {
            OffsetSeg::Line { a, .. } => *a,
            OffsetSeg::Arc(s) => s.start(),
        }
    }
}

fn offset_seg(seg: &PathSeg, d: f64) -> OffsetSeg {
    match seg {
        PathSeg::Line { a, b } => {
            let len = a.dist(b).max(1e-12);
            let (dx, dy) = ((b.x - a.x) / len, (b.y - a.y) / len);
            let n = (dy, -dx);
            OffsetSeg::Line {
                a: Point::new(a.x + n.0 * d, a.y + n.1 * d),
                b: Point::new(b.x + n.0 * d, b.y + n.1 * d),
            }
        }
        PathSeg::Arc { a, b, center, ccw } => {
            let r = center.dist(a);
            // Left normal is radially outward for ccw travel, inward for cw.
            let r_new = if *ccw { r + d } else { r - d };
            debug_assert!(r_new > 1e-9, "offset collapses arc radius");
            let scale = r_new / r;
            let remap = |p: &Point| {
                Point::new(
                    center.x + (p.x - center.x) * scale,
                    center.y + (p.y - center.y) * scale,
                )
            };
            OffsetSeg::Arc(PathSeg::Arc {
                a: remap(a),
                b: remap(b),
                center: *center,
                ccw: *ccw,
            })
        }
    }
}

/// Make the previous offset segment meet the next one: G1 corners already
/// coincide; line-line corners get a miter (intersection of the two lines);
/// anything else gets a short connector inserted.
fn join(prev: &mut PathSeg, next: &OffsetSeg, out_extra: &mut Vec<PathSeg>) {
    let gap_end = prev.end();
    let next_start = next.start();
    if gap_end.dist(&next_start) < 1e-9 {
        return;
    }
    if let (PathSeg::Line { a: pa, b: pb }, OffsetSeg::Line { a: na, b: nb }) = (&*prev, next) {
        if let Some(x) = line_intersection(*pa, *pb, *na, *nb) {
            // Miter join: extend/trim both lines to their intersection.
            if let PathSeg::Line { b, .. } = prev {
                *b = x;
            }
            out_extra.push(PathSeg::Line {
                a: x,
                b: next_start,
            });
            // The pushed connector is degenerate when the next line starts
            // at the intersection; drop it in that case.
            if x.dist(&next_start) < 1e-9 {
                out_extra.pop();
            }
            return;
        }
    }
    // Fallback: straight connector across the gap.
    out_extra.push(PathSeg::Line {
        a: gap_end,
        b: next_start,
    });
}

fn line_intersection(a1: Point, a2: Point, b1: Point, b2: Point) -> Option<Point> {
    let d1 = (a2.x - a1.x, a2.y - a1.y);
    let d2 = (b2.x - b1.x, b2.y - b1.y);
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() < 1e-12 {
        return None; // parallel
    }
    let t = ((b1.x - a1.x) * d2.1 - (b1.y - a1.y) * d2.0) / denom;
    Some(Point::new(a1.x + t * d1.0, a1.y + t * d1.1))
}

/// Reverse a path: segments in reverse order, each with endpoints swapped
/// (and arc sweep direction flipped).
pub fn reverse_path(path: &[PathSeg]) -> Vec<PathSeg> {
    path.iter()
        .rev()
        .map(|seg| match seg {
            PathSeg::Line { a, b } => PathSeg::Line { a: *b, b: *a },
            PathSeg::Arc { a, b, center, ccw } => PathSeg::Arc {
                a: *b,
                b: *a,
                center: *center,
                ccw: !ccw,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_offset_shifts_perpendicular() {
        let path = [PathSeg::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(10.0, 0.0),
        }];
        // Travel +x: left normal is (0,-1) → negative y (up in board coords).
        let off = offset_path(&path, 0.5);
        assert!(off[0].start().dist(&Point::new(0.0, -0.5)) < 1e-12);
        assert!(off[0].end().dist(&Point::new(10.0, -0.5)) < 1e-12);
    }

    #[test]
    fn ccw_arc_offset_grows_radius() {
        let arc = [PathSeg::Arc {
            a: Point::new(1.0, 0.0),
            b: Point::new(-1.0, 0.0),
            center: Point::new(0.0, 0.0),
            ccw: true,
        }];
        let off = offset_path(&arc, 0.25);
        assert!((off[0].radius() - 1.25).abs() < 1e-12);
        let off_in = offset_path(&arc, -0.25);
        assert!((off_in[0].radius() - 0.75).abs() < 1e-12);
    }

    #[test]
    fn right_angle_corner_gets_mitered() {
        let path = [
            PathSeg::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(10.0, 0.0),
            },
            PathSeg::Line {
                a: Point::new(10.0, 0.0),
                b: Point::new(10.0, 10.0),
            },
        ];
        // Offset to the left of travel: the outside of this corner.
        let off = offset_path(&path, 0.5);
        // Continuous through the corner:
        for w in off.windows(2) {
            assert!(w[0].end().dist(&w[1].start()) < 1e-9);
        }
        // Outer corner passes through the miter point (10.5, -0.5).
        assert!(off
            .iter()
            .any(|s| s.end().dist(&Point::new(10.5, -0.5)) < 1e-9
                || s.start().dist(&Point::new(10.5, -0.5)) < 1e-9));
    }

    #[test]
    fn reverse_path_flips_everything() {
        let path = [
            PathSeg::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(1.0, 0.0),
            },
            PathSeg::Arc {
                a: Point::new(1.0, 0.0),
                b: Point::new(1.0, 2.0),
                center: Point::new(1.0, 1.0),
                ccw: true,
            },
        ];
        let rev = reverse_path(&path);
        assert!(rev[0].start().dist(&Point::new(1.0, 2.0)) < 1e-12);
        assert!(rev[1].end().dist(&Point::new(0.0, 0.0)) < 1e-12);
        match &rev[0] {
            PathSeg::Arc { ccw, .. } => assert!(!ccw),
            _ => panic!("expected arc first after reversal"),
        }
    }
}
