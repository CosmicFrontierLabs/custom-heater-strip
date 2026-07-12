//! Boustrophedon serpentine fill: horizontal scanlines across the outline,
//! connected end-to-end in alternating directions.

use crate::{outline::Polygon, EngineError, Point};

pub struct Serpentine {
    pub path: Vec<Point>,
    pub length_mm: f64,
}

/// Fill `outline` with a serpentine of the given pitch, keeping `inset` mm of
/// clearance from the boundary (edge margin + half trace width).
pub fn fill(
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
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

    let rows = (usable_height / pitch_mm).floor() as usize + 1;
    if rows < 2 {
        return Err(EngineError::OutlineTooSmall(format!(
            "only {rows} serpentine row(s) fit; outline is too small for the \
             required {pitch_mm:.2} mm pitch"
        )));
    }
    // Center the rows vertically in the usable band.
    let y_start = y_lo + (usable_height - (rows - 1) as f64 * pitch_mm) / 2.0;

    let mut path: Vec<Point> = Vec::new();
    let mut multi_span_rows = 0usize;
    let mut dropped_rows = 0usize;
    let mut leftward = false;

    for row in 0..rows {
        let y = y_start + row as f64 * pitch_mm;
        let hits = outline.scanline_hits(y);
        // Pair up even-odd crossings into inside spans, shrunk by the inset.
        let mut spans: Vec<(f64, f64)> = hits
            .chunks_exact(2)
            .map(|c| (c[0] + inset_mm, c[1] - inset_mm))
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
        let (x0, x1) = spans[0];
        let (start, end) = if leftward { (x1, x0) } else { (x0, x1) };
        path.push(Point::new(start, y));
        path.push(Point::new(end, y));
        leftward = !leftward;
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

    if path.len() < 4 {
        return Err(EngineError::OutlineTooSmall(
            "fewer than two serpentine rows could be routed".into(),
        ));
    }

    let length_mm = path.windows(2).map(|w| w[0].dist(&w[1])).sum();
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

    #[test]
    fn rect_fill_length_close_to_estimate() {
        let poly = rect(100.0, 20.0);
        let pitch = 1.0;
        let inset = 0.6;
        let mut w = Vec::new();
        let s = fill(&poly, pitch, inset, &mut w).unwrap();
        // ~19 rows of ~98.8 mm each plus connectors.
        let est = (20.0 - 2.0 * inset) / pitch * (100.0 - 2.0 * inset);
        assert!(
            (s.length_mm - est).abs() / est < 0.15,
            "len {} vs est {}",
            s.length_mm,
            est
        );
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn path_stays_inside_rect() {
        let poly = rect(50.0, 10.0);
        let inset = 0.5;
        let s = fill(&poly, 0.8, inset, &mut Vec::new()).unwrap();
        for p in &s.path {
            assert!(p.x >= inset - 1e-9 && p.x <= 50.0 - inset + 1e-9);
            assert!(p.y >= inset - 1e-9 && p.y <= 10.0 - inset + 1e-9);
        }
    }

    #[test]
    fn tiny_outline_rejected() {
        let poly = rect(2.0, 1.0);
        assert!(fill(&poly, 1.0, 0.6, &mut Vec::new()).is_err());
    }
}
