//! Concentric fill: repeated inward offsets of the outline (via
//! cavalier_contours), spliced into one continuous path through a narrow
//! horizontal channel on the left. Rings follow the outline shape, so this
//! pattern has the best boundary coverage for irregular boards.
//!
//! The channel sits at the board's vertical center: each ring is cut where
//! it crosses the channel, and the resulting open arcs are chained with
//! short connectors that hug the channel's top and bottom edges — leaving
//! the channel centerline clear as the exit corridor for the inner
//! terminal's feed run.

use cavalier_contours::polyline::{BooleanOp, PlineSource, PlineSourceMut, Polyline};

use super::Reserve;
use crate::{outline::Polygon, EngineError, PathSeg, Point};

pub fn fill(
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
    reserve: Reserve,
    warnings: &mut Vec<String>,
) -> Result<Vec<PathSeg>, EngineError> {
    let (min, max) = outline.bbox();
    let cy = (min.y + max.y) / 2.0;

    // Region = outline minus the solder-pad pocket, so the rings wrap the
    // pads and stay dense above and below them.
    let mut region = to_pline(outline);
    let notch_x1 = reserve.pocket_x1 - inset_mm;
    if notch_x1 > min.x {
        let notch_y0 = if reserve.pocket_y0.is_finite() {
            reserve.pocket_y0 + inset_mm
        } else {
            min.y - 1.0
        };
        let notch_y1 = if reserve.pocket_y1.is_finite() {
            reserve.pocket_y1 - inset_mm
        } else {
            max.y + 1.0
        };
        let mut pocket = Polyline::new_closed();
        pocket.add(min.x - 1.0, notch_y0, 0.0);
        pocket.add(notch_x1, notch_y0, 0.0);
        pocket.add(notch_x1, notch_y1, 0.0);
        pocket.add(min.x - 1.0, notch_y1, 0.0);
        ensure_ccw(&mut pocket);
        let result = region.boolean(&pocket, BooleanOp::Not);
        region = result
            .pos_plines
            .into_iter()
            .map(|r| r.pline)
            .max_by(|a, b| a.area().abs().partial_cmp(&b.area().abs()).unwrap())
            .ok_or_else(|| {
                EngineError::OutlineTooSmall(
                    "outline vanishes after reserving the terminal pocket".into(),
                )
            })?;
        ensure_ccw(&mut region);
    }

    // Generate rings by repeated inward offsets.
    let mut rings: Vec<Vec<Point>> = Vec::new();
    let mut split_rings = 0usize;
    for k in 0.. {
        let d = inset_mm + k as f64 * pitch_mm;
        let mut offs = region.parallel_offset(d);
        if offs.is_empty() {
            break;
        }
        if offs.len() > 1 {
            split_rings += 1;
            offs.sort_by(|a, b| b.area().abs().partial_cmp(&a.area().abs()).unwrap());
        }
        let ring = flatten(&offs[0]);
        if ring.len() < 4 || loop_area(&ring).abs() < 4.0 * pitch_mm * pitch_mm {
            break;
        }
        rings.push(ring);
        if k > 10_000 {
            break; // paranoia against a degenerate offset loop
        }
    }
    if split_rings > 0 {
        warnings.push(format!(
            "{split_rings} concentric ring depth(s) split into islands; only \
             the largest island was routed at each depth"
        ));
    }
    if rings.len() < 2 {
        return Err(EngineError::OutlineTooSmall(format!(
            "only {} concentric ring(s) fit at {pitch_mm:.2} mm pitch",
            rings.len()
        )));
    }

    // Cut each ring at the left-side channel and chain them. The channel
    // is a full pitch tall on each side of the centerline so the two feed
    // runs (terminal A at cy−p, terminal B at cy) keep their clearance.
    let w = pitch_mm;
    let mut path: Vec<PathSeg> = Vec::new();
    let mut prev_exit: Option<(Point, bool)> = None; // (point, exited_at_top)
    for (k, ring) in rings.iter().enumerate() {
        // Innermost rings can collapse to slivers that never reach the
        // channel; stop chaining there. Only the outer ring is required.
        let (up, lo) = match (
            leftmost_crossing(ring, cy - w),
            leftmost_crossing(ring, cy + w),
        ) {
            (Some(up), Some(lo)) => (up, lo),
            _ if k == 0 => return Err(ring_cut_err(k)),
            _ => break,
        };
        let arc = open_ring_between(ring, up, lo);
        // Even rings run top→bottom, odd rings bottom→top, so consecutive
        // connectors alternate between the channel's top and bottom edges.
        let (entry, body_start) = if k % 2 == 0 {
            (arc.0, arc.1.clone())
        } else {
            let mut rev = arc.1.clone();
            rev.reverse();
            (arc.2, rev)
        };
        if let Some((prev, _)) = prev_exit {
            path.push(PathSeg::Line { a: prev, b: entry });
        }
        for pair in body_start.windows(2) {
            if pair[0].dist(&pair[1]) > 1e-9 {
                path.push(PathSeg::Line {
                    a: pair[0],
                    b: pair[1],
                });
            }
        }
        let exit = *body_start.last().unwrap();
        prev_exit = Some((exit, k % 2 == 1));
    }

    // Jog the inner terminal onto the channel centerline so its feed run
    // exits straight down the clear corridor.
    let (last_exit, _) = prev_exit.unwrap();
    let t_b = Point::new(last_exit.x, cy);
    path.push(PathSeg::Line {
        a: last_exit,
        b: t_b,
    });

    Ok(path)
}

