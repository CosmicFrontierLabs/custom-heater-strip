//! Turn a pile of loose line segments into closed composite polygons.
//!
//! Real DXF exports are frequently nothing but `LINE` entities, and they are
//! rarely tidy: segments cross each other mid-span, duplicate each other
//! exactly, dangle, and — when the file is a flattened 3D model — meet at
//! interior junctions where several facet edges converge. None of that has a
//! single obvious ring to read off, so the segments are turned into a planar
//! arrangement and the boundary of each connected piece is traced.
//!
//! The result is the **silhouette** of each piece: interior subdivisions are
//! swallowed. That is deliberate and matches the rest of the engine, which
//! fills one simply-connected region at a time and does not support holes.
//!
//! Three steps:
//!
//! 1. **Split at crossings.** Every segment is cut at every point another
//!    segment touches it, so afterwards segments only ever meet at endpoints.
//! 2. **Clean up.** Duplicate edges collapse (the top and bottom faces of an
//!    extrusion project onto each other), and dangling chains are pruned
//!    repeatedly — a spur would otherwise be walked out and back.
//! 3. **Trace each component's boundary** by always taking the tightest
//!    clockwise turn, which is the standard way to walk the outer face of a
//!    planar subdivision.

use std::collections::{BTreeMap, BTreeSet};

use crate::Point;

/// One traced piece of the arrangement.
pub struct Piece {
    /// Closed ring, in input coordinates.
    pub ring: Vec<Point>,
}

/// Vertices closer than `extent * QUANTUM` are treated as the same point.
/// Coarse enough to absorb export rounding, fine enough not to merge real
/// features: at a 100 mm extent this is 0.1 µm.
const QUANTUM: f64 = 1e-9;

type Key = (i64, i64);

/// Build the arrangement and trace one ring per connected piece.
pub fn compose(segments: &[(Point, Point)], warnings: &mut Vec<String>) -> Vec<Piece> {
    if segments.is_empty() {
        return Vec::new();
    }
    let extent = segments
        .iter()
        .flat_map(|(a, b)| [a.x.abs(), a.y.abs(), b.x.abs(), b.y.abs()])
        .fold(0.0f64, f64::max)
        .max(1.0);
    let tol = extent * QUANTUM;

    let split = split_at_crossings(segments, tol);
    let mut graph = Graph::build(&split, tol);
    let pruned = graph.prune_spurs();
    if pruned > 0 {
        warnings.push(format!(
            "{pruned} dangling line segment(s) ignored; they enclose no area"
        ));
    }

    let mut pieces = Vec::new();
    let mut untraceable = 0usize;
    for component in graph.components() {
        match graph.trace_boundary(&component) {
            Some(ring) if ring.len() >= 3 => pieces.push(Piece { ring }),
            // A component too small to enclose anything is unremarkable; one
            // with real size that will not close is worth saying out loud.
            _ if component.len() >= 3 => untraceable += 1,
            _ => {}
        }
    }
    if untraceable > 0 {
        warnings.push(format!(
            "{untraceable} group(s) of line work could not be closed into an              outline and were skipped"
        ));
    }
    pieces
}

