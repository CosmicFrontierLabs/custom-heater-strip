//! Generalized Hilbert ("gilbert") space-filling curve fill: best thermal
//! isotropy of the catalog (no long parallel runs). Port of the recursive
//! algorithm from github.com/jakubcerveny/gilbert, driven with the major
//! axis vertical so both curve endpoints land on the LEFT edge of the grid
//! (top-left start, bottom-left end) — exactly where the terminal zone is.
//!
//! Rectangular outlines only: the curve is generated over the bounding box,
//! so a non-rectangular outline would leave copper outside the board.

use crate::{outline::Polygon, EngineError, PathSeg, Point};

pub fn fill(
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
    left_reserved_mm: f64,
    _warnings: &mut Vec<String>,
) -> Result<Vec<PathSeg>, EngineError> {
    let (min, max) = outline.bbox();
    let bbox_area = (max.x - min.x) * (max.y - min.y);
    if outline.area_mm2() < 0.98 * bbox_area {
        return Err(EngineError::Infeasible(
            "the Hilbert fill only supports rectangular outlines (the curve \
             is generated over the bounding box); use Serpentine or \
             Concentric for shaped boards"
                .into(),
        ));
    }

    let x0 = min.x + inset_mm + left_reserved_mm;
    let y0 = min.y + inset_mm;
    let w = max.x - inset_mm - x0;
    let h = max.y - inset_mm - y0;
    let cols = (w / pitch_mm).floor() as i64;
    let rows = (h / pitch_mm).floor() as i64;
    if cols < 2 || rows < 2 {
        return Err(EngineError::OutlineTooSmall(format!(
            "Hilbert grid is {cols}×{rows} cells at {pitch_mm:.2} mm pitch; \
             need at least 2×2"
        )));
    }

    // Cell centers, exact-fit cell size.
    let (cw, ch) = (w / cols as f64, h / rows as f64);
    let to_mm =
        |gx: i64, gy: i64| Point::new(x0 + (gx as f64 + 0.5) * cw, y0 + (gy as f64 + 0.5) * ch);

    let mut cells = Vec::with_capacity((cols * rows) as usize);
    // Major axis vertical: curve runs (0,0) → (0,rows-1), both on the left.
    generate(0, 0, 0, rows, cols, 0, &mut cells);
    debug_assert_eq!(cells.len(), (cols * rows) as usize);

    // Merge collinear runs while emitting segments.
    let mut path: Vec<PathSeg> = Vec::with_capacity(cells.len() / 2);
    for pair in cells.windows(2) {
        let (a, b) = (to_mm(pair[0].0, pair[0].1), to_mm(pair[1].0, pair[1].1));
        if let Some(PathSeg::Line { a: pa, b: pb }) = path.last_mut() {
            let d1 = (pb.x - pa.x, pb.y - pa.y);
            let d2 = (b.x - pb.x, b.y - pb.y);
            if (d1.0 * d2.1 - d1.1 * d2.0).abs() < 1e-9 && (d1.0 * d2.0 + d1.1 * d2.1) > 0.0 {
                *pb = b; // extend the collinear run
                continue;
            }
        }
        path.push(PathSeg::Line { a, b });
    }
    Ok(path)
}

/// Recursive gilbert generator: fills the box spanned by axis vectors
/// (ax,ay) and (bx,by) starting at (x,y), pushing grid cells in curve order.
#[allow(clippy::too_many_arguments)]
fn generate(x: i64, y: i64, ax: i64, ay: i64, bx: i64, by: i64, out: &mut Vec<(i64, i64)>) {
    let w = (ax + ay).abs();
    let h = (bx + by).abs();
    let (dax, day) = (ax.signum(), ay.signum());
    let (dbx, dby) = (bx.signum(), by.signum());

    if h == 1 {
        let (mut cx, mut cy) = (x, y);
        for _ in 0..w {
            out.push((cx, cy));
            cx += dax;
            cy += day;
        }
        return;
    }
    if w == 1 {
        let (mut cx, mut cy) = (x, y);
        for _ in 0..h {
            out.push((cx, cy));
            cx += dbx;
            cy += dby;
        }
        return;
    }

    let (mut ax2, mut ay2) = (ax / 2, ay / 2);
    let (mut bx2, mut by2) = (bx / 2, by / 2);
    let w2 = (ax2 + ay2).abs();
    let h2 = (bx2 + by2).abs();

    if 2 * w > 3 * h {
        if w2 % 2 != 0 && w > 2 {
            ax2 += dax;
            ay2 += day;
        }
        generate(x, y, ax2, ay2, bx, by, out);
        generate(x + ax2, y + ay2, ax - ax2, ay - ay2, bx, by, out);
    } else {
        if h2 % 2 != 0 && h > 2 {
            bx2 += dbx;
            by2 += dby;
        }
        generate(x, y, bx2, by2, ax2, ay2, out);
        generate(x + bx2, y + by2, ax, ay, bx - bx2, by - by2, out);
        generate(
            x + (ax - dax) + (bx2 - dbx),
            y + (ay - day) + (by2 - dby),
            -bx2,
            -by2,
            -(ax - ax2),
            -(ay - ay2),
            out,
        );
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
    fn curve_visits_every_cell_exactly_once() {
        for (cols, rows) in [(8, 8), (10, 6), (7, 5), (13, 4)] {
            let mut cells = Vec::new();
            generate(0, 0, 0, rows, cols, 0, &mut cells);
            assert_eq!(cells.len(), (cols * rows) as usize, "{cols}×{rows}");
            let mut seen = std::collections::HashSet::new();
            let mut prev: Option<(i64, i64)> = None;
            for c in &cells {
                assert!(seen.insert(*c), "cell {c:?} visited twice ({cols}×{rows})");
                if let Some(p) = prev {
                    let d = (c.0 - p.0).abs() + (c.1 - p.1).abs();
                    assert_eq!(d, 1, "non-adjacent step {p:?}→{c:?} ({cols}×{rows})");
                }
                prev = Some(*c);
            }
        }
    }

    #[test]
    fn endpoints_on_left_edge() {
        let mut cells = Vec::new();
        generate(0, 0, 0, 16, 40, 0, &mut cells);
        assert_eq!(*cells.first().unwrap(), (0, 0));
        let end = *cells.last().unwrap();
        assert_eq!(end.0, 0, "curve should end on the left column, got {end:?}");
    }

    #[test]
    fn fill_produces_connected_inbounds_path() {
        let path = fill(&rect(60.0, 20.0), 1.0, 0.6, 5.0, &mut Vec::new()).unwrap();
        assert_path_well_formed(&path, 5.6, 0.6, 60.0 - 0.6, 20.0 - 0.6);
        let start = path.first().unwrap().start();
        let end = path.last().unwrap().end();
        assert!(start.x < 7.0 && end.x < 7.0, "terminals near left edge");
        assert!(start.y < end.y, "start above end");
        // Isotropy proxy: plenty of direction changes.
        assert!(path.len() > 100, "only {} segments", path.len());
    }

    #[test]
    fn non_rect_outline_rejected() {
        let tri = Polygon {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(60.0, 0.0),
                Point::new(0.0, 30.0),
            ],
        };
        assert!(matches!(
            fill(&tri, 1.0, 0.6, 0.0, &mut Vec::new()),
            Err(EngineError::Infeasible(_))
        ));
    }
}
