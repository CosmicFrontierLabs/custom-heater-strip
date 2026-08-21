//! Routing a heater over a selection of DXF polygons.
//!
//! The selection is required to be **contiguous**, and the solder tabs to sit
//! **inside** it. Those two constraints are what make this tractable, and
//! together they collapse what used to be a hard problem into an easy one:
//!
//! - Contiguous polygons have a connected union, so they are unioned into
//!   **one region** and filled **once**. There is no chain of regions and no
//!   linking runs between them — which is worth stating plainly, because those
//!   links were the entire source of copper-crossing-copper. A problem removed
//!   rather than mitigated.
//! - Tabs inside the region mean every feed run is short and local. Each tab
//!   gets a **channel** carved from it to a **lane** reserved down one side of
//!   the region, and the fill is handed what is left. That is the same shape as
//!   the auto-placed pads' pocket-and-lane, which is already known to produce
//!   clean feeds; the only difference is that the pocket sits wherever the user
//!   drew the tab instead of where we chose to put it.
//!
//! The region is rotated by a multiple of 90° before filling so the lane is on
//! the left, which is where every pattern delivers its path ends, and the
//! routed path is rotated back. Only rotations, never reflections: a
//! reflection would invert every arc's sweep direction.

use shared::{CornerStyle, FillKind};

use crate::fills::{self, Reserve};
use crate::outline::{self, Polygon};
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

/// The heater selection, validated and merged.
#[derive(Debug)]
pub struct Plan {
    /// The single region to fill: the union of the selected polygons.
    pub region: Polygon,
    /// How many polygons went into it.
    pub merged_from: usize,
}

/// Union the selection and enforce the two rules that make it routable.
///
/// Contiguity is checked by *counting the pieces of the union*: if the
/// polygons all touch, there is one. That is the same question asked once
/// rather than an adjacency test with its own separate tolerance.
pub fn plan(
    heaters: &[Polygon],
    tabs: [&Pad; 2],
    warnings: &mut Vec<String>,
) -> Result<Plan, EngineError> {
    if heaters.is_empty() {
        return Err(EngineError::BadGeometry(
            "select at least one polygon as a heater region".into(),
        ));
    }

    let pieces = outline::union_all(heaters);
    let Some(region) = pieces.first().cloned() else {
        return Err(EngineError::BadGeometry(
            "the selected heater polygons enclose no area".into(),
        ));
    };
    if pieces.len() > 1 {
        return Err(EngineError::BadGeometry(format!(
            "the selected heater regions fall into {} separate groups, but they \
             must all touch so the heater is one connected element. Extend them \
             until they share an edge, or deselect the outliers.",
            pieces.len()
        )));
    }

    for (tab, which) in tabs.into_iter().zip(["input", "output"]) {
        let ring = Polygon { points: tab.ring() };
        if !region.contains_polygon(&ring) {
            let touching = region.overlaps(&ring);
            return Err(EngineError::BadGeometry(format!(
                "the {which} solder tab must sit entirely inside the heater \
                 area{}",
                if touching {
                    ", but it hangs over the edge"
                } else {
                    "; it is currently outside it"
                }
            )));
        }
    }

    if heaters.len() > 1 {
        warnings.push(format!(
            "{} selected polygons merged into one heater of {:.1} cm²",
            heaters.len(),
            region.area_mm2() / 100.0
        ));
    }
    Ok(Plan {
        region,
        merged_from: heaters.len(),
    })
}

/// Area of the region the terminal corridor and tab pocket keep the fill out
/// of, in mm², for the orientation that would actually be chosen.
///
/// Wanted before routing, so the electrical solve can size the trace against
/// the area it can really fill.
pub fn reserved_area(
    plan: &Plan,
    tab_in: &Pad,
    tab_out: &Pad,
    inset_mm: f64,
    pitch_mm: f64,
) -> f64 {
    let (_, rotated, _, _, corridor) = choose_orientation(
        plan,
        tab_in,
        tab_out,
        inset_mm,
        pitch_mm,
        FillKind::Serpentine,
    );
    fills::reserved_area(&rotated, pitch_mm, inset_mm, corridor.reserve)
}

/// A routed design over the unioned region.
pub struct Routed {
    pub trace: Vec<PathSeg>,
    /// Indices of the feed runs, for the copper-on-copper check.
    pub link_indices: Vec<usize>,
    /// Total length of those runs.
    pub link_length_mm: f64,
}

