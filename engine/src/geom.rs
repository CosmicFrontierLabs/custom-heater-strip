//! Exact intersection tests for routed path segments.
//!
//! A heater is one long conductor folded to nearly touch itself, so "does
//! this path cross itself" is the check that separates a working board from a
//! short. Arcs have to be treated as arcs: a semicircular turnaround bulges
//! well outside the chord between its endpoints, so chord-based tests report
//! crossings that do not exist (and, for an arc that doubles back, miss ones
//! that do). Everything here works on true circles restricted to their sweep.

use crate::{PathSeg, Point};

/// Points closer together than this are the same point. Board coordinates are
/// millimetres and the finest real feature is around 0.1 mm, so 1e-7 mm sits
/// far below anything physical while still absorbing accumulated float error.
const EPS: f64 = 1e-7;

/// Every point at which two segments meet.
pub fn intersections(a: &PathSeg, b: &PathSeg) -> Vec<Point> {
    match (a, b) {
        (PathSeg::Line { a: p0, b: p1 }, PathSeg::Line { a: q0, b: q1 }) => {
            line_line(*p0, *p1, *q0, *q1)
        }
        (PathSeg::Line { a: p0, b: p1 }, PathSeg::Arc { .. }) => line_arc(*p0, *p1, b),
        (PathSeg::Arc { .. }, PathSeg::Line { a: q0, b: q1 }) => line_arc(*q0, *q1, a),
        (PathSeg::Arc { .. }, PathSeg::Arc { .. }) => arc_arc(a, b),
    }
}

/// Does copper touch copper anywhere other than at an endpoint the two
/// segments legitimately share?
///
/// Consecutive segments of a continuous path meet end to end by construction,
/// and that is not a fault. Anything else is: a T-junction where one
/// segment's end lands part-way along another, or a genuine crossing.
pub fn shorts(a: &PathSeg, b: &PathSeg) -> bool {
    intersections(a, b)
        .iter()
        .any(|p| !(is_endpoint(a, *p) && is_endpoint(b, *p)))
}

fn is_endpoint(s: &PathSeg, p: Point) -> bool {
    s.start().dist(&p) < EPS || s.end().dist(&p) < EPS
}

/// Is `p`, already known to lie on the segment's circle, within its sweep?
fn arc_contains(arc: &PathSeg, p: Point) -> bool {
    let PathSeg::Arc { a, center, ccw, .. } = arc else {
        return false;
    };
    let ang = |q: &Point| (q.y - center.y).atan2(q.x - center.x);
    let from = ang(a);
    let to = ang(&p);
    // Angle travelled from the arc's start to p, in the direction of travel.
    let mut delta = if *ccw { to - from } else { from - to };
    while delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    while delta >= std::f64::consts::TAU {
        delta -= std::f64::consts::TAU;
    }
    // Allow a hair past either end so a point sitting exactly on a terminus
    // is reported (the caller decides whether a shared end is a fault).
    let slack = angular_slack(arc);
    delta <= arc.sweep() + slack || delta >= std::f64::consts::TAU - slack
}

/// EPS expressed as an angle at this arc's radius.
fn angular_slack(arc: &PathSeg) -> f64 {
    let r = arc.radius();
    if r < EPS {
        std::f64::consts::PI
    } else {
        (EPS / r).min(1e-3)
    }
}

fn line_line(p0: Point, p1: Point, q0: Point, q1: Point) -> Vec<Point> {
    let r = (p1.x - p0.x, p1.y - p0.y);
    let s = (q1.x - q0.x, q1.y - q0.y);
    let len_r = (r.0 * r.0 + r.1 * r.1).sqrt();
    let len_s = (s.0 * s.0 + s.1 * s.1).sqrt();
    if len_r < EPS || len_s < EPS {
        return Vec::new();
    }
    let denom = r.0 * s.1 - r.1 * s.0;
    let qp = (q0.x - p0.x, q0.y - p0.y);

    // Parallel: either apart, or collinear and possibly overlapping.
    if denom.abs() < EPS * len_r * len_s {
        // Perpendicular distance from q0 to the p line.
        if (qp.0 * r.1 - qp.1 * r.0).abs() > EPS * len_r {
            return Vec::new();
        }
        // Project both q ends onto p's parameter axis and clip to [0, 1].
        let t_of = |pt: Point| ((pt.x - p0.x) * r.0 + (pt.y - p0.y) * r.1) / (len_r * len_r);
        let (mut t0, mut t1) = (t_of(q0), t_of(q1));
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        let lo = t0.max(0.0);
        let hi = t1.min(1.0);
        let slack = EPS / len_r;
        if hi < lo - slack {
            return Vec::new();
        }
        let at = |t: f64| Point::new(p0.x + t * r.0, p0.y + t * r.1);
        // A single touch point, or the two ends of a real overlap.
        if (hi - lo).abs() <= slack {
            return vec![at(lo)];
        }
        return vec![at(lo), at(hi)];
    }

    let t = (qp.0 * s.1 - qp.1 * s.0) / denom;
    let u = (qp.0 * r.1 - qp.1 * r.0) / denom;
    let st = EPS / len_r;
    let su = EPS / len_s;
    if t < -st || t > 1.0 + st || u < -su || u > 1.0 + su {
        return Vec::new();
    }
    vec![Point::new(p0.x + t * r.0, p0.y + t * r.1)]
}

