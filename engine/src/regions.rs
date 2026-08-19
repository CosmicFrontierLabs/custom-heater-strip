//! Multi-region routing: fill several heater polygons and chain them in
//! series so the whole board is one electrical element between two tabs.
//!
//! ```text
//!    ┌─ tab in                        regions are filled independently,
//!    ▼                                then joined end to end:
//!   ╔═╗  ┌────────┐   ┌────────┐
//!   ║ ╠══╪════════╪═══╪════════╪══╗
//!   ╚═╝  │ region │   │ region │  ║  ┌────────┐
//!        │   0    │   │   1    │  ╚══╪════════╪═╗
//!        └────────┘   └────────┘     │region 2│ ║
//!                                    └────────┘ ▼
//!                                            tab out
//! ```
//!
//! Two problems have to be solved for the joins to be manufacturable.
//!
//! **Where a region's terminals land.** A link that has to reach around a
//! region to find its terminal would cross the fill that region just laid
//! down, shorting it. Two things prevent that:
//!
//! - A region whose neighbours lie on *opposite* sides is filled with its two
//!   ends at opposite edges ([`Terminals::OppositeSides`]), so the run coming
//!   in and the run going out each meet an edge that faces where they came
//!   from. A region whose neighbours are both off the *same* side keeps its
//!   ends together and aims between them.
//! - The region is then rotated by a multiple of 90° before filling so the
//!   entry edge faces the incoming run, and the routed path is rotated back.
//!   Only rotations are used — never reflections — because a reflection would
//!   invert every arc's sweep direction.
//!
//! Only the plain serpentine can split its terminals; the other patterns
//! always deliver both ends together, so a chain of them may still need a
//! link that crosses copper. That is detected and warned about rather than
//! silently shipped.
//!
//! **Keeping copper off the pads.** A tab that overlaps a heater region is
//! cut out of that region (plus clearance) before filling, so the pattern
//! wraps around the pad instead of shorting into it.

use shared::{CornerStyle, FillKind};

use crate::fills::{self, Reserve, Terminals};
use crate::outline::Polygon;
use crate::terminals::Pad;
use crate::{EngineError, PathSeg, Point};

/// One of the four rotations that can orient a region before filling.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Quarter {
    R0,
    R90,
    R180,
    R270,
}

impl Quarter {
    const ALL: [Quarter; 4] = [Quarter::R0, Quarter::R90, Quarter::R180, Quarter::R270];

    /// Rotate a point about the origin.
    fn apply(&self, p: Point) -> Point {
        match self {
            Quarter::R0 => p,
            Quarter::R90 => Point::new(-p.y, p.x),
            Quarter::R180 => Point::new(-p.x, -p.y),
            Quarter::R270 => Point::new(p.y, -p.x),
        }
    }

    fn inverse(&self) -> Quarter {
        match self {
            Quarter::R0 => Quarter::R0,
            Quarter::R90 => Quarter::R270,
            Quarter::R180 => Quarter::R180,
            Quarter::R270 => Quarter::R90,
        }
    }

    fn rotate_polygon(&self, poly: &Polygon) -> Polygon {
        Polygon {
            points: poly.points.iter().map(|p| self.apply(*p)).collect(),
        }
    }

    /// Rotate a routed path. Rotation preserves handedness, so arc sweep
    /// directions carry over unchanged.
    fn rotate_path(&self, path: &[PathSeg]) -> Vec<PathSeg> {
        path.iter()
            .map(|seg| match seg {
                PathSeg::Line { a, b } => PathSeg::Line {
                    a: self.apply(*a),
                    b: self.apply(*b),
                },
                PathSeg::Arc { a, b, center, ccw } => PathSeg::Arc {
                    a: self.apply(*a),
                    b: self.apply(*b),
                    center: self.apply(*center),
                    ccw: *ccw,
                },
            })
            .collect()
    }
}

