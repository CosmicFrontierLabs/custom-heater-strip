//! Minimal `.kicad_pcb` writer: heater serpentine as F.Cu track segments,
//! board outline as Edge.Cuts graphics.
//!
//! Tag/coordinate conventions (mm, y-down, `segment`/`gr_line`/layer names)
//! follow the grammar documented by pastebom's KiCad parser
//! (`pcb-extract/src/parsers/kicad.rs`).

use std::fmt::Write;

use crate::Design;

const BOARD_VERSION: &str = "20240108"; // KiCad 8 file format

pub fn render(design: &Design) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "(kicad_pcb\n  (version {BOARD_VERSION})\n  (generator \"custom-heater-strip\")\n  (generator_version \"0.1\")"
    );
    s.push_str(
        r#"  (general
    (thickness 0.2)
    (legacy_teardrops no)
  )
  (paper "A4")
  (layers
    (0 "F.Cu" signal)
    (31 "B.Cu" signal)
    (36 "B.SilkS" user "B.Silkscreen")
    (37 "F.SilkS" user "F.Silkscreen")
    (38 "B.Mask" user)
    (39 "F.Mask" user)
    (44 "Edge.Cuts" user)
  )
  (setup
    (pad_to_mask_clearance 0)
  )
  (net 0 "")
  (net 1 "HEATER")
"#,
    );

    // Board outline on Edge.Cuts.
    let pts = &design.outline.points;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        let _ = writeln!(
            s,
            "  (gr_line (start {:.4} {:.4}) (end {:.4} {:.4}) (stroke (width 0.05) (type solid)) (layer \"Edge.Cuts\"))",
            a.x, a.y, b.x, b.y
        );
    }

    // Silkscreen legend strokes on F.SilkS (same artwork as the gerber
    // legend layer, so the KiCad view matches what gets printed).
    for stroke in &design.silk.strokes {
        for w in stroke.windows(2) {
            let _ = writeln!(
                s,
                "  (gr_line (start {:.4} {:.4}) (end {:.4} {:.4}) (stroke (width {:.3}) (type solid)) (layer \"F.SilkS\"))",
                w[0].x, w[0].y, w[1].x, w[1].y, design.silk.stroke_mm
            );
        }
    }

    // Serpentine as connected track segments on F.Cu.
    for w in design.trace.windows(2) {
        let _ = writeln!(
            s,
            "  (segment (start {:.4} {:.4}) (end {:.4} {:.4}) (width {:.4}) (layer \"F.Cu\") (net 1))",
            w[0].x, w[0].y, w[1].x, w[1].y, design.trace_width_mm
        );
    }

    s.push_str(")\n");
    s
}

#[cfg(test)]
mod tests {
    use crate::tests::RECT_SVG;
    use shared::DesignRequest;

    #[test]
    fn kicad_output_has_tracks_and_outline() {
        let req = DesignRequest {
            svg: RECT_SVG.to_string(),
            ..DesignRequest::default()
        };
        let d = crate::design(&req).unwrap();
        let pcb = super::render(&d);
        assert!(pcb.contains("(segment (start"));
        assert!(pcb.contains("Edge.Cuts"));
        // Balanced parens — cheap structural sanity check.
        let open = pcb.matches('(').count();
        let close = pcb.matches(')').count();
        assert_eq!(open, close);
    }
}