fn ring_cut_err(k: usize) -> EngineError {
    EngineError::OutlineTooSmall(format!(
        "concentric ring {k} never crosses the exit channel; outline is too \
         convoluted for this pattern"
    ))
}

fn to_pline(outline: &Polygon) -> Polyline {
    let mut pl = Polyline::new_closed();
    for p in &outline.points {
        pl.add(p.x, p.y, 0.0);
    }
    ensure_ccw(&mut pl);
    pl
}

/// cavalier_contours offsets inward for positive values only when the
/// polyline has positive (CCW) orientation.
fn ensure_ccw(pl: &mut Polyline) {
    if pl.area() < 0.0 {
        pl.invert_direction_mut();
    }
}

fn flatten(pl: &Polyline) -> Vec<Point> {
    let flat = pl.arcs_to_approx_lines(0.05).unwrap_or_else(|| pl.clone());
    (0..flat.vertex_count())
        .map(|i| {
            let v = flat.at(i);
            Point::new(v.x, v.y)
        })
        .collect()
}

fn loop_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    let mut acc = 0.0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        acc += a.x * b.y - b.x * a.y;
    }
    acc / 2.0
}

/// Position of a crossing on the ring: edge index + interpolated point.
#[derive(Clone, Copy)]
struct Crossing {
    edge: usize,
    point: Point,
}

/// The ring's leftmost crossing of the horizontal line at `y`.
fn leftmost_crossing(ring: &[Point], y: f64) -> Option<Crossing> {
    let n = ring.len();
    let mut best: Option<Crossing> = None;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        if (a.y <= y && b.y > y) || (b.y <= y && a.y > y) {
            let t = (y - a.y) / (b.y - a.y);
            let p = Point::new(a.x + t * (b.x - a.x), y);
            if best.is_none_or(|c| p.x < c.point.x) {
                best = Some(Crossing { edge: i, point: p });
            }
        }
    }
    best
}

