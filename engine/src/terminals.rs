//! Terminal layout: the solder pads live in a small pocket at the left
//! edge, centered on the board's horizontal centerline, so the fill can run
//! dense above, below, and to the right of them.
//!
//! ```text
//!   ┌───────────────────────────────┐
//!   │ ┆ ═══════════ rows ═════════  │   rows outside the band reach the
//!   │ ┆ ═══════════════════════════ │   thin lane corridor (┆)
//!   │ ┆ ┌────┐.  ═══════ rows ════  │ ┐
//!   │ ┆ │pad A│.  ═════════════════ │ │ pocket band: rows stop right of
//!   │ ┆ ├────┤.  ═════════════════  │ │ the pocket
//!   │ ┆ │pad B│.  ═════════════════ │ ┘
//!   │ ┆ └────┘.  ════════════════   │
//!   │ ┆ ═══════════════════════════ │
//!   └───────────────────────────────┘
//! ```
//!
//! Two feed lanes: the **left lane** (┆) runs the full height just left of
//! the pads — serpentine-family patterns use it to reach their top/bottom
//! corner terminals. The **right lane** (.) lives inside the pocket —
//! spiral/concentric patterns whose terminals exit at the centerline use it
//! without ever crossing pad copper.

use crate::fills::Reserve;
use crate::outline::Polygon;
use crate::{EngineError, PathSeg, Point};

/// A rectangular solder pad, center + size, board mm.
#[derive(Debug, Clone, Copy)]
pub struct PadRect {
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
}

impl PadRect {
    pub fn center(&self) -> Point {
        Point::new(self.cx, self.cy)
    }

    /// Corner ring, clockwise from the top-left in the y-down frame.
    pub fn ring(&self) -> Vec<Point> {
        let (x0, y0) = (self.cx - self.w / 2.0, self.cy - self.h / 2.0);
        let (x1, y1) = (self.cx + self.w / 2.0, self.cy + self.h / 2.0);
        vec![
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ]
    }

    /// The same rectangle grown by `d` on every side (mask openings).
    pub fn grown(&self, d: f64) -> PadRect {
        PadRect {
            cx: self.cx,
            cy: self.cy,
            w: self.w + 2.0 * d,
            h: self.h + 2.0 * d,
        }
    }
}

/// A solder terminal's copper. Auto-placed designs get a rectangle; designs
/// driven by a DXF selection get the user's own polygon, so the pad is
/// whatever shape they drew.
#[derive(Debug, Clone)]
pub enum Pad {
    Rect(PadRect),
    Poly(Polygon),
}

impl Pad {
    /// Where a feed trace should meet the pad.
    pub fn center(&self) -> Point {
        match self {
            Pad::Rect(r) => r.center(),
            Pad::Poly(p) => p.centroid(),
        }
    }

    pub fn ring(&self) -> Vec<Point> {
        match self {
            Pad::Rect(r) => r.ring(),
            Pad::Poly(p) => p.points.clone(),
        }
    }

    pub fn bbox(&self) -> (Point, Point) {
        match self {
            Pad::Rect(r) => (
                Point::new(r.cx - r.w / 2.0, r.cy - r.h / 2.0),
                Point::new(r.cx + r.w / 2.0, r.cy + r.h / 2.0),
            ),
            Pad::Poly(p) => p.bbox(),
        }
    }

    /// The pad outline grown by `d` on every side, for soldermask openings.
    /// Polygon pads are offset by scaling about the centroid, which is exact
    /// for convex tabs and slightly conservative for concave ones.
    pub fn grown_ring(&self, d: f64) -> Vec<Point> {
        match self {
            Pad::Rect(r) => r.grown(d).ring(),
            Pad::Poly(p) => {
                let c = p.centroid();
                p.points
                    .iter()
                    .map(|v| {
                        let (dx, dy) = (v.x - c.x, v.y - c.y);
                        let len = (dx * dx + dy * dy).sqrt();
                        if len < 1e-9 {
                            *v
                        } else {
                            Point::new(v.x + d * dx / len, v.y + d * dy / len)
                        }
                    })
                    .collect()
            }
        }
    }
}

