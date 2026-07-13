//! Double Archimedean spiral: two interleaved arms r = c·θ (phase 0 and π)
//! joined by a short bridge through the center. Radial gap between adjacent
//! passes is exactly the pitch; both arm ends exit on the left at the
//! centerline ± pitch/2 and run straight to the terminal zone, threading
//! the gap between the solder pads.
//!
//! Fills the inscribed circle around the fill-region center — ideal for
//! round outlines, coverage-limited on long strips (a warning reports the
//! covered fraction).

use crate::{outline::Polygon, EngineError, PathSeg, Point};

pub fn fill(
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
    left_reserved_mm: f64,
    warnings: &mut Vec<String>,
) -> Result<Vec<PathSeg>, EngineError> {
    let (min, max) = outline.bbox();
    let left = min.x + inset_mm + left_reserved_mm;
    let right = max.x - inset_mm;
    let top = min.y + inset_mm;
    let bottom = max.y - inset_mm;
    let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
    let center = Point::new(cx, cy);

    let r_max = dist_to_polygon(&center, outline)
        .min(cx - left)
        .min(right - cx)
        .min(cy - top)
        .min(bottom - cy)
        - inset_mm * 0.0; // outline distance already unpadded; insets above bound the box

    let c = pitch_mm / std::f64::consts::PI;
    let theta_max = r_max / c;
    if theta_max < 3.0 * std::f64::consts::TAU {
        return Err(EngineError::OutlineTooSmall(format!(
            "inscribed radius {r_max:.1} mm fits fewer than 3 spiral \
             revolutions at {pitch_mm:.2} mm pitch"
        )));
    }

    let coverage = std::f64::consts::PI * r_max * r_max / outline.area_mm2();
    if coverage < 0.9 {
        warnings.push(format!(
            "double spiral covers the inscribed circle only — about {:.0}% \
             of the outline; it suits round boards best",
            coverage * 100.0
        ));
    }

    // Arm end angles: arm 1 exits at absolute angle π+δ (y = cy − p/2),
    // arm 2 (phase π) at π−δ' (y = cy + p/2), both on their outermost pass.
    let tau = std::f64::consts::TAU;
    let pi = std::f64::consts::PI;
    let end_theta = |phase: f64, target_offset: f64| -> f64 {
        // Solve θ ≤ θ_max with (θ + phase) ≡ π + asin-ish offset (mod 2π).
        let mut theta = theta_max;
        for _ in 0..4 {
            let delta = (pitch_mm / 2.0 / (c * theta)).asin();
            let want = pi + target_offset * delta - phase;
            theta = want + tau * ((theta_max - want) / tau).floor();
        }
        theta
    };
    let theta1 = end_theta(0.0, 1.0);
    let theta2 = end_theta(pi, -1.0);

    let theta_join = pi / 2.0;
    let arm = |phase: f64, theta_end: f64| -> Vec<Point> {
        let mut pts = Vec::new();
        let mut theta = theta_join;
        while theta < theta_end {
            pts.push(spiral_point(center, c, theta, phase));
            // Adaptive step: ~0.25 mm chords.
            theta += (0.25 / (c * theta)).clamp(0.01, 0.5);
        }
        pts.push(spiral_point(center, c, theta_end, phase));
        pts
    };
    let arm1 = arm(0.0, theta1);
    let arm2 = arm(pi, theta2);

    // Assemble: stub in → arm1 outer→center → bridge → arm2 center→outer →
    // stub out. Terminals thread the pad gap at cy ± p/2.
    let t_a = Point::new(left, arm1.last().unwrap().y);
    let t_b = Point::new(left, arm2.last().unwrap().y);
    let mut pts: Vec<Point> = Vec::with_capacity(arm1.len() + arm2.len() + 4);
    pts.push(t_a);
    pts.extend(arm1.iter().rev());
    pts.extend(arm2.iter());
    pts.push(t_b);

    let mut path = Vec::with_capacity(pts.len() - 1);
    for w in pts.windows(2) {
        if w[0].dist(&w[1]) > 1e-9 {
            path.push(PathSeg::Line { a: w[0], b: w[1] });
        }
    }
    Ok(path)
}

fn spiral_point(center: Point, c: f64, theta: f64, phase: f64) -> Point {
    let r = c * theta;
    let angle = theta + phase;
    Point::new(center.x + r * angle.cos(), center.y + r * angle.sin())
}

/// Minimum distance from `p` to any edge of the polygon.
fn dist_to_polygon(p: &Point, poly: &Polygon) -> f64 {
    let pts = &poly.points;
    let n = pts.len();
    let mut best = f64::INFINITY;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = (dx * dx + dy * dy).max(1e-12);
        let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
        let proj = Point::new(a.x + t * dx, a.y + t * dy);
        best = best.min(p.dist(&proj));
    }
    best
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
    fn spiral_fills_square_with_adjacent_left_terminals() {
        let mut w = Vec::new();
        let path = fill(&rect(50.0, 50.0), 1.0, 0.6, 6.0, &mut w).unwrap();
        assert_path_well_formed(&path, 6.6 - 1e-6, 0.6, 50.0 - 0.6, 50.0 - 0.6);
        let start = path.first().unwrap().start();
        let end = path.last().unwrap().end();
        assert!((start.x - 6.6).abs() < 1e-6 && (end.x - 6.6).abs() < 1e-6);
        // Terminals straddle the centerline one pitch apart.
        assert!(
            ((end.y - start.y) - 1.0).abs() < 0.05,
            "{}",
            end.y - start.y
        );
        assert!(start.y < end.y);
    }

    #[test]
    fn strip_outline_warns_about_coverage() {
        let mut w = Vec::new();
        let path = fill(&rect(100.0, 20.0), 0.5, 0.6, 5.0, &mut w);
        let path = path.unwrap();
        assert!(w.iter().any(|m| m.contains("inscribed circle")), "{w:?}");
        assert!(!path.is_empty());
    }

    #[test]
    fn radial_pitch_is_uniform() {
        // Sample the path's distance from center along the +x axis: hits
        // should be ~pitch apart.
        let path = fill(&rect(50.0, 50.0), 1.0, 0.6, 0.0, &mut Vec::new()).unwrap();
        let (cx, cy) = (25.3, 25.0); // region center with zone 0: (0.6+49.4)/2=25.0 + shift
        let mut xs: Vec<f64> = path
            .iter()
            .filter_map(|s| {
                let (a, b) = (s.start(), s.end());
                // Segment crossing the horizontal ray y=cy, x>cx.
                if (a.y - cy) * (b.y - cy) <= 0.0 && (a.y - b.y).abs() > 1e-12 {
                    let t = (cy - a.y) / (b.y - a.y);
                    let x = a.x + t * (b.x - a.x);
                    (x > cx + 2.0).then_some(x)
                } else {
                    None
                }
            })
            .collect();
        xs.sort_by(|p, q| p.partial_cmp(q).unwrap());
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.3);
        assert!(xs.len() > 5, "too few crossings: {xs:?}");
        for pair in xs.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                (gap - 1.0).abs() < 0.15,
                "radial gap {gap:.3} deviates from pitch"
            );
        }
    }

    #[test]
    fn too_small_region_rejected() {
        assert!(fill(&rect(8.0, 8.0), 1.0, 0.6, 0.0, &mut Vec::new()).is_err());
    }
}