/// Cut every segment wherever another one meets it, so the result only ever
/// touches at endpoints.
fn split_at_crossings(segments: &[(Point, Point)], tol: f64) -> Vec<(Point, Point)> {
    let n = segments.len();
    // Parameters along each segment where a cut is needed, ends included.
    let mut cuts: Vec<Vec<f64>> = vec![vec![0.0, 1.0]; n];

    for i in 0..n {
        for j in (i + 1)..n {
            let (a0, a1) = segments[i];
            let (b0, b1) = segments[j];
            for (t, u) in crossings(a0, a1, b0, b1, tol) {
                cuts[i].push(t);
                cuts[j].push(u);
            }
        }
    }

    let mut out = Vec::new();
    for (i, (a, b)) in segments.iter().enumerate() {
        let len = a.dist(b);
        if len <= tol {
            continue;
        }
        let mut ts = std::mem::take(&mut cuts[i]);
        ts.retain(|t| t.is_finite() && *t >= 0.0 && *t <= 1.0);
        ts.sort_by(|p, q| p.partial_cmp(q).unwrap());
        // Merge parameters that land on the same physical point.
        let min_dt = tol / len;
        ts.dedup_by(|x, y| (*x - *y).abs() <= min_dt);
        for w in ts.windows(2) {
            let p = lerp(*a, *b, w[0]);
            let q = lerp(*a, *b, w[1]);
            if p.dist(&q) > tol {
                out.push((p, q));
            }
        }
    }
    out
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// Parameter pairs where two segments touch. Handles the collinear-overlap
/// case by cutting at the overlap's ends, which is what keeps coincident
/// edges from confusing the traversal.
fn crossings(a0: Point, a1: Point, b0: Point, b1: Point, tol: f64) -> Vec<(f64, f64)> {
    let r = (a1.x - a0.x, a1.y - a0.y);
    let s = (b1.x - b0.x, b1.y - b0.y);
    let denom = r.0 * s.1 - r.1 * s.0;
    let qp = (b0.x - a0.x, b0.y - a0.y);
    let len_r = (r.0 * r.0 + r.1 * r.1).sqrt().max(1e-30);
    let len_s = (s.0 * s.0 + s.1 * s.1).sqrt().max(1e-30);

    if denom.abs() <= tol * len_r * len_s {
        // Parallel. Only interesting if they are the same line and overlap.
        if (qp.0 * r.1 - qp.1 * r.0).abs() > tol * len_r {
            return Vec::new();
        }
        let along = |p: Point| ((p.x - a0.x) * r.0 + (p.y - a0.y) * r.1) / (len_r * len_r);
        let (mut t0, mut t1) = (along(b0), along(b1));
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        let mut out = Vec::new();
        for t in [t0, t1] {
            if t > 0.0 && t < 1.0 {
                // Where on b does that point sit?
                let p = lerp(a0, a1, t);
                let u = ((p.x - b0.x) * s.0 + (p.y - b0.y) * s.1) / (len_s * len_s);
                out.push((t, u.clamp(0.0, 1.0)));
            }
        }
        return out;
    }

    let t = (qp.0 * s.1 - qp.1 * s.0) / denom;
    let u = (qp.0 * r.1 - qp.1 * r.0) / denom;
    let (st, su) = (tol / len_r, tol / len_s);
    if t < -st || t > 1.0 + st || u < -su || u > 1.0 + su {
        return Vec::new();
    }
    vec![(t.clamp(0.0, 1.0), u.clamp(0.0, 1.0))]
}

struct Graph {
    tol: f64,
    /// Neighbours of each vertex.
    adj: BTreeMap<Key, BTreeSet<Key>>,
}

impl Graph {
    fn build(segments: &[(Point, Point)], tol: f64) -> Self {
        let mut adj: BTreeMap<Key, BTreeSet<Key>> = BTreeMap::new();
        for (a, b) in segments {
            let (ka, kb) = (key(*a, tol), key(*b, tol));
            if ka == kb {
                continue;
            }
            // A BTreeSet collapses duplicate edges for free.
            adj.entry(ka).or_default().insert(kb);
            adj.entry(kb).or_default().insert(ka);
        }
        Graph { tol, adj }
    }

    /// Repeatedly drop degree-1 vertices. A spur encloses no area, and leaving
    /// it in makes the boundary walk travel out along it and back.
    fn prune_spurs(&mut self) -> usize {
        let mut removed = 0;
        loop {
            let leaves: Vec<Key> = self
                .adj
                .iter()
                .filter(|(_, n)| n.len() <= 1)
                .map(|(k, _)| *k)
                .collect();
            if leaves.is_empty() {
                return removed;
            }
            for leaf in leaves {
                if let Some(neighbours) = self.adj.remove(&leaf) {
                    removed += 1;
                    for n in neighbours {
                        if let Some(set) = self.adj.get_mut(&n) {
                            set.remove(&leaf);
                        }
                    }
                }
            }
        }
    }

    /// Connected components, as vertex lists.
    fn components(&self) -> Vec<Vec<Key>> {
        let mut seen: BTreeSet<Key> = BTreeSet::new();
        let mut out = Vec::new();
        for &start in self.adj.keys() {
            if !seen.insert(start) {
                continue;
            }
            let mut stack = vec![start];
            let mut group = vec![start];
            while let Some(v) = stack.pop() {
                for &n in &self.adj[&v] {
                    if seen.insert(n) {
                        group.push(n);
                        stack.push(n);
                    }
                }
            }
            out.push(group);
        }
        out
    }

    /// Walk the outer boundary of one component.
    ///
    /// Start at the lowest-leftmost vertex, which is guaranteed to be on the
    /// outer face, heading in a direction nothing can lie clockwise of. Then
    /// repeatedly take the tightest clockwise turn available. That is the
    /// standard planar-subdivision boundary walk: it hugs the outside and
    /// never wanders into an interior face.
    fn trace_boundary(&self, component: &[Key]) -> Option<Vec<Point>> {
        let start = *component
            .iter()
            .min_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)))?;
        let pt = |k: Key| Point::new(k.0 as f64 * self.tol, k.1 as f64 * self.tol);

        let mut ring = vec![start];
        // Nothing in the component lies above-left of `start`, so arriving
        // from straight above is a safe fiction for the first turn.
        let mut prev = (start.0, start.1 - 1);
        let mut cur = start;
        // Each undirected edge can carry the boundary at most twice, so this
        // bounds the walk and stops a malformed graph looping forever.
        let limit = 4 * component.len() + 8;

        for _ in 0..limit {
            let from = pt(prev);
            let here = pt(cur);
            let incoming = (here.x - from.x, here.y - from.y);
            let next = self
                .adj
                .get(&cur)?
                .iter()
                .copied()
                .filter(|n| *n != prev || self.adj[&cur].len() == 1)
                .min_by(|x, y| {
                    turn(incoming, here, pt(*x))
                        .partial_cmp(&turn(incoming, here, pt(*y)))
                        .unwrap()
                })
                .or_else(|| self.adj.get(&cur)?.iter().copied().next())?;

            if next == start {
                return Some(ring.into_iter().map(pt).collect());
            }
            ring.push(next);
            prev = cur;
            cur = next;
        }
        None
    }
}