/// Which lane a pattern's feeds route through.
#[derive(Debug, Clone, Copy)]
pub enum Lane {
    /// Full-height corridor left of the pads.
    Left,
    /// Pocket-internal corridor right of the pads.
    Right,
}

pub struct TerminalPlan {
    /// What the fill must keep clear.
    pub reserve: Reserve,
    /// [pad A (upper), pad B (lower)], symmetric about the centerline.
    pub pads: [PadRect; 2],
    x_left: f64,
    x_right: f64,
    /// Two nested lanes inside the left corridor, one routing pitch apart,
    /// for patterns whose two path ends come back adjacent to each other.
    lane_inner: f64,
    lane_outer: f64,
}

pub fn layout(
    bbox: (Point, Point),
    inset_mm: f64,
    trace_width_mm: f64,
    gap_mm: f64,
    pad_size_mm: f64,
    warnings: &mut Vec<String>,
) -> Result<TerminalPlan, EngineError> {
    let (min, max) = bbox;

    let mut s = pad_size_mm;
    let floor = (trace_width_mm * 2.0).max(1.0);
    if s < floor {
        warnings.push(format!(
            "solder pad size raised from {s:.2} mm to {floor:.2} mm so the \
             pad stays solderable"
        ));
        s = floor;
    }
    let pad_w = 1.6 * s;
    let pad_h = s;
    let pad_gap = (2.0 * gap_mm).max(0.8);
    // Keepout around the pads: two routing pitches (so even a turn arc plus
    // its trace body clears comfortably), never under 1.2 mm.
    let clearance = (2.0 * (trace_width_mm + gap_mm)).max(1.2);

    let lane = (2.0 * (trace_width_mm + gap_mm)).max(1.2);
    let pocket_w = 2.0 * lane + pad_w;

    let usable_w = max.x - min.x - 2.0 * inset_mm;
    let usable_h = max.y - min.y - 2.0 * inset_mm;
    if pocket_w > usable_w / 2.0 {
        return Err(EngineError::OutlineTooSmall(format!(
            "terminal pocket ({pocket_w:.1} mm) doesn't leave room to route; \
             outline is only {usable_w:.1} mm wide inside margins"
        )));
    }
    let stack = 2.0 * pad_h + pad_gap;
    if stack + 2.0 * clearance > usable_h {
        return Err(EngineError::OutlineTooSmall(format!(
            "two {pad_h:.1} mm pads don't fit the {usable_h:.1} mm outline \
             height; shrink the solder pad size"
        )));
    }

    let x0 = min.x + inset_mm;
    let cy = (min.y + max.y) / 2.0;
    let px0 = x0 + lane;
    let pcx = px0 + pad_w / 2.0;

    // Two lanes stepping back from the fill's left edge one routing pitch at a
    // time. A pitch is by definition the spacing the fab can hold between
    // adjacent traces, so this needs no extra clearance rule. Both stay right
    // of `x0` because `lane` is itself at least two pitches wide.
    let pitch = trace_width_mm + gap_mm;
    let lane_inner = px0 - pitch;
    let lane_outer = px0 - 2.0 * pitch;
    debug_assert!(lane_outer >= x0 - 1e-9, "nested lanes escape the corridor");

    Ok(TerminalPlan {
        reserve: Reserve {
            lane_edge: px0,
            pocket_x1: px0 + pad_w + lane,
            pocket_y0: cy - stack / 2.0 - clearance,
            pocket_y1: cy + stack / 2.0 + clearance,
        },
        pads: [
            PadRect {
                cx: pcx,
                cy: cy - (pad_gap + pad_h) / 2.0,
                w: pad_w,
                h: pad_h,
            },
            PadRect {
                cx: pcx,
                cy: cy + (pad_gap + pad_h) / 2.0,
                w: pad_w,
                h: pad_h,
            },
        ],
        x_left: x0 + lane / 2.0,
        x_right: px0 + pad_w + lane / 2.0,
        lane_inner,
        lane_outer,
    })
}