fn line_arc(p0: Point, p1: Point, arc: &PathSeg) -> Vec<Point> {
    let PathSeg::Arc { center, .. } = arc else {
        return Vec::new();
    };
    let radius = arc.radius();
    let d = (p1.x - p0.x, p1.y - p0.y);
    let len = (d.0 * d.0 + d.1 * d.1).sqrt();
    if len < EPS || radius < EPS {
        return Vec::new();
    }
    // |p0 + t·d − c|² = r²  →  aa·t² + bb·t + cc = 0
    let f = (p0.x - center.x, p0.y - center.y);
    let aa = d.0 * d.0 + d.1 * d.1;
    let bb = 2.0 * (f.0 * d.0 + f.1 * d.1);
    let cc = f.0 * f.0 + f.1 * f.1 - radius * radius;
    let disc = bb * bb - 4.0 * aa * cc;
    // Scale the tangency window with the geometry rather than using a bare
    // absolute epsilon, which would be meaningless at these magnitudes.
    let disc_tol = EPS * aa * radius.max(1.0);
    if disc < -disc_tol {
        return Vec::new();
    }
    let root = if disc <= disc_tol { 0.0 } else { disc.sqrt() };
    let slack = EPS / len;
    let mut out = Vec::new();
    for t in [(-bb - root) / (2.0 * aa), (-bb + root) / (2.0 * aa)] {
        if t < -slack || t > 1.0 + slack {
            continue;
        }
        let p = Point::new(p0.x + t * d.0, p0.y + t * d.1);
        if arc_contains(arc, p) && !out.iter().any(|q: &Point| q.dist(&p) < EPS) {
            out.push(p);
        }
        if root == 0.0 {
            break; // tangent: one root only
        }
    }
    out
}

fn arc_arc(a: &PathSeg, b: &PathSeg) -> Vec<Point> {
    let (PathSeg::Arc { center: c1, .. }, PathSeg::Arc { center: c2, .. }) = (a, b) else {
        return Vec::new();
    };
    let (r1, r2) = (a.radius(), b.radius());
    let d = c1.dist(c2);

    // Same circle: the arcs may run along each other. Report a point inside
    // the shared span if there is one — that is a short either way.
    if d < EPS && (r1 - r2).abs() < EPS {
        for p in [a.start(), a.end(), b.start(), b.end()] {
            if arc_contains(a, p) && arc_contains(b, p) {
                return vec![p];
            }
        }
        return Vec::new();
    }
    if d < EPS || d > r1 + r2 + EPS || d < (r1 - r2).abs() - EPS {
        return Vec::new();
    }

    // Distance along the centre line to the radical line, then off it by h.
    let x = ((r1 * r1 - r2 * r2) / d + d) / 2.0;
    let h2 = r1 * r1 - x * x;
    let h = if h2 <= 0.0 { 0.0 } else { h2.sqrt() };
    let (ux, uy) = ((c2.x - c1.x) / d, (c2.y - c1.y) / d);
    let base = Point::new(c1.x + x * ux, c1.y + x * uy);

    let mut out = Vec::new();
    for p in [
        Point::new(base.x - h * uy, base.y + h * ux),
        Point::new(base.x + h * uy, base.y - h * ux),
    ] {
        if arc_contains(a, p) && arc_contains(b, p) && !out.iter().any(|q: &Point| q.dist(&p) < EPS)
        {
            out.push(p);
        }
        if h == 0.0 {
            break; // tangent
        }
    }
    out
}

/// Somewhere the copper does not fit inside the board.
#[derive(Debug, Clone, Copy)]
pub struct Escape {
    /// Index of the offending trace segment.
    pub seg: usize,
    /// The sampled centreline point that is too close to (or past) the edge.
    pub at: Point,
    /// Signed clearance from the outline: positive inside, negative outside.
    pub clearance_mm: f64,
}

