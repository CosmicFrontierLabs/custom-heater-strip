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
        trace_d(&design.trace),
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

    // Rectangular terminal pads.
    for p in &design.pads {
        let _ = write!(
            svg,
            r##"<rect x="{:.4}" y="{:.4}" width="{:.4}" height="{:.4}" fill="#e8c56b" stroke="#8c6d1f" stroke-width="0.15"/>"##,
            p.cx - p.w / 2.0,
            p.cy - p.h / 2.0,
            p.w,
            p.h
        );
    }

    svg.push_str("</svg>");
    svg
}

/// SVG `d` string for the routed trace: lines as `L`, arcs as `A` commands.
fn trace_d(segs: &[crate::PathSeg]) -> String {
    use crate::PathSeg;
    let mut d = String::with_capacity(segs.len() * 24);
    for (i, seg) in segs.iter().enumerate() {
        if i == 0 {
            let s = seg.start();
            let _ = write!(d, "M{:.4} {:.4}", s.x, s.y);
        }
        match seg {
            PathSeg::Line { b, .. } => {
                let _ = write!(d, "L{:.4} {:.4}", b.x, b.y);
            }
            PathSeg::Arc { b, ccw, .. } => {
                // SVG's sweep=1 is the positive-angle direction in its
                // y-down frame, matching our `ccw` convention.
                let _ = write!(
                    d,
                    "A{r:.4} {r:.4} 0 0 {sweep} {x:.4} {y:.4}",
                    r = seg.radius(),
                    sweep = if *ccw { 1 } else { 0 },
                    x = b.x,
                    y = b.y
                );
            }
        }
    }
    d
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
                crate::PathSeg::Line {
                    a: Point::new(1.0, 1.0),
                    b: Point::new(9.0, 1.0),
                },
                crate::PathSeg::Arc {
                    a: Point::new(9.0, 1.0),
                    b: Point::new(9.0, 4.0),
                    center: Point::new(9.0, 2.5),
                    ccw: true,
                },
                crate::PathSeg::Line {
                    a: Point::new(9.0, 4.0),
                    b: Point::new(1.0, 4.0),
                },
            ],
            trace_width_mm: 0.4,
            pads: [
                crate::PadRect {
                    cx: 1.0,
                    cy: 1.0,
                    w: 2.0,
                    h: 1.25,
                },
                crate::PadRect {
                    cx: 1.0,
                    cy: 4.0,
                    w: 2.0,
                    h: 1.25,
                },
            ],
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
        assert_eq!(svg.matches("<rect").count(), 2, "two rectangular pads");
        // Arc turnaround renders as an SVG arc command with sweep=1.
        assert!(svg.contains("A1.5000 1.5000 0 0 1 9.0000 4.0000"), "{svg}");
        // Pad rect derives from center and size.
        assert!(svg.contains(r#"<rect x="0.0000" y="0.3750" width="2.0000" height="1.2500""#));
    }
}