/// One routing request.
pub struct RouteSpec<'a> {
    pub plan: &'a Plan,
    pub kind: FillKind,
    pub pitch_mm: f64,
    pub inset_mm: f64,
    pub style: CornerStyle,
    pub tab_in: &'a Pad,
    pub tab_out: &'a Pad,
}

/// Fill the region once and feed both tabs through the reserved corridor.
pub fn route(spec: RouteSpec<'_>, warnings: &mut Vec<String>) -> Result<Routed, EngineError> {
    let RouteSpec {
        plan,
        kind,
        pitch_mm,
        inset_mm,
        style,
        tab_in,
        tab_out,
    } = spec;

    let (quarter, rotated, ra, rb, corridor) =
        choose_orientation(plan, tab_in, tab_out, inset_mm, pitch_mm, kind);

    if fills::is_scanline(kind) {
        let coverage = fills::scanline_coverage(&rotated, pitch_mm, inset_mm, corridor.reserve);
        if coverage < 0.999 {
            warnings.push(format!(
                "the heater area crosses each row more than once in every \
                 orientation, so {:.0}% of it cannot be reached by a single \
                 {} path ({:.1} cm² unheated). The concentric fill follows the \
                 outline instead and may cover more.",
                100.0 * (1.0 - coverage),
                kind.label().to_lowercase(),
                plan.region.area_mm2() * (1.0 - coverage) / 100.0
            ));
        }
    }
    let mut path = fills::fill(
        fills::FillSpec {
            kind,
            outline: &rotated,
            pitch_mm,
            inset_mm,
            reserve: corridor.reserve,
            style,
        },
        warnings,
    )?;

    // Which terminal each tab connects to is ours to choose, and it decides
    // whether the two feeds can be routed without crossing.
    //
    // Each feed spans the stretch of the corridor between its tab and its
    // terminal. Two such spans can be routed side by side only if one
    // *contains* the other — then the outer lane carries the containing span
    // and encloses the inner one. If they merely interleave, no assignment of
    // lanes helps and they must cross somewhere.
    //
    // Running the fill backwards swaps which terminal each tab gets, which
    // turns an interleaved pair into a nested one. That is what the bifilar
    // pattern needs: its two ends come back a single pitch apart, so the
    // natural pairing is almost always the interleaved one.
    let (ca, cb) = (ra.centroid(), rb.centroid());
    let span = |tab_y: f64, term_y: f64| (tab_y.min(term_y), tab_y.max(term_y));
    let nests =
        |a: (f64, f64), b: (f64, f64)| (a.0 <= b.0 && b.1 <= a.1) || (b.0 <= a.0 && a.1 <= b.1);

    let ends = |p: &[PathSeg]| {
        (
            p.first().expect("nonempty fill").start(),
            p.last().expect("nonempty fill").end(),
        )
    };
    let (f0, l0) = ends(&path);
    if !nests(span(ca.y, f0.y), span(cb.y, l0.y)) {
        let flipped = crate::fills::reverse_path(&path);
        let (f1, l1) = ends(&flipped);
        if nests(span(ca.y, f1.y), span(cb.y, l1.y)) {
            path = flipped;
        }
    }
    let (t_first, t_last) = ends(&path);

    // Outer lane carries the span that encloses the other.
    let (sa, sb) = (span(ca.y, t_first.y), span(cb.y, t_last.y));
    let first_is_outer = (sa.1 - sa.0) >= (sb.1 - sb.0);
    let (lane_first, lane_last) = if first_is_outer {
        (corridor.lane_outer, corridor.lane_inner)
    } else {
        (corridor.lane_inner, corridor.lane_outer)
    };

    let (hop_a, hop_b) = hop_heights(&ra, &rb, pitch_mm);
    let head = corridor.feed(&ra, lane_first, hop_a, t_first);
    let tail = crate::fills::reverse_path(&corridor.feed(&rb, lane_last, hop_b, t_last));

    let head_len: f64 = head.iter().map(|s| s.length()).sum();
    let tail_len: f64 = tail.iter().map(|s| s.length()).sum();
    let mut trace = head;
    let head_n = trace.len();
    trace.extend(path);
    let tail_start = trace.len();
    trace.extend(tail);

    let link_indices: Vec<usize> = (0..head_n).chain(tail_start..trace.len()).collect();
    Ok(Routed {
        trace: quarter.inverse().rotate_path(&trace),
        link_indices,
        link_length_mm: head_len + tail_len,
    })
}