/// Points sampled along each segment when checking containment. An arc bulges
/// away from its chord, so its endpoints alone would miss exactly the place a
/// turnaround is most likely to push past an edge.
const SAMPLES_PER_SEG: usize = 8;

/// Find every place the trace's copper leaves the board, or comes closer to the
/// edge than half its own width.
///
/// A centreline inside the outline is not sufficient: the trace is
/// `trace_width_mm` wide, so its *edge* is half that off the centreline, and a
/// centreline sitting exactly on the boundary means half the copper hangs off
/// the board. `margin_mm` is required clearance beyond that half-width.
pub fn find_escapes(
    trace: &[PathSeg],
    outline: &crate::Polygon,
    trace_width_mm: f64,
    margin_mm: f64,
) -> Vec<Escape> {
    let need = trace_width_mm / 2.0 + margin_mm;
    let mut out = Vec::new();
    for (i, seg) in trace.iter().enumerate() {
        let mut worst: Option<Escape> = None;
        for k in 0..=SAMPLES_PER_SEG {
            let t = k as f64 / SAMPLES_PER_SEG as f64;
            let p = sample(seg, t);
            let clearance = outline.clearance(p);
            if clearance < need && worst.is_none_or(|w| clearance < w.clearance_mm) {
                worst = Some(Escape {
                    seg: i,
                    at: p,
                    clearance_mm: clearance,
                });
            }
        }
        out.extend(worst);
    }
    out
}

/// Point a fraction `t` along a segment, following the true arc.
fn sample(seg: &PathSeg, t: f64) -> Point {
    match seg {
        PathSeg::Line { a, b } => Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t),
        PathSeg::Arc { a, center, ccw, .. } => {
            let r = seg.radius();
            let a0 = (a.y - center.y).atan2(a.x - center.x);
            let swept = seg.sweep() * t;
            let ang = if *ccw { a0 + swept } else { a0 - swept };
            Point::new(center.x + r * ang.cos(), center.y + r * ang.sin())
        }
    }
}

