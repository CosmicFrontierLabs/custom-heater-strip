//! RS-274X (Gerber X2) output: top copper, top soldermask terminal openings,
//! and board outline.
//!
//! Format choices (X2 `%TF.FileFunction` strings, 4.6 mm coordinate format)
//! match what pastebom's Gerber reader (`pcb-extract/src/parsers/gerber`)
//! accepts, so uploads of our own output round-trip for visual checking.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::{Design, Point};

/// mm → X4.6 integer coordinate.
fn coord(mm: f64) -> i64 {
    (mm * 1e6).round() as i64
}

/// Gerber Y is up; board coordinates are y-down. Negate consistently across
/// all layers.
fn xy(p: &Point) -> (i64, i64) {
    (coord(p.x), coord(-p.y))
}

struct GerberFile {
    body: String,
}

impl GerberFile {
    fn new(file_function: &str, polarity: &str) -> Self {
        let mut body = String::new();
        let _ = writeln!(
            body,
            "%TF.GenerationSoftware,custom-heater-strip,engine,0.1*%"
        );
        let _ = writeln!(body, "%TF.FileFunction,{file_function}*%");
        let _ = writeln!(body, "%TF.FilePolarity,{polarity}*%");
        body.push_str("%FSLAX46Y46*%\n%MOMM*%\nG01*\n%LPD*%\n");
        Self { body }
    }

    fn circle_aperture(&mut self, dcode: u32, diameter_mm: f64) {
        let _ = writeln!(self.body, "%ADD{dcode}C,{diameter_mm:.6}*%");
    }

    fn rect_aperture(&mut self, dcode: u32, w_mm: f64, h_mm: f64) {
        let _ = writeln!(self.body, "%ADD{dcode}R,{w_mm:.6}X{h_mm:.6}*%");
    }

    fn select(&mut self, dcode: u32) {
        let _ = writeln!(self.body, "D{dcode}*");
    }

    fn polyline(&mut self, pts: &[Point]) {
        for (i, p) in pts.iter().enumerate() {
            let (x, y) = xy(p);
            let op = if i == 0 { "D02" } else { "D01" };
            let _ = writeln!(self.body, "X{x}Y{y}{op}*");
        }
    }

    /// Stroke a mixed line/arc path. Arcs use multi-quadrant (G75) circular
    /// interpolation with I/J center offsets, returning to linear (G01)
    /// afterwards.
    fn path(&mut self, segs: &[crate::PathSeg]) {
        for (i, seg) in segs.iter().enumerate() {
            if i == 0 {
                let (x, y) = xy(&seg.start());
                let _ = writeln!(self.body, "X{x}Y{y}D02*");
            }
            match seg {
                crate::PathSeg::Line { b, .. } => {
                    let (x, y) = xy(b);
                    let _ = writeln!(self.body, "X{x}Y{y}D01*");
                }
                crate::PathSeg::Arc { a, b, center, ccw } => {
                    // The y-axis flip to Gerber's y-up frame reverses the
                    // sweep handedness: board-ccw becomes G02 (clockwise).
                    let g = if *ccw { "G02" } else { "G03" };
                    let (x, y) = xy(b);
                    let i_off = coord(center.x - a.x);
                    let j_off = coord(-(center.y - a.y));
                    let _ = writeln!(self.body, "G75*");
                    let _ = writeln!(self.body, "{g}X{x}Y{y}I{i_off}J{j_off}D01*");
                    let _ = writeln!(self.body, "G01*");
                }
            }
        }
    }

    fn flash(&mut self, p: &Point) {
        let (x, y) = xy(p);
        let _ = writeln!(self.body, "X{x}Y{y}D03*");
    }

    fn finish(mut self) -> String {
        self.body.push_str("M02*\n");
        self.body
    }
}

pub fn render(design: &Design) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let [pad_a, pad_b] = design.pads;

    // Top copper: serpentine strokes + rectangular terminal pads.
    let mut cu = GerberFile::new("Copper,L1,Top", "Positive");
    cu.circle_aperture(10, design.trace_width_mm);
    cu.rect_aperture(11, pad_a.w, pad_a.h);
    cu.select(10);
    cu.path(&design.trace);
    cu.select(11);
    for p in [&pad_a, &pad_b] {
        cu.flash(&Point::new(p.cx, p.cy));
    }
    files.insert("heater-F_Cu.gtl".to_string(), cu.finish());

    // Top soldermask: openings over the pads so they're solderable.
    let mut mask = GerberFile::new("Soldermask,Top", "Negative");
    mask.rect_aperture(10, pad_a.w + 0.1, pad_a.h + 0.1);
    mask.select(10);
    for p in [&pad_a, &pad_b] {
        mask.flash(&Point::new(p.cx, p.cy));
    }
    files.insert("heater-F_Mask.gts".to_string(), mask.finish());

    // Top silkscreen: stroked-text legend (specs + stackup).
    if !design.silk.strokes.is_empty() {
        let mut silk = GerberFile::new("Legend,Top", "Positive");
        silk.circle_aperture(10, design.silk.stroke_mm);
        silk.select(10);
        for stroke in &design.silk.strokes {
            silk.polyline(stroke);
        }
        files.insert("heater-F_Silk.gto".to_string(), silk.finish());
    }

    // Board outline.
    let mut edge = GerberFile::new("Profile,NP", "Positive");
    edge.circle_aperture(10, 0.1);
    edge.select(10);
    let mut ring = design.outline.points.clone();
    if let Some(first) = ring.first().copied() {
        ring.push(first);
    }
    edge.polyline(&ring);
    files.insert("heater-Edge_Cuts.gko".to_string(), edge.finish());

    files
}

#[cfg(test)]
mod tests {
    use crate::tests::RECT_SVG;
    use shared::DesignRequest;

    #[test]
    fn gerber_set_is_well_formed() {
        let req = DesignRequest {
            svg: RECT_SVG.to_string(),
            ..DesignRequest::default()
        };
        let d = crate::design(&req).unwrap();
        let files = super::render(&d);
        assert_eq!(files.len(), 4);
        assert!(
            files.contains_key("heater-F_Silk.gto"),
            "missing silkscreen legend layer"
        );
        for (name, body) in &files {
            assert!(body.starts_with("%TF."), "{name} missing X2 header");
            assert!(body.ends_with("M02*\n"), "{name} missing EOF");
            assert!(body.contains("%MOMM*%"), "{name} not metric");
        }
        let cu = &files["heater-F_Cu.gtl"];
        assert_eq!(cu.matches("D03*").count(), 2, "expected 2 terminal flashes");
        assert!(cu.contains("D01*"), "no draw commands in copper layer");
        assert!(cu.contains("%ADD11R,"), "pads should use a rect aperture");
        assert!(
            files["heater-F_Mask.gts"].contains("%ADD10R,"),
            "mask openings should be rectangular"
        );
    }

    #[test]
    fn smooth_corners_emit_true_gerber_arcs() {
        let req = DesignRequest {
            svg: RECT_SVG.to_string(),
            corner_style: shared::CornerStyle::Smooth,
            ..DesignRequest::default()
        };
        let d = crate::design(&req).unwrap();
        let cu = &super::render(&d)["heater-F_Cu.gtl"];
        assert!(cu.contains("G75*"), "multi-quadrant mode not enabled");
        // Board-frame CCW right turns become G02 after the y flip; a full
        // serpentine turns both ways.
        assert!(cu.contains("G02X"), "no clockwise arcs");
        assert!(cu.contains("G03X"), "no counter-clockwise arcs");
        assert!(cu.contains("J-"), "arc J offsets should be signed");
    }
}
