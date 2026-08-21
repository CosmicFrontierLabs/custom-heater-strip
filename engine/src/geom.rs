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

/// Axis-aligned bounds of a segment, arcs measured over their true sweep.
fn bounds(seg: &PathSeg) -> (f64, f64, f64, f64) {
    let mut lo = (f64::INFINITY, f64::INFINITY);
    let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    // An arc can bulge past both endpoints, so sample it rather than taking
    // the chord's box — a box that is too small silently loses real shorts.
    let n = if matches!(seg, PathSeg::Arc { .. }) {
        SAMPLES_PER_SEG
    } else {
        1
    };
    for k in 0..=n {
        let p = sample(seg, k as f64 / n as f64);
        lo = (lo.0.min(p.x), lo.1.min(p.y));
        hi = (hi.0.max(p.x), hi.1.max(p.y));
    }
    (lo.0, lo.1, hi.0, hi.1)
}

/// Every pair of segments in `trace` that shorts against another, ignoring
/// pairs that merely share an endpoint. Returns index pairs, `i < j`.
///
/// A uniform grid keeps this near-linear. The naive form is `O(n²)`, and a
/// heater is the worst possible input for that: the wavy serpentine emits over
/// twenty thousand segments on one board, which is more than four hundred
/// million pair tests — slow enough to look like a hang. Since a trace is a
/// long thin thing packed at uniform pitch, bucketing by bounding box leaves
/// each segment with a handful of candidates instead of all of them.
pub fn find_shorts(trace: &[PathSeg], probe: &[usize]) -> Vec<(usize, usize)> {
    if trace.is_empty() || probe.is_empty() {
        return Vec::new();
    }
    let boxes: Vec<(f64, f64, f64, f64)> = trace.iter().map(bounds).collect();

    // Cell size from the mean segment extent: big enough that a segment spans
    // few cells, small enough that a cell holds few segments.
    let mean = boxes
        .iter()
        .map(|b| (b.2 - b.0).max(b.3 - b.1))
        .sum::<f64>()
        / boxes.len() as f64;
    let cell = mean.max(1e-6);
    let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);

    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, b) in boxes.iter().enumerate() {
        let (lo, hi) = (key(b.0, b.1), key(b.2, b.3));
        for gx in lo.0..=hi.0 {
            for gy in lo.1..=hi.1 {
                grid.entry((gx, gy)).or_default().push(i);
            }
        }
    }

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &i in probe {
        let b = boxes[i];
        let (lo, hi) = (key(b.0, b.1), key(b.2, b.3));
        for gx in lo.0..=hi.0 {
            for gy in lo.1..=hi.1 {
                let Some(bucket) = grid.get(&(gx, gy)) else {
                    continue;
                };
                for &j in bucket {
                    if j == i {
                        continue;
                    }
                    let pair = (i.min(j), i.max(j));
                    if !seen.insert(pair) {
                        continue;
                    }
                    // Cheap box rejection before the exact test.
                    let c = boxes[j];
                    if b.2 < c.0 - EPS || c.2 < b.0 - EPS || b.3 < c.1 - EPS || c.3 < b.1 - EPS {
                        continue;
                    }
                    if shorts(&trace[i], &trace[j]) {
                        out.push(pair);
                    }
                }
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

    /// The grid must be an optimisation, not a filter. A broad phase that
    /// drops a real short is worse than a slow one, so it is checked against
    /// the exhaustive answer on a trace with plenty of near-misses.
    #[test]
    fn the_grid_agrees_with_exhaustive_search() {
        // A serpentine-like comb plus a bar crossing every tooth.
        let mut trace = Vec::new();
        for i in 0..40 {
            let y = i as f64 * 0.5;
            trace.push(line(0.0, y, 20.0, y));
            trace.push(line(20.0, y, 20.0, y + 0.5));
        }
        trace.push(line(10.0, -1.0, 10.0, 21.0));
        // Plus an arc that bulges across several teeth.
        trace.push(PathSeg::Arc {
            a: Point::new(4.0, 4.0),
            b: Point::new(4.0, 9.0),
            center: Point::new(4.0, 6.5),
            ccw: true,
        });

        let all: Vec<usize> = (0..trace.len()).collect();
        let fast = find_shorts(&trace, &all);

        let mut naive = Vec::new();
        for i in 0..trace.len() {
            for j in (i + 1)..trace.len() {
                if shorts(&trace[i], &trace[j]) {
                    naive.push((i, j));
                }
            }
        }
        naive.sort_unstable();

        assert!(!naive.is_empty(), "fixture should contain real shorts");
        assert_eq!(
            fast,
            naive,
            "grid found {} shorts, exhaustive found {}",
            fast.len(),
            naive.len()
        );
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

/// Two pieces of copper closer together than the process allows.
#[derive(Debug, Clone, Copy)]
pub struct TooClose {
    pub a: usize,
    pub b: usize,
    /// Centreline separation at the closest approach, mm.
    pub centre_gap_mm: f64,
    /// Edge-to-edge gap that implies, mm. Negative means the copper merges.
    pub edge_gap_mm: f64,
    /// Where on segment `a` the closest approach happens.
    pub at: Point,
}

/// Sub-chord length used when reducing an arc to a polyline for distance
/// work. The chordal error is `c²/(8r)`, so at 20 µm chords on a
/// turnaround-sized radius the error is well under a nanometre.
const CHORD_MM: f64 = 0.02;

/// Flatten a segment to a polyline fine enough that distances taken from it
/// are exact to far better than any fab tolerance.
fn flatten(seg: &PathSeg) -> Vec<Point> {
    match seg {
        PathSeg::Line { a, b } => vec![*a, *b],
        PathSeg::Arc { .. } => {
            let n = ((seg.length() / CHORD_MM).ceil() as usize).clamp(2, 512);
            (0..=n).map(|k| sample(seg, k as f64 / n as f64)).collect()
        }
    }
}

/// Closest approach between two segments, and where on `a` it happens.
fn closest_approach(a: &PathSeg, b: &PathSeg) -> (f64, Point) {
    if !intersections(a, b).is_empty() {
        let p = intersections(a, b)[0];
        return (0.0, p);
    }
    let (pa, pb) = (flatten(a), flatten(b));
    let mut best = (f64::INFINITY, pa[0]);
    // Every vertex of each polyline against every edge of the other; for
    // non-crossing polylines the closest approach always lands on one.
    for w in pa.windows(2) {
        for q in &pb {
            let d = point_segment_distance(*q, w[0], w[1]);
            if d < best.0 {
                best = (d, nearest_on_segment(*q, w[0], w[1]));
            }
        }
    }
    for w in pb.windows(2) {
        for p in &pa {
            let d = point_segment_distance(*p, w[0], w[1]);
            if d < best.0 {
                best = (d, *p);
            }
        }
    }
    best
}

fn point_segment_distance(p: Point, a: Point, b: Point) -> f64 {
    p.dist(&nearest_on_segment(p, a, b))
}

fn nearest_on_segment(p: Point, a: Point, b: Point) -> Point {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-18 {
        return a;
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    Point::new(a.x + t * dx, a.y + t * dy)
}

/// Find every place the trace's copper comes closer to itself than the fab
/// can hold.
///
/// This is a different question from [`find_shorts`], and the one that
/// actually decides manufacturability. A trace is `width_mm` wide, so two
/// centrelines must stay `width_mm + min_gap_mm` apart for their *edges* to
/// clear by `min_gap_mm`. Straight rows at a pitch of width-plus-gap satisfy
/// that by construction, which is why checking only for crossings looked
/// sufficient for a long time — but anything that makes a row wander, like the
/// wavy pattern, can bring neighbouring centrelines closer without either one
/// ever crossing the other.
///
/// Segments adjacent in the path are skipped: they meet end to end by
/// construction and are the same conductor.
pub fn find_too_close(trace: &[PathSeg], width_mm: f64, min_gap_mm: f64) -> Vec<TooClose> {
    let need = width_mm + min_gap_mm;
    if trace.is_empty() || need <= 0.0 {
        return Vec::new();
    }
    let boxes: Vec<(f64, f64, f64, f64)> = trace.iter().map(bounds).collect();

    // Distance along the path to the start of each segment.
    //
    // Two points on one conductor that are `s` apart *along* it are at most
    // `s` apart in space, so any pair closer together than `need` along the
    // path is guaranteed to fail a straight distance test and guaranteed to
    // tell us nothing: the copper between them is one continuous piece.
    // Skipping only the immediately adjacent segment is not enough once a
    // pattern subdivides its rows — a wavy row sampled every 0.19 mm reports
    // thousands of 0.19 mm "faults" that are just the wire next to itself.
    let mut cum = Vec::with_capacity(trace.len() + 1);
    let mut run = 0.0;
    for seg in trace {
        cum.push(run);
        run += seg.length();
    }
    cum.push(run);

    // Same grid as find_shorts, but cells at least the search radius so a
    // near-miss cannot fall between buckets.
    let mean = boxes
        .iter()
        .map(|b| (b.2 - b.0).max(b.3 - b.1))
        .sum::<f64>()
        / boxes.len() as f64;
    let cell = mean.max(need).max(1e-6);
    let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);

    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, b) in boxes.iter().enumerate() {
        let (lo, hi) = (key(b.0, b.1), key(b.2, b.3));
        for gx in lo.0..=hi.0 {
            for gy in lo.1..=hi.1 {
                grid.entry((gx, gy)).or_default().push(i);
            }
        }
    }

    let mut out: Vec<TooClose> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for i in 0..trace.len() {
        let b = boxes[i];
        // Search one cell out, since a neighbour within `need` can sit there.
        let (lo, hi) = (key(b.0 - need, b.1 - need), key(b.2 + need, b.3 + need));
        for gx in lo.0..=hi.0 {
            for gy in lo.1..=hi.1 {
                let Some(bucket) = grid.get(&(gx, gy)) else {
                    continue;
                };
                for &j in bucket {
                    // Skip pairs too close together along the path to mean
                    // anything (see `cum` above).
                    if j == i || cum[i.max(j)] - cum[i.min(j) + 1] < need {
                        continue;
                    }
                    let pair = (i.min(j), i.max(j));
                    if !seen.insert(pair) {
                        continue;
                    }
                    let c = boxes[j];
                    if b.2 + need < c.0 || c.2 + need < b.0 || b.3 + need < c.1 || c.3 + need < b.1
                    {
                        continue;
                    }
                    let (d, at) = closest_approach(&trace[i], &trace[j]);
                    if d < need - EPS {
                        out.push(TooClose {
                            a: pair.0,
                            b: pair.1,
                            centre_gap_mm: d,
                            edge_gap_mm: d - width_mm,
                            at,
                        });
                    }
                }
            }
        }
    }
    out.sort_by(|x, y| x.centre_gap_mm.partial_cmp(&y.centre_gap_mm).unwrap());
    out
}

#[cfg(test)]
mod clearance_tests {
    use super::*;
    use crate::Point;

    fn poly(pts: &[(f64, f64)]) -> Vec<PathSeg> {
        pts.windows(2)
            .map(|w| PathSeg::Line {
                a: Point::new(w[0].0, w[0].1),
                b: Point::new(w[1].0, w[1].1),
            })
            .collect()
    }

    /// The bug this check shipped with, and the reason it reported 45,000
    /// faults on a design with none.
    ///
    /// A smooth curve sampled finely enough has neighbouring samples closer
    /// together than any clearance you care about — that is what "finely"
    /// means. Skipping only the immediately adjacent segment leaves every
    /// consecutive-but-one pair to be measured against the fab gap, and they
    /// all fail, because a wire is always zero distance from itself.
    #[test]
    fn a_finely_sampled_straight_run_is_not_close_to_itself() {
        let pts: Vec<(f64, f64)> = (0..=200).map(|i| (i as f64 * 0.05, 0.0)).collect();
        assert!(find_too_close(&poly(&pts), 0.3, 0.15).is_empty());
    }

    /// Sampling density must not change the verdict either way.
    #[test]
    fn sampling_density_does_not_change_the_verdict() {
        // Two parallel runs 0.6 mm apart, joined at the right — comfortably
        // clear at a 0.45 mm requirement however finely they are cut up.
        for step in [0.05, 0.5, 2.0] {
            let n = (10.0f64 / step) as usize;
            let mut pts: Vec<(f64, f64)> = (0..=n).map(|i| (i as f64 * step, 0.0)).collect();
            pts.extend((0..=n).map(|i| (10.0 - i as f64 * step, 0.6)));
            assert!(
                find_too_close(&poly(&pts), 0.3, 0.15).is_empty(),
                "step {step} reported a fault on a 0.6 mm gap"
            );
        }
    }

    /// And it still catches the real thing: the same hairpin pinched to
    /// 0.20 mm, which is under the 0.45 mm two 0.3 mm-wide centrelines need
    /// and in fact close enough that the copper overlaps outright.
    #[test]
    fn a_hairpin_tighter_than_the_gap_is_caught() {
        let mut pts: Vec<(f64, f64)> = (0..=200).map(|i| (i as f64 * 0.05, 0.0)).collect();
        pts.extend((0..=200).map(|i| (10.0 - i as f64 * 0.05, 0.20)));
        let faults = find_too_close(&poly(&pts), 0.3, 0.15);
        assert!(!faults.is_empty(), "merged copper went unreported");
        assert!((faults[0].centre_gap_mm - 0.20).abs() < 1e-6);
        assert!(faults[0].edge_gap_mm < 0.0, "should read as overlapping");
    }
}