/// Pick the rotation to fill in, and build its corridor.
///
/// This is the most consequential decision here, so all four are evaluated
/// rather than guessed at, in a strict order of priorities.
///
/// **Coverage first.** The scanline patterns route one span per row, so a row
/// crossing the shape twice loses an arm entirely. Which rows cross twice
/// depends only on which way the rows run: a U swept across its arms throws
/// one away, swept along them loses nothing. Every shape tried so far — U, H,
/// C, L, T, plus, staircase — is single-span in one of the two directions, so
/// this is usually the difference between full coverage and losing half the
/// board.
///
/// **Then tab stacking.** Both feeds leave for the corridor at their own tab's
/// height, so tabs at the same height put the two runs on one line and they
/// short. Rotating 90° turns a side-by-side pair into a stacked one.
///
/// **Then reach**, which is channel length, and therefore interconnect that
/// heats the wrong part of the board.
fn choose_orientation(
    plan: &Plan,
    tab_in: &Pad,
    tab_out: &Pad,
    inset_mm: f64,
    pitch_mm: f64,
    kind: FillKind,
) -> (Quarter, Polygon, Polygon, Polygon, Corridor) {
    let tabs_at = midpoint(tab_in.center(), tab_out.center());
    let scanline = fills::is_scanline(kind);
    let mut best: Option<(Quarter, Polygon, Polygon, Polygon, Corridor, f64)> = None;
    for q in Quarter::ALL {
        let rotated = q.rotate_polygon(&plan.region);
        let ra = q.rotate_polygon(&Polygon {
            points: tab_in.ring(),
        });
        let rb = q.rotate_polygon(&Polygon {
            points: tab_out.ring(),
        });
        let corridor = Corridor::new(&rotated, &[&ra, &rb], inset_mm, pitch_mm);
        let coverage = if scanline {
            fills::scanline_coverage(&rotated, pitch_mm, inset_mm, corridor.reserve)
        } else {
            1.0
        };
        let stacking = (q.apply(tab_in.center()).y - q.apply(tab_out.center()).y).abs();
        let reach = (q.apply(tabs_at).x - rotated.bbox().0.x).abs();
        let score = coverage * 1.0e9 + stacking * 1.0e3 - reach;
        if best.as_ref().is_none_or(|(_, _, _, _, _, s)| score > *s) {
            best = Some((q, rotated, ra, rb, corridor, score));
        }
    }
    let (q, rotated, ra, rb, corridor, _) = best.expect("four orientations");
    (q, rotated, ra, rb, corridor)
}

/// The reserved feed corridor down the region's left side, plus the pocket
/// that keeps the fill off the tabs inside it.
struct Corridor {
    reserve: Reserve,
    lane_inner: f64,
    lane_outer: f64,
}

