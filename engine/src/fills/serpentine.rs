//! Boustrophedon serpentine fill and its two derivatives: the wavy
//! serpentine (sinusoidal rows for flex-fatigue life) and the counterflow
//! bifilar serpentine (offset out-and-back, non-inductive, both terminals
//! adjacent at the start).

use shared::CornerStyle;

use super::offset::{offset_path, reverse_path};
use crate::{outline::Polygon, EngineError, PathSeg, Point};

/// Whether the row count must be even (path ends back at the left edge,
/// required by the terminal zone) or odd (path ends at the right edge,
/// required as the counterflow base so its cap sits at the far side).
#[derive(Clone, Copy, PartialEq)]
pub enum RowParity {
    Even,
    Odd,
}

struct Row {
    y: f64,
    x0: f64,
    x1: f64,
}

/// Fill `outline` with a serpentine of the given pitch, keeping `inset` mm
/// of clearance from the boundary and `left_reserved_mm` free at the left
/// edge for the terminal zone.
pub fn fill(
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
    left_reserved_mm: f64,
    style: CornerStyle,
    parity: RowParity,
    warnings: &mut Vec<String>,
) -> Result<Vec<PathSeg>, EngineError> {
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
    match parity {
        RowParity::Even if rows_n % 2 == 1 => rows_n -= 1,
        RowParity::Odd if rows_n.is_multiple_of(2) => rows_n -= 1,
        _ => {}
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
    // Span drops can flip the count; re-enforce the requested parity by
    // dropping the last row if needed.
    let is_even = rows.len().is_multiple_of(2);
    let want_even = parity == RowParity::Even;
    if is_even != want_even {
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

    Ok(path)
}

/// Serpentine with sinusoidal rows. Same topology and turnarounds; each
/// straight row becomes a sine with an integer number of half-periods
/// (so it starts and ends exactly on the row line), phase-locked to
/// absolute x so adjacent rows undulate together and keep their clearance.
pub fn fill_wavy(
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
    left_reserved_mm: f64,
    style: CornerStyle,
    warnings: &mut Vec<String>,
) -> Result<Vec<PathSeg>, EngineError> {
    let base = fill(
        outline,
        pitch_mm,
        inset_mm,
        left_reserved_mm,
        style,
        RowParity::Even,
        warnings,
    )?;

    let amplitude = 0.3 * pitch_mm;
    let target_wavelength = (6.0 * pitch_mm).max(3.0);
    let min_wavy_len = 2.0 * target_wavelength;

    let mut out = Vec::with_capacity(base.len() * 8);
    for seg in base {
        match seg {
            PathSeg::Line { a, b } if (a.y - b.y).abs() < 1e-9 && a.dist(&b) >= min_wavy_len => {
                let len = a.dist(&b);
                let half_periods = (2.0 * len / target_wavelength).round().max(2.0);
                let x_left = a.x.min(b.x);
                let samples = (half_periods as usize) * 8;
                let mut prev = a;
                for i in 1..=samples {
                    let t = i as f64 / samples as f64;
                    let x = a.x + (b.x - a.x) * t;
                    let y = if i == samples {
                        b.y // exact landing on the row line
                    } else {
                        a.y + amplitude
                            * (half_periods * std::f64::consts::PI * (x - x_left) / len).sin()
                    };
                    let p = Point::new(x, y);
                    out.push(PathSeg::Line { a: prev, b: p });
                    prev = p;
                }
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Counterflow (bifilar) serpentine: a serpentine at pitch 2p is offset
/// ±p/2 into two parallel runs joined by a cap at the far end. Current
/// counterflows everywhere (non-inductive) and both terminals sit adjacent
/// at the start, one pitch apart.
pub fn fill_counterflow(
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
    left_reserved_mm: f64,
    style: CornerStyle,
    warnings: &mut Vec<String>,
) -> Result<Vec<PathSeg>, EngineError> {
    let half = pitch_mm / 2.0;
    // The offset pair expands the base by p/2 on both sides.
    let mut base = fill(
        outline,
        2.0 * pitch_mm,
        inset_mm + half,
        left_reserved_mm,
        style,
        RowParity::Odd,
        warnings,
    )?;

    // Shorten the final row so the far-end cap stays inside the inset.
    match base.last_mut() {
        Some(PathSeg::Line { a, b }) if a.dist(b) > 2.0 * half => {
            let len = a.dist(b);
            let t = (len - half) / len;
            *b = Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        }
        _ => {
            return Err(EngineError::OutlineTooSmall(
                "counterflow base path too short for its end cap".into(),
            ))
        }
    }

    let out_run = offset_path(&base, half);
    let back_run = offset_path(&base, -half);

    // Cap: semicircle around the base endpoint, bulging along travel.
    let base_end = base.last().unwrap().end();
    let base_prev = base.last().unwrap().start();
    let dlen = base_prev.dist(&base_end).max(1e-12);
    let dir = (
        (base_end.x - base_prev.x) / dlen,
        (base_end.y - base_prev.y) / dlen,
    );
    let cap_target = Point::new(base_end.x + dir.0 * half, base_end.y + dir.1 * half);
    let (a_cap, b_cap) = (
        out_run.last().unwrap().end(),
        back_run.last().unwrap().end(),
    );
    let cap = [true, false]
        .into_iter()
        .map(|ccw| PathSeg::Arc {
            a: a_cap,
            b: b_cap,
            center: base_end,
            ccw,
        })
        .min_by(|s1, s2| {
            let d1 = s1.arc_midpoint().unwrap().dist(&cap_target);
            let d2 = s2.arc_midpoint().unwrap().dist(&cap_target);
            d1.partial_cmp(&d2).unwrap()
        })
        .unwrap();

    let mut path = out_run;
    path.push(cap);
    path.extend(reverse_path(&back_run));
    Ok(path)
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

    fn fill_style(style: CornerStyle) -> Vec<PathSeg> {
        fill(
            &rect(100.0, 20.0),
            1.0,
            0.6,
            0.0,
            style,
            RowParity::Even,
            &mut Vec::new(),
        )
        .unwrap()
    }

    fn path_len(path: &[PathSeg]) -> f64 {
        path.iter().map(|s| s.length()).sum()
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
            RowParity::Even,
            &mut w,
        )
        .unwrap();
        let est = (20.0 - 2.0 * 0.6) / 1.0 * (100.0 - 2.0 * 0.6);
        let len = path_len(&s);
        assert!((len - est).abs() / est < 0.15, "len {len} vs est {est}");
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn all_styles_stay_inside_envelope_and_connect() {
        for style in CornerStyle::ALL {
            let s = fill_style(style);
            assert_path_well_formed(&s, 0.6, 0.6, 100.0 - 0.6, 20.0 - 0.6);
            for seg in &s {
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
    fn odd_parity_ends_at_right_edge() {
        let s = fill(
            &rect(100.0, 20.0),
            1.0,
            0.6,
            5.0,
            CornerStyle::Rectangular,
            RowParity::Odd,
            &mut Vec::new(),
        )
        .unwrap();
        let start = s.first().unwrap().start();
        let end = s.last().unwrap().end();
        assert!(start.x < 10.0, "starts at left ({:.2})", start.x);
        assert!(end.x > 90.0, "ends at right ({:.2})", end.x);
    }

    #[test]
    fn wavy_rows_undulate_but_stay_in_band() {
        let s = fill_wavy(
            &rect(100.0, 20.0),
            1.0,
            0.6,
            0.0,
            CornerStyle::Rectangular,
            &mut Vec::new(),
        )
        .unwrap();
        assert_path_well_formed(&s, 0.6, 0.6 - 0.31, 100.0 - 0.6, 20.0 - 0.6 + 0.31);
        // Some segment must deviate from its row line (the wave exists).
        let wavy = s.iter().any(|seg| {
            matches!(seg, PathSeg::Line { a, b } if (a.y - b.y).abs() > 0.05 && (a.x - b.x).abs() > 1e-9)
        });
        assert!(wavy, "no undulation found");
        // Far more segments than the plain serpentine's ~2 per row.
        assert!(s.len() > 200, "only {} segments", s.len());
    }

    #[test]
    fn counterflow_terminals_adjacent_and_path_connected() {
        for style in CornerStyle::ALL {
            let mut w = Vec::new();
            let s = fill_counterflow(&rect(100.0, 20.0), 1.0, 0.6, 5.0, style, &mut w).unwrap();
            assert_path_well_formed(&s, 0.0, 0.0, 100.0, 20.0);
            let start = s.first().unwrap().start();
            let end = s.last().unwrap().end();
            // Both terminals at the left edge, one pitch apart vertically.
            assert!(start.x < 10.0 && end.x < 10.0, "{style:?}");
            assert!(
                ((end.y - start.y) - 1.0).abs() < 1e-6,
                "{style:?}: terminal separation {}",
                end.y - start.y
            );
            assert!(start.y < end.y, "{style:?}: start above end");
        }
    }

    #[test]
    fn counterflow_length_roughly_matches_full_coverage() {
        let s = fill_counterflow(
            &rect(100.0, 20.0),
            1.0,
            0.6,
            0.0,
            CornerStyle::Smooth,
            &mut Vec::new(),
        )
        .unwrap();
        // Full coverage at pitch 1.0 over ~99×18.8 usable: ~1700–2000 mm.
        let len = path_len(&s);
        assert!(
            len > 1400.0 && len < 2100.0,
            "counterflow length {len} out of expected range"
        );
    }

    #[test]
    fn tiny_outline_rejected() {
        assert!(fill(
            &rect(2.0, 1.0),
            1.0,
            0.6,
            0.0,
            CornerStyle::Rectangular,
            RowParity::Even,
            &mut Vec::new()
        )
        .is_err());
    }
}