/// A heater region prepared for routing.
pub struct Region {
    /// The polygon to fill, with tab keepouts already cut out.
    pub polygon: Polygon,
    /// Where this region's entry terminal should face.
    target: Point,
    /// Whether the two terminals should sit together or at opposite ends.
    terminals: Terminals,
}

/// Everything the chainer needs to route a design.
pub struct Chain {
    /// The regions in series order.
    pub regions: Vec<Region>,
}

/// Order regions into a series chain and cut tab keepouts out of them.
///
/// `clearance_mm` is how far the fill must stay from pad copper.
pub fn plan(
    heaters: &[Polygon],
    tab_in: &Pad,
    tab_out: &Pad,
    clearance_mm: f64,
    warnings: &mut Vec<String>,
) -> Result<Chain, EngineError> {
    if heaters.is_empty() {
        return Err(EngineError::BadGeometry(
            "no heater regions selected".into(),
        ));
    }

    // Walk the regions greedily from the input tab: at each step take the
    // nearest region not yet used. This keeps the links short and, for the
    // common case of regions in a row, visits them in the obvious order.
    let mut remaining: Vec<usize> = (0..heaters.len()).collect();
    let mut order: Vec<usize> = Vec::with_capacity(heaters.len());
    let mut cursor = tab_in.center();
    while !remaining.is_empty() {
        let (slot, &idx) = remaining
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = heaters[**a].centroid().dist(&cursor);
                let db = heaters[**b].centroid().dist(&cursor);
                da.partial_cmp(&db).unwrap()
            })
            .expect("remaining is non-empty");
        order.push(idx);
        cursor = heaters[idx].centroid();
        remaining.remove(slot);
    }

    // Each region's terminals should face whatever it connects to: the tab at
    // either end of the chain, the neighbouring region in the middle. A lone
    // region connects to both tabs, so it aims between them.
    let n = order.len();
    let mut regions = Vec::with_capacity(n);
    for (pos, &idx) in order.iter().enumerate() {
        let before = if pos == 0 {
            tab_in.center()
        } else {
            heaters[order[pos - 1]].centroid()
        };
        let after = if pos + 1 == n {
            tab_out.center()
        } else {
            heaters[order[pos + 1]].centroid()
        };

        // If what this region connects to sits on roughly opposite sides of
        // it, the fill must enter one side and leave the other; otherwise a
        // link would have to cross the fill to get out. When both neighbours
        // are off the same side, keep the ends together and aim between them.
        let c = heaters[idx].centroid();
        let (v_in, v_out) = (
            Point::new(before.x - c.x, before.y - c.y),
            Point::new(after.x - c.x, after.y - c.y),
        );
        let opposed = v_in.x * v_out.x + v_in.y * v_out.y < 0.0;
        let (terminals, target) = if opposed {
            // Orient toward the incoming side; the exit lands opposite it.
            (Terminals::OppositeSides, before)
        } else {
            (
                Terminals::SameSide,
                Point::new((before.x + after.x) / 2.0, (before.y + after.y) / 2.0),
            )
        };

        let polygon = cut_tabs(&heaters[idx], [tab_in, tab_out], clearance_mm, warnings)?;
        regions.push(Region {
            polygon,
            target,
            terminals,
        });
    }

    Ok(Chain { regions })
}

/// Cut any tab that overlaps this region out of it, so the fill wraps the pad.
fn cut_tabs(
    region: &Polygon,
    tabs: [&Pad; 2],
    clearance_mm: f64,
    warnings: &mut Vec<String>,
) -> Result<Polygon, EngineError> {
    let mut out = region.clone();
    for tab in tabs {
        let keepout = Polygon {
            points: tab.grown_ring(clearance_mm),
        };
        if !out.overlaps(&keepout) {
            continue;
        }
        let cut = out
            .subtract(&channel_keepout(&out, &keepout))
            .ok_or_else(|| {
                EngineError::BadGeometry(
                    "a solder tab covers its whole heater region; move the tab or \
                 pick a larger region"
                        .into(),
                )
            })?;
        if cut.pieces > 1 {
            warnings.push(format!(
                "a solder tab splits its heater region into {} pieces; only the \
                 largest is filled. Move the tab nearer an edge to keep full \
                 coverage.",
                cut.pieces
            ));
        }
        out = cut.largest;
    }
    Ok(out)
}

