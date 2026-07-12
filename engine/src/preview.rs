//! SVG preview rendering of a routed heater design.
//!
//! Path/viewBox helpers follow the conventions in pastebom's
//! `pcb-extract/src/svg.rs` (`M/L` d-strings, 4-decimal coordinates).

use std::fmt::Write;

use crate::{Design, Point};

/// Render the design as a standalone SVG: outline in fab purple, copper
/// serpentine in copper orange, endpoint terminals marked.
pub fn render(design: &Design) -> String {
    let (min, max) = design.outline.bbox();
    let margin = 2.0;
    let (vx, vy) = (min.x - margin, min.y - margin);
    let (vw, vh) = (max.x - min.x + 2.0 * margin, max.y - min.y + 2.0 * margin);

    let mut svg = String::new();
    let _ = write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vx:.3} {vy:.3} {vw:.3} {vh:.3}" width="{vw:.1}mm" height="{vh:.1}mm">"#
    );

    // Board substrate (polyimide amber, translucent).
    let _ = write!(
        svg,
        r##"<path d="{}" fill="#c77f2e" fill-opacity="0.25" stroke="#7a4faf" stroke-width="0.2"/>"##,
        polyline_d(&design.outline.points, true)
    );

    // Copper serpentine.
    let _ = write!(
        svg,
        r##"<path d="{}" fill="none" stroke="#d98f3d" stroke-width="{:.4}" stroke-linecap="round" stroke-linejoin="round"/>"##,
        polyline_d(&design.trace, false),
        design.trace_width_mm
    );

    // Silkscreen legend.
    for stroke in &design.silk.strokes {
        let _ = write!(
            svg,
            r##"<path d="{}" fill="none" stroke="#e8ecf5" stroke-width="{:.4}" stroke-linecap="round" stroke-linejoin="round"/>"##,
            polyline_d(stroke, false),
            design.silk.stroke_mm
        );
    }

    // Terminal markers at the two ends of the trace.
    if let (Some(a), Some(b)) = (design.trace.first(), design.trace.last()) {
        let r = (design.trace_width_mm * 1.2).max(0.8);
        for p in [a, b] {
            let _ = write!(
                svg,
                r##"<circle cx="{:.4}" cy="{:.4}" r="{r:.3}" fill="#e8c56b" stroke="#8c6d1f" stroke-width="0.15"/>"##,
                p.x, p.y
            );
        }
    }

    svg.push_str("</svg>");
    svg
}

fn polyline_d(pts: &[Point], close: bool) -> String {
    let mut d = String::with_capacity(pts.len() * 18);
    for (i, p) in pts.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(d, "{cmd}{:.4} {:.4}", p.x, p.y);
    }
    if close {
        d.push('Z');
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outline::Polygon;
    use shared::DesignReport;

    #[test]
    fn preview_contains_outline_and_trace() {
        let design = Design {
            outline: Polygon {
                points: vec![
                    Point::new(0.0, 0.0),
                    Point::new(10.0, 0.0),
                    Point::new(10.0, 5.0),
                    Point::new(0.0, 5.0),
                ],
            },
            trace: vec![
                Point::new(1.0, 1.0),
                Point::new(9.0, 1.0),
                Point::new(9.0, 4.0),
                Point::new(1.0, 4.0),
            ],
            trace_width_mm: 0.4,
            silk: crate::silk::Silk {
                strokes: vec![],
                stroke_mm: 0.15,
            },
            report: DesignReport {
                target_resistance_ohms: 1.0,
                achieved_resistance_ohms: 1.0,
                achieved_watts: 1.0,
                operating_current_amps: 1.0,
                current_headroom_frac: 0.5,
                trace_width_mm: 0.4,
                trace_gap_mm: 0.2,
                trace_length_mm: 19.0,
                outline_area_cm2: 0.5,
                power_density_w_cm2: 2.0,
                copper_thickness_um: 17.4,
                warnings: vec![],
            },
        };
        let svg = render(&design);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("stroke-width=\"0.4000\""));
        assert!(svg.matches("<circle").count() == 2);
    }
}