/// Every pair of segments in `trace` that shorts against another, ignoring
/// pairs that merely share an endpoint. Returns index pairs, `i < j`.
///
/// `O(n²)`, so callers with long traces should restrict `probe` to the
/// segments they actually care about.
pub fn find_shorts(trace: &[PathSeg], probe: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &i in probe {
        for (j, other) in trace.iter().enumerate() {
            if j == i {
                continue;
            }
            if shorts(&trace[i], other) {
                out.push((i.min(j), i.max(j)));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> PathSeg {
        PathSeg::Line {
            a: Point::new(x0, y0),
            b: Point::new(x1, y1),
        }
    }

    /// Semicircle bulging to the +x side of its centre, travelling top to
    /// bottom. Coordinates are y-down, so sweeping from angle −π/2 to +π/2
    /// through 0 is the positive-angle (`ccw`) direction — the same
    /// convention the serpentine turnarounds use.
    fn right_semicircle(cx: f64, cy: f64, r: f64) -> PathSeg {
        PathSeg::Arc {
            a: Point::new(cx, cy - r),
            b: Point::new(cx, cy + r),
            center: Point::new(cx, cy),
            ccw: true,
        }
    }

    #[test]
    fn the_test_fixture_really_does_bulge_right() {
        // Guards every arc test below: if this helper swept the other way,
        // they would all be asserting about the wrong half of the circle.
        let arc = right_semicircle(0.0, 0.0, 5.0);
        assert!(arc_contains(&arc, Point::new(5.0, 0.0)), "rightmost point");
        assert!(
            !arc_contains(&arc, Point::new(-5.0, 0.0)),
            "leftmost point must be outside the sweep"
        );
    }

    #[test]
    fn crossing_lines_intersect_at_the_crossing_point() {
        let hits = intersections(&line(0.0, 0.0, 10.0, 0.0), &line(5.0, -5.0, 5.0, 5.0));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].dist(&Point::new(5.0, 0.0)) < 1e-9, "{:?}", hits[0]);
        assert!(shorts(
            &line(0.0, 0.0, 10.0, 0.0),
            &line(5.0, -5.0, 5.0, 5.0)
        ));
    }

    #[test]
    fn lines_meeting_end_to_end_are_not_a_short() {
        let a = line(0.0, 0.0, 10.0, 0.0);
        let b = line(10.0, 0.0, 10.0, 10.0);
        assert_eq!(intersections(&a, &b).len(), 1, "they do touch");
        assert!(!shorts(&a, &b), "a shared endpoint is how a path is built");
    }

    #[test]
    fn a_t_junction_is_a_short() {
        // b's end lands in the middle of a, which is copper on copper.
        let a = line(0.0, 0.0, 10.0, 0.0);
        let b = line(5.0, -5.0, 5.0, 0.0);
        assert!(shorts(&a, &b));
    }

    #[test]
    fn parallel_lines_apart_never_intersect() {
        assert!(intersections(&line(0.0, 0.0, 10.0, 0.0), &line(0.0, 1.0, 10.0, 1.0)).is_empty());
    }

    #[test]
    fn collinear_overlap_is_a_short_but_touching_ends_are_not() {
        // Overlapping run: x 5..10 shared.
        assert!(shorts(
            &line(0.0, 0.0, 10.0, 0.0),
            &line(5.0, 0.0, 15.0, 0.0)
        ));
        // Meeting exactly end to end along the same line.
        assert!(!shorts(
            &line(0.0, 0.0, 10.0, 0.0),
            &line(10.0, 0.0, 20.0, 0.0)
        ));
    }

    #[test]
    fn a_chord_crossing_line_does_not_touch_the_arc_itself() {
        // This is the case a chord-based test gets wrong. The arc bulges to
        // x > 0; a vertical line at x = 0 spans the chord but never meets
        // the arc except at its two endpoints.
        let arc = right_semicircle(0.0, 0.0, 5.0);
        let chord_line = line(0.0, -5.0, 0.0, 5.0);
        // Only the endpoints are shared, so this is not a short.
        assert!(
            !shorts(&arc, &chord_line),
            "chord of an arc must not count as crossing it"
        );
        // A line that genuinely cuts the bulge does intersect.
        let cutting = line(0.0, 0.0, 10.0, 0.0);
        let hits = intersections(&arc, &cutting);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].dist(&Point::new(5.0, 0.0)) < 1e-9, "{:?}", hits[0]);
    }

    #[test]
    fn line_missing_the_arcs_sweep_does_not_intersect() {
        // The left half of the circle is not part of this right-half arc.
        let arc = right_semicircle(0.0, 0.0, 5.0);
        let left = line(-10.0, 0.0, -1.0, 0.0);
        assert!(intersections(&arc, &left).is_empty());
    }

    #[test]
    fn tangent_line_touches_the_arc_once() {
        let arc = right_semicircle(0.0, 0.0, 5.0);
        // Vertical tangent at the arc's rightmost point (5, 0).
        let hits = intersections(&arc, &line(5.0, -3.0, 5.0, 3.0));
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].dist(&Point::new(5.0, 0.0)) < 1e-6, "{:?}", hits[0]);
    }

    #[test]
    fn nested_arcs_at_different_radii_never_meet() {
        // Concentric turnarounds one pitch apart — the bifilar arrangement.
        let inner = right_semicircle(0.0, 0.0, 5.0);
        let outer = right_semicircle(0.0, 0.0, 6.0);
        assert!(intersections(&inner, &outer).is_empty());
        assert!(!shorts(&inner, &outer));
    }

    #[test]
    fn overlapping_arcs_on_offset_centers_intersect_twice() {
        // Two circles of radius 5 whose centres are 6 apart cross at two
        // points; full circles are used so both lie within the sweeps.
        let full = |cx: f64| PathSeg::Arc {
            a: Point::new(cx + 5.0, 0.0),
            b: Point::new(cx + 5.0, 0.0),
            center: Point::new(cx, 0.0),
            ccw: true,
        };
        let hits = intersections(&full(0.0), &full(6.0));
        assert_eq!(hits.len(), 2, "{hits:?}");
        for p in &hits {
            assert!((p.x - 3.0).abs() < 1e-9, "{p:?}");
            assert!((p.y.abs() - 4.0).abs() < 1e-9, "{p:?}");
        }
    }

    #[test]
    fn arcs_meeting_at_a_shared_endpoint_are_not_a_short() {
        // Two semicircles forming an S: they join at (0, 5) and nowhere else.
        let upper = right_semicircle(0.0, 0.0, 5.0);
        let lower = PathSeg::Arc {
            a: Point::new(0.0, 5.0),
            b: Point::new(0.0, 15.0),
            center: Point::new(0.0, 10.0),
            ccw: true,
        };
        assert!(!shorts(&upper, &lower));
    }

    #[test]
    fn find_shorts_reports_each_pair_once() {
        let trace = vec![
            line(0.0, 0.0, 10.0, 0.0),
            line(10.0, 0.0, 10.0, 10.0),
            // Crosses segment 0 in its interior.
            line(5.0, -5.0, 5.0, 5.0),
        ];
        assert_eq!(find_shorts(&trace, &[0, 1, 2]), vec![(0, 2)]);
    }
}