/// Keepout to cut for a tab: its bounding box, extended out past the nearest
/// side of the region.
///
/// The extension is what makes this correct. Subtracting a tab that sits
/// wholly inside a region would leave an enclosed hole, and the fill patterns
/// route one continuous path through a simply-connected polygon — they cannot
/// wrap a hole, and the boolean's positive contour would come back as the
/// untouched region, silently leaving copper over the pad. Reaching the edge
/// turns the cut into a notch instead, which doubles as the corridor the
/// tab's feed run travels down.
///
/// Using the bounding box rather than the exact ring removes a little more
/// copper than strictly needed for a round or angled tab; that errs toward
/// clearance, which is the safe direction.
fn channel_keepout(region: &Polygon, keepout: &Polygon) -> Polygon {
    let (rlo, rhi) = region.bbox();
    let (lo, hi) = keepout.bbox();
    // Overshoot the boundary so the cut definitely crosses it rather than
    // leaving a sliver of copper behind.
    let over = 1.0;

    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| Polygon {
        points: vec![
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ],
    };

    // Break out toward whichever side is closest, so the channel is as short
    // as possible and takes the least copper with it.
    let candidates = [
        (lo.x - rlo.x, rect(rlo.x - over, lo.y, hi.x, hi.y)),
        (rhi.x - hi.x, rect(lo.x, lo.y, rhi.x + over, hi.y)),
        (lo.y - rlo.y, rect(lo.x, rlo.y - over, hi.x, hi.y)),
        (rhi.y - hi.y, rect(lo.x, lo.y, hi.x, rhi.y + over)),
    ];
    candidates
        .into_iter()
        .min_by(|(a, _), (b, _)| a.partial_cmp(b).expect("finite bounds"))
        .expect("four candidates")
        .1
}

/// A routed chain: one continuous pad-to-pad trace, plus which of its
/// segments are the links inserted between regions and tabs.
pub struct Routed {
    pub trace: Vec<PathSeg>,
    /// Indices into `trace` of the connecting runs.
    pub link_indices: Vec<usize>,
    /// Total length of those runs — resistance that heats the interconnect
    /// rather than the board.
    pub link_length_mm: f64,
}

/// One routing request: the planned chain plus the trace parameters.
pub struct RouteSpec<'a> {
    pub chain: &'a Chain,
    pub kind: FillKind,
    pub pitch_mm: f64,
    pub inset_mm: f64,
    pub style: CornerStyle,
    pub tab_in: &'a Pad,
    pub tab_out: &'a Pad,
}

/// Fill every region and stitch the whole chain into one pad-to-pad path.
pub fn route(spec: RouteSpec<'_>, warnings: &mut Vec<String>) -> Result<Routed, EngineError> {
    let RouteSpec {
        chain,
        kind,
        pitch_mm,
        inset_mm,
        style,
        tab_in,
        tab_out,
    } = spec;
    let mut trace: Vec<PathSeg> = Vec::new();
    let mut link_indices: Vec<usize> = Vec::new();
    let mut link_len = 0.0;
    // The chain is a single conductor: the pen starts on the input pad and
    // must reach every region's entry from wherever the last one left off.
    let mut cursor = tab_in.center();

    for (pos, region) in chain.regions.iter().enumerate() {
        let quarter = best_quarter(&region.polygon, region.target);
        let rotated = quarter.rotate_polygon(&region.polygon);
        let path = fills::fill(
            fills::FillSpec {
                kind,
                outline: &rotated,
                pitch_mm,
                inset_mm,
                // Tab keepouts are already cut out of the region, so the
                // pattern may use all of what it is given.
                reserve: Reserve::none(),
                style,
                terminals: region.terminals,
            },
            warnings,
        )?;
        let path = quarter.inverse().rotate_path(&path);

        let (start, end) = (
            path.first().expect("nonempty fill").start(),
            path.last().expect("nonempty fill").end(),
        );
        // Enter at whichever terminal is closer to the pen; the fill is
        // symmetric, so running it backwards is electrically identical.
        let (path, start, end) = if cursor.dist(&start) <= cursor.dist(&end) {
            (path, start, end)
        } else {
            (crate::fills::reverse_path(&path), end, start)
        };

        link_len += push_link(&mut trace, &mut link_indices, cursor, start);
        trace.extend(path);
        cursor = end;

        if pos + 1 == chain.regions.len() {
            link_len += push_link(&mut trace, &mut link_indices, cursor, tab_out.center());
        }
    }

    if trace.is_empty() {
        return Err(EngineError::BadGeometry(
            "routing produced an empty trace".into(),
        ));
    }
    Ok(Routed {
        trace,
        link_indices,
        link_length_mm: link_len,
    })
}