/// Split the ring at the two crossings and return
/// (upper point, the long way around as a point list, lower point).
fn open_ring_between(ring: &[Point], up: Crossing, lo: Crossing) -> (Point, Vec<Point>, Point) {
    let n = ring.len();
    // Walk forward from `from`'s edge to `to`'s edge collecting vertices.
    let collect = |from: Crossing, to: Crossing| -> Vec<Point> {
        let mut pts = vec![from.point];
        let mut i = (from.edge + 1) % n;
        loop {
            pts.push(ring[i]);
            if i == to.edge {
                break;
            }
            i = (i + 1) % n;
            if pts.len() > n + 2 {
                break; // safety
            }
        }
        pts.push(to.point);
        pts
    };

    if up.edge == lo.edge {
        // Both crossings on one edge: the notch is the piece of that edge
        // between them. The long way starts at whichever crossing is
        // further along the edge and walks the full loop back to the other.
        let edge_start = ring[up.edge];
        let (first, second, up_is_first) =
            if edge_start.dist(&up.point) <= edge_start.dist(&lo.point) {
                (up, lo, true)
            } else {
                (lo, up, false)
            };
        let pts = collect(second, first);
        return if up_is_first {
            // pts runs lo→…→up; reorient to up→…→lo.
            let mut rev = pts;
            rev.reverse();
            (up.point, rev, lo.point)
        } else {
            (up.point, pts, lo.point)
        };
    }

    let fwd = collect(up, lo);
    let bwd = collect(lo, up);
    let fwd_len: f64 = fwd.windows(2).map(|w| w[0].dist(&w[1])).sum();
    let bwd_len: f64 = bwd.windows(2).map(|w| w[0].dist(&w[1])).sum();
    // Keep the long way around — the short way is the notch through the
    // channel that we're removing.
    if fwd_len >= bwd_len {
        (up.point, fwd, lo.point)
    } else {
        let mut rev = bwd;
        rev.reverse();
        (up.point, rev, lo.point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fills::assert_path_well_formed;

    fn rect(w: f64, h: f64) -> Polygon {
        Polygon {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(w, 0.0),
                Point::new(w, h),
                Point::new(0.0, h),
            ],
        }
    }

    #[test]
    fn rings_chain_into_single_connected_path() {
        let mut w = Vec::new();
        let path = fill(&rect(60.0, 20.0), 1.0, 0.6, Reserve::column(6.6), &mut w).unwrap();
        assert_path_well_formed(&path, 0.0, 0.0, 60.0, 20.0);
        let start = path.first().unwrap().start();
        let end = path.last().unwrap().end();
        // Terminal A on the outer ring near the pocket, above the centerline.
        assert!(start.x < 10.0, "start x {}", start.x);
        assert!((start.y - (10.0 - 1.0)).abs() < 0.1, "start y {}", start.y);
        // Terminal B on the channel centerline, inward of A.
        assert!((end.y - 10.0).abs() < 1e-6, "end y {}", end.y);
        assert!(end.x > start.x, "end should be inward of start");
    }

    #[test]
    fn channel_centerline_stays_clear() {
        // No copper may cross the corridor y=cy between the zone edge and
        // the inner terminal (the feed run needs it).
        let path = fill(
            &rect(60.0, 20.0),
            1.0,
            0.6,
            Reserve::column(6.6),
            &mut Vec::new(),
        )
        .unwrap();
        let cy = 10.0;
        let end = path.last().unwrap().end();
        for seg in &path[..path.len() - 1] {
            let (a, b) = (seg.start(), seg.end());
            if (a.y - cy) * (b.y - cy) < 0.0 {
                let t = (cy - a.y) / (b.y - a.y);
                let x = a.x + t * (b.x - a.x);
                assert!(
                    x > end.x - 1e-6,
                    "copper crosses the exit corridor at x={x:.2}"
                );
            }
        }
    }

    #[test]
    fn ring_count_matches_pitch() {
        // 20 mm tall rect, inset 0.6, pitch 1.0 → half-height 9.4 → ~9 rings.
        let path = fill(
            &rect(60.0, 20.0),
            1.0,
            0.6,
            Reserve::none(),
            &mut Vec::new(),
        )
        .unwrap();
        // Count crossings of a vertical line at x=30 above center: one per ring.
        let crossings = path
            .iter()
            .filter(|s| {
                let (a, b) = (s.start(), s.end());
                (a.x - 30.0) * (b.x - 30.0) <= 0.0
                    && (a.x - b.x).abs() > 1e-12
                    && (a.y + b.y) / 2.0 < 10.0
            })
            .count();
        assert!(
            (7..=10).contains(&crossings),
            "expected ~9 ring crossings, got {crossings}"
        );
    }

    #[test]
    fn tiny_outline_rejected() {
        assert!(fill(&rect(4.0, 3.0), 1.0, 0.6, Reserve::none(), &mut Vec::new()).is_err());
    }
}
