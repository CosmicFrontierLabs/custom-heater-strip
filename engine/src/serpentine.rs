//! Boustrophedon serpentine fill: horizontal scanlines across the outline,
//! connected end-to-end in alternating directions. Turnarounds are drawn in
//! the requested corner style: square, 45° mitered, or semicircular arcs.

use shared::CornerStyle;

use crate::{outline::Polygon, EngineError, PathSeg, Point};

pub struct Serpentine {
    pub path: Vec<PathSeg>,
    pub length_mm: f64,
}

struct Row {
    y: f64,
    x0: f64,
    x1: f64,
}

/// Fill `outline` with a serpentine of the given pitch, keeping `inset` mm of
/// clearance from the boundary (edge margin + half trace width).
///
/// `left_reserved_mm` keeps a strip at the left edge free for the terminal
/// zone; when nonzero the row count is forced even so the path starts *and*
/// ends at the left, next to the pads.
pub fn fill(
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
    left_reserved_mm: f64,
    style: CornerStyle,
    warnings: &mut Vec<String>,
) -> Result<Serpentine, EngineError> {
    let (min, max) = outline.bbox();
    let y_lo = min.y + inset_mm;
    let y_hi = max.y - inset_mm;
    let usable_height = y_hi - y_lo;
    if usable_height <= 0.0 {
        return Err(EngineError::OutlineTooSmall(format!(
            "outline height {:.2} mm can't fit the {:.2} mm edge inset",
            max.y - min.y,
            inset_mm
        )));
    }

    let mut rows_n = (usable_height / pitch_mm).floor() as usize + 1;
    if left_reserved_mm > 0.0 && rows_n % 2 == 1 {
        rows_n -= 1;
    }
    if rows_n < 2 {
        return Err(EngineError::OutlineTooSmall(format!(
            "only {rows_n} serpentine row(s) fit; outline is too small for the \
             required {pitch_mm:.2} mm pitch"
        )));
    }
    // Center the rows vertically in the usable band.
    let y_start = y_lo + (usable_height - (rows_n - 1) as f64 * pitch_mm) / 2.0;

    let mut rows: Vec<Row> = Vec::new();
    let mut multi_span_rows = 0usize;
    let mut dropped_rows = 0usize;

    for i in 0..rows_n {
        let y = y_start + i as f64 * pitch_mm;
        let hits = outline.scanline_hits(y);
        // Pair up even-odd crossings into inside spans, shrunk by the inset
        // and pushed right of the reserved terminal zone.
        let zone_edge = min.x + inset_mm + left_reserved_mm;
        let mut spans: Vec<(f64, f64)> = hits
            .chunks_exact(2)
            .map(|c| ((c[0] + inset_mm).max(zone_edge), c[1] - inset_mm))
            .filter(|(a, b)| b > a)
            .collect();
        if spans.is_empty() {
            dropped_rows += 1;
            continue;
        }
        if spans.len() > 1 {
            multi_span_rows += 1;
            // Keep the widest span; concave regions to the side are skipped.
            spans.sort_by(|p, q| (q.1 - q.0).partial_cmp(&(p.1 - p.0)).unwrap());
        }
        rows.push(Row {
            y,
            x0: spans[0].0,
            x1: spans[0].1,
        });
    }

    if multi_span_rows > 0 {
        warnings.push(format!(
            "outline is concave: {multi_span_rows} row(s) crossed it more than \
             once; only the widest section was routed. Split complex shapes \
             into separate heaters for full coverage."
        ));
    }
    if dropped_rows > 0 {
        warnings.push(format!(
            "{dropped_rows} row(s) near the outline edge were too narrow to \
             route and were skipped"
        ));
    }
    // Span drops can re-odd the count; the terminal zone needs the path to
    // end back at the left edge.
    if left_reserved_mm > 0.0 && rows.len() % 2 == 1 {
        rows.pop();
    }
    if rows.len() < 2 {
        return Err(EngineError::OutlineTooSmall(
            "fewer than two serpentine rows could be routed".into(),
        ));
    }

    // Mitered/smooth turnarounds eat pitch/2 of row length at each turn.
    // If any row is too narrow to give that up, fall back to square corners.
    let mut style = style;
    let turn_inset = match style {
        CornerStyle::Rectangular => 0.0,
        CornerStyle::Mitered | CornerStyle::Smooth => pitch_mm / 2.0,
    };
    if turn_inset > 0.0 && rows.iter().any(|r| r.x1 - r.x0 < 3.0 * turn_inset) {
        warnings.push(format!(
            "some rows are too narrow for {} turnarounds; using rectangular \
             corners instead",
            match style {
                CornerStyle::Mitered => "mitered",
                _ => "smooth",
            }
        ));
        style = CornerStyle::Rectangular;
    }
    let turn_inset = match style {
        CornerStyle::Rectangular => 0.0,
        _ => pitch_mm / 2.0,
    };

    // Trim rows at each turn so the connector geometry stays inside the
    // envelope, and record the turn's x line. Turn k joins rows k and k+1;
    // even k turns on the right, odd on the left.
    let n = rows.len();
    let mut xl: Vec<f64> = rows.iter().map(|r| r.x0).collect();
    let mut xr: Vec<f64> = rows.iter().map(|r| r.x1).collect();
    let mut turn_x = vec![0.0f64; n - 1];
    for k in 0..n - 1 {
        if k % 2 == 0 {
            let x = rows[k].x1.min(rows[k + 1].x1);
            turn_x[k] = x;
            xr[k] = x - turn_inset;
            xr[k + 1] = x - turn_inset;
        } else {
            let x = rows[k].x0.max(rows[k + 1].x0);
            turn_x[k] = x;
            xl[k] = x + turn_inset;
            xl[k + 1] = x + turn_inset;
        }
    }

    let mut path: Vec<PathSeg> = Vec::new();
    for k in 0..n {
        let y = rows[k].y;
        let (sx, ex) = if k % 2 == 0 {
            (xl[k], xr[k])
        } else {
            (xr[k], xl[k])
        };
        path.push(PathSeg::Line {
            a: Point::new(sx, y),
            b: Point::new(ex, y),
        });

        if k + 1 == n {
            break;
        }
        let y2 = rows[k + 1].y;
        let ymid = (y + y2) / 2.0;
        let right_turn = k % 2 == 0;
        match style {
            CornerStyle::Rectangular => path.push(PathSeg::Line {
                a: Point::new(ex, y),
                b: Point::new(ex, y2),
            }),
            CornerStyle::Mitered => {
                let apex = Point::new(turn_x[k], ymid);
                path.push(PathSeg::Line {
                    a: Point::new(ex, y),
                    b: apex,
                });
                path.push(PathSeg::Line {
                    a: apex,
                    b: Point::new(ex, y2),
                });
            }
            CornerStyle::Smooth => path.push(PathSeg::Arc {
                a: Point::new(ex, y),
                b: Point::new(ex, y2),
                center: Point::new(ex, ymid),
                // Right turns bulge +x, which is a positive-angle sweep in
                // the y-down board frame.
                ccw: right_turn,
            }),
        }
    }

    let length_mm = path.iter().map(|s| s.length()).sum();
    Ok(Serpentine { path, length_mm })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn fill_style(style: CornerStyle) -> Serpentine {
        fill(&rect(100.0, 20.0), 1.0, 0.6, 0.0, style, &mut Vec::new()).unwrap()
    }

    #[test]
    fn rect_fill_length_close_to_estimate() {
        let mut w = Vec::new();
        let s = fill(
            &rect(100.0, 20.0),
            1.0,
            0.6,
            0.0,
            CornerStyle::Rectangular,
            &mut w,
        )
        .unwrap();
        let est = (20.0 - 2.0 * 0.6) / 1.0 * (100.0 - 2.0 * 0.6);
        assert!(
            (s.length_mm - est).abs() / est < 0.15,
            "len {} vs est {}",
            s.length_mm,
            est
        );
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn all_styles_stay_inside_envelope_and_connect() {
        for style in CornerStyle::ALL {
            let s = fill_style(style);
            let mut prev_end: Option<Point> = None;
            for seg in &s.path {
                // Continuity: each segment starts where the last ended.
                if let Some(pe) = prev_end {
                    assert!(pe.dist(&seg.start()) < 1e-9, "gap in {style:?} path");
                }
                prev_end = Some(seg.end());
                // Envelope: endpoints and arc bulges stay inside the inset.
                for p in [seg.start(), seg.end()] {
                    assert!(p.x >= 0.6 - 1e-9 && p.x <= 100.0 - 0.6 + 1e-9, "{style:?}");
                    assert!(p.y >= 0.6 - 1e-9 && p.y <= 20.0 - 0.6 + 1e-9, "{style:?}");
                }
                if let PathSeg::Arc { center, .. } = seg {
                    let bulge = center.x + seg.radius();
                    let bulge_l = center.x - seg.radius();
                    assert!(bulge <= 100.0 - 0.6 + 1e-9 && bulge_l >= 0.6 - 1e-9);
                }
            }
        }
    }

    #[test]
    fn smooth_arcs_are_semicircles_with_pitch_radius() {
        let s = fill_style(CornerStyle::Smooth);
        let arcs: Vec<_> = s
            .path
            .iter()
            .filter(|seg| matches!(seg, PathSeg::Arc { .. }))
            .collect();
        assert!(!arcs.is_empty());
        for arc in arcs {
            assert!((arc.radius() - 0.5).abs() < 1e-9, "r={}", arc.radius());
            assert!((arc.sweep() - std::f64::consts::PI).abs() < 1e-9);
        }
    }

    #[test]
    fn smooth_is_shorter_than_rectangular() {
        // Arc turnarounds trade π·r of arc for 2r of trimmed row on each
        // side plus the 2r connector: net shorter path.
        let rect_len = fill_style(CornerStyle::Rectangular).length_mm;
        let smooth_len = fill_style(CornerStyle::Smooth).length_mm;
        let miter_len = fill_style(CornerStyle::Mitered).length_mm;
        assert!(smooth_len < rect_len);
        assert!(miter_len < rect_len);
    }

    #[test]
    fn narrow_outline_falls_back_to_rectangular() {
        let mut w = Vec::new();
        // 2 mm wide rows with 1.5 mm pitch: no room for 0.75 mm turn insets.
        let s = fill(&rect(3.0, 20.0), 1.5, 0.4, 0.0, CornerStyle::Smooth, &mut w).unwrap();
        assert!(w.iter().any(|m| m.contains("rectangular")), "{w:?}");
        assert!(s.path.iter().all(|seg| matches!(seg, PathSeg::Line { .. })));
    }

    #[test]
    fn tiny_outline_rejected() {
        assert!(fill(
            &rect(2.0, 1.0),
            1.0,
            0.6,
            0.0,
            CornerStyle::Rectangular,
            &mut Vec::new()
        )
        .is_err());
    }
}