/// Append a straight link, recording its index, and return its length.
fn push_link(
    trace: &mut Vec<PathSeg>,
    link_indices: &mut Vec<usize>,
    from: Point,
    to: Point,
) -> f64 {
    let d = from.dist(&to);
    if d < 1e-9 {
        return 0.0;
    }
    link_indices.push(trace.len());
    trace.push(PathSeg::Line { a: from, b: to });
    d
}

/// Pick the rotation that puts `target` to the left of the region, which is
/// where every fill pattern places its two path ends.
fn best_quarter(region: &Polygon, target: Point) -> Quarter {
    let c = region.centroid();
    let d = Point::new(target.x - c.x, target.y - c.y);
    // In rotated space we want the target direction pointing at -x, so pick
    // the rotation minimising the rotated x component.
    *Quarter::ALL
        .iter()
        .min_by(|a, b| {
            a.apply(d)
                .x
                .partial_cmp(&b.apply(d).x)
                .expect("finite coordinates")
        })
        .expect("ALL is non-empty")
}

/// Count places where a link run shorts against the rest of the trace.
///
/// Links are routed as straight shots, which is right when a tab sits near
/// the terminals it feeds but can cut across a fill when it does not. Rather
/// than silently emitting a short, the caller warns. Arcs are tested as true
/// arcs — see [`crate::geom`].
pub fn count_link_crossings(trace: &[PathSeg], link_indices: &[usize]) -> usize {
    crate::geom::find_shorts(trace, link_indices).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminals::PadRect;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon {
            points: vec![
                Point::new(x0, y0),
                Point::new(x1, y0),
                Point::new(x1, y1),
                Point::new(x0, y1),
            ],
        }
    }

    fn pad_at(cx: f64, cy: f64) -> Pad {
        Pad::Rect(PadRect {
            cx,
            cy,
            w: 2.0,
            h: 1.5,
        })
    }

    #[test]
    fn quarter_rotations_are_invertible_and_preserve_arc_handedness() {
        let path = vec![PathSeg::Arc {
            a: Point::new(1.0, 0.0),
            b: Point::new(-1.0, 0.0),
            center: Point::new(0.0, 0.0),
            ccw: true,
        }];
        for q in Quarter::ALL {
            let there = q.rotate_path(&path);
            match there[0] {
                // A rotation must never flip the sweep direction.
                PathSeg::Arc { ccw, .. } => assert!(ccw, "{q:?} flipped the arc"),
                _ => unreachable!(),
            }
            let back = q.inverse().rotate_path(&there);
            assert!(back[0].start().dist(&path[0].start()) < 1e-12, "{q:?}");
            assert!(back[0].end().dist(&path[0].end()) < 1e-12, "{q:?}");
        }
    }

    #[test]
    fn terminals_face_the_target_side() {
        let region = rect(0.0, 0.0, 10.0, 10.0);
        // Target to the region's right → needs a 180° turn to face left.
        assert_eq!(best_quarter(&region, Point::new(30.0, 5.0)), Quarter::R180);
        // Already to the left → no rotation.
        assert_eq!(best_quarter(&region, Point::new(-30.0, 5.0)), Quarter::R0);
        // Directly above (smaller y in the y-down frame).
        let up = best_quarter(&region, Point::new(5.0, -30.0));
        assert!(up == Quarter::R90 || up == Quarter::R270, "{up:?}");
    }

    #[test]
    fn regions_chain_nearest_first_from_the_input_tab() {
        // Three regions in a row; the tab sits left of the middle one, so the
        // walk should go middle → left → right or middle → ... nearest-first.
        let heaters = vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(20.0, 0.0, 30.0, 10.0),
            rect(40.0, 0.0, 50.0, 10.0),
        ];
        let mut w = Vec::new();
        let chain = plan(
            &heaters,
            &pad_at(-5.0, 5.0),
            &pad_at(55.0, 5.0),
            0.5,
            &mut w,
        )
        .unwrap();
        assert_eq!(chain.regions.len(), 3);
        // Starting from x=-5, nearest-first gives left → middle → right.
        let xs: Vec<f64> = chain
            .regions
            .iter()
            .map(|r| r.polygon.centroid().x)
            .collect();
        assert!(xs[0] < xs[1] && xs[1] < xs[2], "{xs:?}");
    }

    #[test]
    fn an_overlapping_tab_is_cut_out_of_its_region() {
        let heaters = vec![rect(0.0, 0.0, 20.0, 20.0)];
        let tab = pad_at(3.0, 10.0); // well inside the region
        let mut w = Vec::new();
        let chain = plan(&heaters, &tab, &pad_at(30.0, 10.0), 0.5, &mut w).unwrap();
        let poly = &chain.regions[0].polygon;
        // The pad's own centre must no longer be inside the fillable region.
        assert!(
            !poly.contains(Point::new(3.0, 10.0)),
            "tab was not cut out of the region"
        );
        // The cut must be a notch open to the left edge, not an enclosed
        // hole: a point between the pad and that edge is also outside now.
        assert!(
            !poly.contains(Point::new(0.5, 10.0)),
            "cut left a hole instead of a notch to the edge"
        );
        // Pad grown to 3.0 × 2.5, channel run out to the left edge:
        // 4.5 mm × 2.5 mm ≈ 11.25 mm² off the original 400 mm².
        let lost = 400.0 - poly.area_mm2();
        assert!(
            (lost - 11.25).abs() < 0.5,
            "expected to lose ~11.25 mm², lost {lost}"
        );
    }

    #[test]
    fn a_tab_outside_every_region_leaves_them_untouched() {
        let heaters = vec![rect(0.0, 0.0, 20.0, 20.0)];
        let mut w = Vec::new();
        let chain = plan(
            &heaters,
            &pad_at(-5.0, 10.0),
            &pad_at(30.0, 10.0),
            0.5,
            &mut w,
        )
        .unwrap();
        assert!((chain.regions[0].polygon.area_mm2() - 400.0).abs() < 1e-6);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn crossing_detection_finds_a_link_cutting_through_a_row() {
        // A link that spans a horizontal run it is not joined to.
        let trace = vec![
            PathSeg::Line {
                a: Point::new(0.0, 5.0),
                b: Point::new(10.0, 5.0),
            },
            PathSeg::Line {
                a: Point::new(20.0, 0.0),
                b: Point::new(20.0, 10.0),
            },
            // This link crosses segment 0.
            PathSeg::Line {
                a: Point::new(5.0, 0.0),
                b: Point::new(5.0, 10.0),
            },
        ];
        assert_eq!(count_link_crossings(&trace, &[2]), 1);
        // A link that only touches endpoints of its neighbours is fine.
        let clean = vec![
            PathSeg::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(10.0, 0.0),
            },
            PathSeg::Line {
                a: Point::new(10.0, 0.0),
                b: Point::new(10.0, 10.0),
            },
        ];
        assert_eq!(count_link_crossings(&clean, &[1]), 0);
    }
}