impl Corridor {
    fn new(region: &Polygon, tabs: &[&Polygon], inset_mm: f64, pitch_mm: f64) -> Self {
        let (rlo, rhi) = region.bbox();
        // Wide enough for two lanes a pitch apart plus clearance to the fill.
        let width = (3.0 * pitch_mm).max(1.2);
        let lane_edge = rlo.x + inset_mm + width;
        // The pocket spans the y band the tabs occupy and reaches right of the
        // rightmost of them, so each tab's run out to the corridor crosses
        // only cleared ground.
        // A pitch of headroom on top of the clearance, so a staggered hop
        // (see `hop_heights`) still leaves from inside the pocket.
        let clearance = inset_mm + 2.0 * pitch_mm;
        let (mut y0, mut y1, mut x1) = (f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for t in tabs {
            let (lo, hi) = t.bbox();
            y0 = y0.min(lo.y - clearance);
            y1 = y1.max(hi.y + clearance);
            x1 = x1.max(hi.x + clearance);
        }
        Corridor {
            reserve: Reserve {
                lane_edge,
                pocket_x1: x1.min(rhi.x),
                pocket_y0: y0,
                pocket_y1: y1,
            },
            lane_inner: lane_edge - pitch_mm,
            lane_outer: lane_edge - 2.0 * pitch_mm,
        }
    }

    /// Tab centre → out to `lane_x` at height `hop_y` → along the corridor →
    /// into the terminal. Every leg stays in reserved ground: the hop is inside
    /// the tab pocket, the run along the corridor inside the lane.
    ///
    /// `hop_y` is separate from the tab's own centre so the caller can stagger
    /// two feeds that would otherwise leave at the same height — see
    /// [`hop_heights`].
    fn feed(&self, tab: &Polygon, lane_x: f64, hop_y: f64, terminal: Point) -> Vec<PathSeg> {
        let c = tab.centroid();
        segments(&[
            c,
            Point::new(c.x, hop_y),
            Point::new(lane_x, hop_y),
            Point::new(lane_x, terminal.y),
            terminal,
        ])
    }
}

/// Heights at which the two feeds should leave their tabs.
///
/// Each feed runs out to the corridor at its own tab's height, which is fine
/// while the tabs sit at different heights. When they do not — two tabs side
/// by side, and the orientation was chosen for coverage rather than for them —
/// both runs land on the same line and short against each other.
///
/// So when they are within a pitch, one is stepped a pitch clear of the other,
/// away from its partner. The pocket is built a pitch wider than strictly
/// needed to guarantee that step is still over reserved ground.
fn hop_heights(a: &Polygon, b: &Polygon, pitch_mm: f64) -> (f64, f64) {
    let (ya, yb) = (a.centroid().y, b.centroid().y);
    if (ya - yb).abs() >= pitch_mm {
        return (ya, yb);
    }
    // Step whichever one is already further along, so they move apart.
    if ya <= yb {
        (ya - pitch_mm, yb + pitch_mm)
    } else {
        (ya + pitch_mm, yb - pitch_mm)
    }
}

fn midpoint(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}

/// Consecutive points to line segments, dropping degenerate hops.
fn segments(pts: &[Point]) -> Vec<PathSeg> {
    pts.windows(2)
        .filter(|w| w[0].dist(&w[1]) > 1e-9)
        .map(|w| PathSeg::Line { a: w[0], b: w[1] })
        .collect()
}

/// Count places where a feed run shorts against the rest of the trace, with
/// arcs handled exactly — see [`crate::geom`].
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
            w: 4.0,
            h: 3.0,
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
                PathSeg::Arc { ccw, .. } => assert!(ccw, "{q:?} flipped the arc"),
                _ => unreachable!(),
            }
            let back = q.inverse().rotate_path(&there);
            assert!(back[0].start().dist(&path[0].start()) < 1e-12, "{q:?}");
        }
    }

    #[test]
    fn abutting_selections_merge_into_one_region() {
        let heaters = [
            rect(0.0, 0.0, 40.0, 30.0),
            rect(40.0, 0.0, 80.0, 30.0),
            rect(80.0, 0.0, 120.0, 30.0),
        ];
        let mut w = Vec::new();
        let p = plan(&heaters, [&pad_at(10.0, 15.0), &pad_at(20.0, 15.0)], &mut w).unwrap();
        assert_eq!(p.merged_from, 3);
        assert!(
            (p.region.area_mm2() - 3600.0).abs() < 1e-6,
            "{}",
            p.region.area_mm2()
        );
        assert!(
            w.iter().any(|m| m.contains("merged into one heater")),
            "{w:?}"
        );
    }

    #[test]
    fn a_detached_selection_is_rejected() {
        let heaters = [rect(0.0, 0.0, 40.0, 30.0), rect(200.0, 0.0, 240.0, 30.0)];
        let err = plan(
            &heaters,
            [&pad_at(10.0, 15.0), &pad_at(20.0, 15.0)],
            &mut Vec::new(),
        )
        .expect_err("not contiguous");
        assert!(err.to_string().contains("separate groups"), "{err}");
    }

    #[test]
    fn a_tab_outside_the_heater_is_rejected() {
        let heaters = [rect(0.0, 0.0, 40.0, 30.0)];
        let err = plan(
            &heaters,
            [&pad_at(10.0, 15.0), &pad_at(200.0, 15.0)],
            &mut Vec::new(),
        )
        .expect_err("tab outside");
        assert!(err.to_string().contains("outside it"), "{err}");
    }

    #[test]
    fn a_tab_straddling_the_edge_says_so() {
        let heaters = [rect(0.0, 0.0, 40.0, 30.0)];
        let err = plan(
            &heaters,
            [&pad_at(10.0, 15.0), &pad_at(39.0, 15.0)],
            &mut Vec::new(),
        )
        .expect_err("tab straddles");
        assert!(err.to_string().contains("hangs over the edge"), "{err}");
    }
}