impl TerminalPlan {
    fn lane_x(&self, lane: Lane) -> f64 {
        match lane {
            Lane::Left => self.x_left,
            Lane::Right => self.x_right,
        }
    }

    /// Path from pad A's center via the lane to the fill's start terminal.
    pub fn feed_start(&self, lane: Lane, t: Point) -> Vec<PathSeg> {
        let a = self.pads[0];
        let x = self.lane_x(lane);
        segments(&[
            Point::new(a.cx, a.cy),
            Point::new(x, a.cy),
            Point::new(x, t.y),
            t,
        ])
    }

    /// Path from the fill's end terminal via the lane into pad B.
    pub fn feed_end(&self, lane: Lane, t: Point) -> Vec<PathSeg> {
        let b = self.pads[1];
        let x = self.lane_x(lane);
        segments(&[
            t,
            Point::new(x, t.y),
            Point::new(x, b.cy),
            Point::new(b.cx, b.cy),
        ])
    }

    /// Feeds for a pattern whose two path ends come back **adjacent** to each
    /// other rather than at opposite corners — the bifilar counterflow, whose
    /// whole point is that its two arms finish side by side.
    ///
    /// [`feed_start`](Self::feed_start) and [`feed_end`](Self::feed_end) cannot
    /// serve that case: they share one lane `x`, so with both terminals a
    /// single pitch apart the two runs end up **collinear**, sitting on top of
    /// each other for the whole height of the board rather than merely
    /// touching.
    ///
    /// The fix is to nest the two routes instead of overlapping them. The
    /// terminal farther from the pads takes the outer lane and the farther
    /// pad; the nearer terminal takes the inner lane and the nearer pad. One
    /// route then encloses the other without ever meeting it:
    ///
    /// ```text
    ///   t_far  ─────────────────┐   outer lane
    ///   t_near ───────────┐     │   inner lane
    ///                     │     │
    ///        pad_near ────┘     │
    ///        pad_far  ──────────┘
    /// ```
    ///
    /// Returns `(feed_start, feed_end)`: the first runs pad → `t_start`, the
    /// second `t_end` → pad, so they bracket the fill in trace order.
    pub fn feeds_adjacent(&self, t_start: Point, t_end: Point) -> (Vec<PathSeg>, Vec<PathSeg>) {
        let pad_mid = (self.pads[0].cy + self.pads[1].cy) / 2.0;
        let t_mid = (t_start.y + t_end.y) / 2.0;

        // Which terminal is farther out, and which pad is farther away. Those
        // two go together on the outer lane; nesting is what avoids crossings.
        let start_is_far = (t_start.y - pad_mid).abs() >= (t_end.y - pad_mid).abs();
        let pad0_is_far = (self.pads[0].cy - t_mid).abs() >= (self.pads[1].cy - t_mid).abs();

        let (t_far, t_near) = if start_is_far {
            (t_start, t_end)
        } else {
            (t_end, t_start)
        };
        let (pad_far, pad_near) = if pad0_is_far {
            (self.pads[0], self.pads[1])
        } else {
            (self.pads[1], self.pads[0])
        };

        // pad → lane → terminal, as three orthogonal hops.
        let leg = |pad: PadRect, x: f64, t: Point| {
            [
                Point::new(pad.cx, pad.cy),
                Point::new(x, pad.cy),
                Point::new(x, t.y),
                t,
            ]
        };
        let far_leg = leg(pad_far, self.lane_outer, t_far);
        let near_leg = leg(pad_near, self.lane_inner, t_near);

        // `feed_start` must arrive at t_start; `feed_end` must leave t_end, so
        // whichever leg belongs to t_end is walked backwards.
        let (to_start, to_end) = if start_is_far {
            (far_leg, near_leg)
        } else {
            (near_leg, far_leg)
        };
        let mut reversed = to_end;
        reversed.reverse();
        (segments(&to_start), segments(&reversed))
    }
}