/// Clockwise turn angle from the incoming direction to the candidate edge, in
/// `[0, 2π)`. Smallest value is the tightest clockwise turn.
fn turn(incoming: (f64, f64), here: Point, cand: Point) -> f64 {
    let out = (cand.x - here.x, cand.y - here.y);
    // Angle of the outgoing edge relative to carrying straight on.
    let back = (-incoming.0, -incoming.1);
    let cross = back.0 * out.1 - back.1 * out.0;
    let dot = back.0 * out.0 + back.1 * out.1;
    let mut a = cross.atan2(dot);
    if a <= 0.0 {
        a += std::f64::consts::TAU;
    }
    a
}

fn key(p: Point, tol: f64) -> Key {
    ((p.x / tol).round() as i64, (p.y / tol).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outline::Polygon;

    fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> (Point, Point) {
        (Point::new(x0, y0), Point::new(x1, y1))
    }

    fn area(ring: &[Point]) -> f64 {
        Polygon {
            points: ring.to_vec(),
        }
        .area_mm2()
    }

    #[test]
    fn four_loose_segments_become_a_square() {
        let segs = [
            seg(0.0, 0.0, 10.0, 0.0),
            seg(10.0, 0.0, 10.0, 10.0),
            seg(10.0, 10.0, 0.0, 10.0),
            seg(0.0, 10.0, 0.0, 0.0),
        ];
        let mut w = Vec::new();
        let pieces = compose(&segs, &mut w);
        assert_eq!(pieces.len(), 1);
        assert!((area(&pieces[0].ring) - 100.0).abs() < 1e-6);
    }

    /// Segments given in scrambled order and direction, as exporters do.
    #[test]
    fn order_and_direction_do_not_matter() {
        let segs = [
            seg(10.0, 10.0, 0.0, 10.0),
            seg(0.0, 0.0, 0.0, 10.0),
            seg(10.0, 0.0, 0.0, 0.0),
            seg(10.0, 0.0, 10.0, 10.0),
        ];
        let mut w = Vec::new();
        let pieces = compose(&segs, &mut w);
        assert_eq!(pieces.len(), 1);
        assert!((area(&pieces[0].ring) - 100.0).abs() < 1e-6);
    }

    /// An interior division must not become the boundary: the silhouette of
    /// two squares sharing a wall is the enclosing rectangle.
    #[test]
    fn an_interior_wall_is_swallowed() {
        let segs = [
            seg(0.0, 0.0, 20.0, 0.0),
            seg(20.0, 0.0, 20.0, 10.0),
            seg(20.0, 10.0, 0.0, 10.0),
            seg(0.0, 10.0, 0.0, 0.0),
            // The shared wall down the middle.
            seg(10.0, 0.0, 10.0, 10.0),
        ];
        let mut w = Vec::new();
        let pieces = compose(&segs, &mut w);
        assert_eq!(pieces.len(), 1, "{} pieces", pieces.len());
        assert!(
            (area(&pieces[0].ring) - 200.0).abs() < 1e-6,
            "area {}",
            area(&pieces[0].ring)
        );
    }

    /// Two segments crossing mid-span must be cut at the crossing, otherwise
    /// the graph has no vertex there and the walk cannot turn.
    #[test]
    fn crossing_segments_are_split() {
        let segs = [
            seg(0.0, 0.0, 10.0, 10.0),
            seg(0.0, 10.0, 10.0, 0.0),
            seg(0.0, 0.0, 0.0, 10.0),
            seg(10.0, 0.0, 10.0, 10.0),
            seg(0.0, 0.0, 10.0, 0.0),
            seg(0.0, 10.0, 10.0, 10.0),
        ];
        let mut w = Vec::new();
        let pieces = compose(&segs, &mut w);
        assert_eq!(pieces.len(), 1);
        assert!(
            (area(&pieces[0].ring) - 100.0).abs() < 1e-6,
            "area {}",
            area(&pieces[0].ring)
        );
    }

    /// Exactly coincident duplicates — what a flattened extrusion produces —
    /// must collapse rather than double the boundary.
    #[test]
    fn coincident_duplicates_collapse() {
        let mut segs = vec![
            seg(0.0, 0.0, 10.0, 0.0),
            seg(10.0, 0.0, 10.0, 10.0),
            seg(10.0, 10.0, 0.0, 10.0),
            seg(0.0, 10.0, 0.0, 0.0),
        ];
        let dup = segs.clone();
        segs.extend(dup);
        let mut w = Vec::new();
        let pieces = compose(&segs, &mut w);
        assert_eq!(pieces.len(), 1);
        assert!((area(&pieces[0].ring) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn a_dangling_spur_is_pruned() {
        let segs = [
            seg(0.0, 0.0, 10.0, 0.0),
            seg(10.0, 0.0, 10.0, 10.0),
            seg(10.0, 10.0, 0.0, 10.0),
            seg(0.0, 10.0, 0.0, 0.0),
            // A whisker sticking out of the corner.
            seg(10.0, 10.0, 15.0, 15.0),
        ];
        let mut w = Vec::new();
        let pieces = compose(&segs, &mut w);
        assert_eq!(pieces.len(), 1);
        assert!((area(&pieces[0].ring) - 100.0).abs() < 1e-6);
        assert!(w.iter().any(|m| m.contains("dangling")), "{w:?}");
    }

    #[test]
    fn disjoint_shapes_become_separate_pieces() {
        let segs = [
            seg(0.0, 0.0, 10.0, 0.0),
            seg(10.0, 0.0, 10.0, 10.0),
            seg(10.0, 10.0, 0.0, 10.0),
            seg(0.0, 10.0, 0.0, 0.0),
            seg(30.0, 0.0, 40.0, 0.0),
            seg(40.0, 0.0, 40.0, 10.0),
            seg(40.0, 10.0, 30.0, 10.0),
            seg(30.0, 10.0, 30.0, 0.0),
        ];
        let mut w = Vec::new();
        let mut pieces = compose(&segs, &mut w);
        pieces.sort_by(|a, b| area(&a.ring).partial_cmp(&area(&b.ring)).unwrap());
        assert_eq!(pieces.len(), 2);
        for p in &pieces {
            assert!((area(&p.ring) - 100.0).abs() < 1e-6);
        }
    }
}
