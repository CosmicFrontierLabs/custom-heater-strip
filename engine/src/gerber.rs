//! RS-274X (Gerber X2) output: top copper, top soldermask terminal openings,
//! and board outline.
//!
//! Format choices (X2 `%TF.FileFunction` strings, 4.6 mm coordinate format)
//! match what pastebom's Gerber reader (`pcb-extract/src/parsers/gerber`)
//! accepts, so uploads of our own output round-trip for visual checking.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::terminals::Pad;
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

    /// Flood-fill a closed contour as a Gerber region (G36/G37). Used for
    /// solder tabs whose shape came from the user's DXF, where no standard
    /// aperture would do.
    fn region(&mut self, ring: &[Point]) {
        if ring.len() < 3 {
            return;
        }
        let _ = writeln!(self.body, "G36*");
        self.polyline(ring);
        // Close the contour explicitly; some CAM readers require it.
        let (x, y) = xy(&ring[0]);
        let _ = writeln!(self.body, "X{x}Y{y}D01*");
        let _ = writeln!(self.body, "G37*");
    }

    fn finish(mut self) -> String {
        self.body.push_str("M02*\n");
        self.body
    }
}

/// Soldermask relief around each pad, mm.
const MASK_GROW_MM: f64 = 0.05;

pub fn render(design: &Design) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();

    // Top copper: trace strokes + terminal pads. Rectangular pads flash a
    // rect aperture; DXF-shaped pads are emitted as filled regions.
    let mut cu = GerberFile::new("Copper,L1,Top", "Positive");
    cu.circle_aperture(10, design.trace_width_mm);
    let rect_dcode = rect_pad_aperture(&mut cu, design, 0.0);
    cu.select(10);
    cu.path(&design.trace);
    emit_pads(&mut cu, design, rect_dcode, 0.0);
    files.insert("heater-F_Cu.gtl".to_string(), cu.finish());

    // Top soldermask: openings over the pads so they're solderable.
    let mut mask = GerberFile::new("Soldermask,Top", "Negative");
    let mask_dcode = rect_pad_aperture(&mut mask, design, MASK_GROW_MM);
    emit_pads(&mut mask, design, mask_dcode, MASK_GROW_MM);
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

/// Declare a rect aperture sized for the design's rectangular pads, if it has
/// any. Returns the D-code to select, or `None` when every pad is a polygon.
fn rect_pad_aperture(f: &mut GerberFile, design: &Design, grow: f64) -> Option<u32> {
    let r = design.pads.iter().find_map(|p| match p {
        Pad::Rect(r) => Some(*r),
        Pad::Poly(_) => None,
    })?;
    let g = r.grown(grow);
    f.rect_aperture(11, g.w, g.h);
    Some(11)
}

/// Flash the rectangular pads and flood-fill the polygon ones.
fn emit_pads(f: &mut GerberFile, design: &Design, rect_dcode: Option<u32>, grow: f64) {
    for pad in &design.pads {
        match pad {
            Pad::Rect(r) => {
                if let Some(d) = rect_dcode {
                    f.select(d);
                    f.flash(&r.center());
                }
            }
            Pad::Poly(_) => f.region(&pad.grown_ring(grow)),
        }
    }
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
            files["heater-F_Mask.gts"].contains("%ADD11R,"),
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