/// Consecutive points → line segments, skipping degenerate hops.
fn segments(pts: &[Point]) -> Vec<PathSeg> {
    pts.windows(2)
        .filter(|w| w[0].dist(&w[1]) > 1e-9)
        .map(|w| PathSeg::Line { a: w[0], b: w[1] })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> TerminalPlan {
        layout(
            (Point::new(0.0, 0.0), Point::new(100.0, 20.0)),
            0.6,
            0.3,
            0.15,
            2.5,
            &mut Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn pads_are_symmetric_about_centerline() {
        let p = plan();
        let cy = 10.0;
        assert!((cy - p.pads[0].cy - (p.pads[1].cy - cy)).abs() < 1e-9);
        assert_eq!(p.pads[0].cx, p.pads[1].cx);
        assert_eq!(p.pads[0].w, p.pads[1].w);
        let gap = (p.pads[1].cy - p.pads[1].h / 2.0) - (p.pads[0].cy + p.pads[0].h / 2.0);
        assert!(gap > 0.0 && gap < p.pads[0].h, "gap {gap}");
    }

    #[test]
    fn pocket_band_hugs_the_pads() {
        let p = plan();
        // The band covers both pads plus clearance and no more than ~2 mm
        // beyond them.
        let pads_top = p.pads[0].cy - p.pads[0].h / 2.0;
        let pads_bot = p.pads[1].cy + p.pads[1].h / 2.0;
        assert!(p.reserve.pocket_y0 < pads_top && p.reserve.pocket_y0 > pads_top - 2.0);
        assert!(p.reserve.pocket_y1 > pads_bot && p.reserve.pocket_y1 < pads_bot + 2.0);
        // Rows outside the band may come much further left than inside it.
        assert!(p.reserve.lane_edge < p.reserve.pocket_x1);
        assert!(p.reserve.left_bound(1.0) < p.reserve.left_bound(10.0));
    }

    #[test]
    fn left_feeds_connect_pads_to_corner_terminals() {
        let p = plan();
        let t_start = Point::new(p.reserve.lane_edge, 1.0);
        let t_end = Point::new(p.reserve.lane_edge, 19.0);
        let fs = p.feed_start(Lane::Left, t_start);
        assert!(
            fs.first()
                .unwrap()
                .start()
                .dist(&Point::new(p.pads[0].cx, p.pads[0].cy))
                < 1e-9
        );
        assert!(fs.last().unwrap().end().dist(&t_start) < 1e-9);
        let fe = p.feed_end(Lane::Left, t_end);
        assert!(fe.first().unwrap().start().dist(&t_end) < 1e-9);
        assert!(
            fe.last()
                .unwrap()
                .end()
                .dist(&Point::new(p.pads[1].cx, p.pads[1].cy))
                < 1e-9
        );
        // Left-lane verticals stay left of the pads.
        for seg in fs.iter().chain(fe.iter()) {
            let (a, b) = (seg.start(), seg.end());
            if (a.x - b.x).abs() < 1e-9 {
                assert!(a.x < p.pads[0].cx - p.pads[0].w / 2.0);
            }
        }
    }

    #[test]
    fn right_feeds_stay_clear_of_pad_copper() {
        let p = plan();
        // Center-exit terminals like the concentric fill's.
        let t_start = Point::new(p.reserve.pocket_x1, 9.0);
        let t_end = Point::new(p.reserve.pocket_x1 + 8.0, 10.0);
        let pad_right = p.pads[0].cx + p.pads[0].w / 2.0;
        for seg in p
            .feed_start(Lane::Right, t_start)
            .iter()
            .chain(p.feed_end(Lane::Right, t_end).iter())
        {
            let (a, b) = (seg.start(), seg.end());
            // Vertical runs sit right of the pads; horizontal runs at pad
            // center height only touch their own pad.
            if (a.x - b.x).abs() < 1e-9 {
                assert!(a.x > pad_right, "vertical at {} crosses pads", a.x);
            }
        }
    }

    #[test]
    fn tiny_pad_size_gets_floored() {
        let mut w = Vec::new();
        let p = layout(
            (Point::new(0.0, 0.0), Point::new(100.0, 20.0)),
            0.6,
            0.3,
            0.15,
            0.1,
            &mut w,
        )
        .unwrap();
        assert!(p.pads[0].h >= 1.0);
        assert_eq!(w.len(), 1);
    }
}
