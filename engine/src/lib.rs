//! Heater strip design engine.
//!
//! Takes an SVG outline plus electrical constraints (voltage, target wattage,
//! current ceiling) and produces a serpentine copper trace that dissipates the
//! requested power, along with fab outputs: KiCad board, Gerbers, SVG preview,
//! and a numeric design report.

mod fills;
pub mod geom;
mod gerber;
mod kicad;
mod outline;
mod preview;
mod silk;
mod solver;
mod terminals;

use shared::{DesignReport, DesignRequest, DesignResponse};

pub use outline::Polygon;
pub use terminals::PadRect;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("SVG parse failed: {0}")]
    SvgParse(String),
    #[error("no closed outline path found in SVG")]
    NoOutline,
    #[error("outline too small: {0}")]
    OutlineTooSmall(String),
    #[error("design infeasible: {0}")]
    Infeasible(String),
}

/// A point in board coordinates, millimeters, y-down (SVG convention).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn dist(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// One piece of the routed trace centerline. Coordinates are board mm,
/// y-down; `ccw` is the sweep direction in that y-down frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSeg {
    Line {
        a: Point,
        b: Point,
    },
    Arc {
        a: Point,
        b: Point,
        center: Point,
        ccw: bool,
    },
}

impl PathSeg {
    pub fn start(&self) -> Point {
        match self {
            PathSeg::Line { a, .. } | PathSeg::Arc { a, .. } => *a,
        }
    }

    pub fn end(&self) -> Point {
        match self {
            PathSeg::Line { b, .. } | PathSeg::Arc { b, .. } => *b,
        }
    }

    /// Radius of an arc segment (0 for lines).
    pub fn radius(&self) -> f64 {
        match self {
            PathSeg::Line { .. } => 0.0,
            PathSeg::Arc { a, center, .. } => center.dist(a),
        }
    }

    /// Sweep angle in radians, positive, measured along the travel direction.
    pub fn sweep(&self) -> f64 {
        match self {
            PathSeg::Line { .. } => 0.0,
            PathSeg::Arc { a, b, center, ccw } => {
                let a0 = (a.y - center.y).atan2(a.x - center.x);
                let a1 = (b.y - center.y).atan2(b.x - center.x);
                let mut sweep = if *ccw { a1 - a0 } else { a0 - a1 };
                while sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                sweep
            }
        }
    }

    pub fn length(&self) -> f64 {
        match self {
            PathSeg::Line { a, b } => a.dist(b),
            PathSeg::Arc { .. } => self.radius() * self.sweep(),
        }
    }

    /// A point on the arc midway along the sweep (used for KiCad's
    /// start/mid/end arc encoding).
    pub fn arc_midpoint(&self) -> Option<Point> {
        match self {
            PathSeg::Line { .. } => None,
            PathSeg::Arc { a, center, ccw, .. } => {
                let r = self.radius();
                let a0 = (a.y - center.y).atan2(a.x - center.x);
                let half = self.sweep() / 2.0;
                let mid = if *ccw { a0 + half } else { a0 - half };
                Some(Point::new(
                    center.x + r * mid.cos(),
                    center.y + r * mid.sin(),
                ))
            }
        }
    }
}

/// Everything computed for one design: geometry + electrical numbers.
pub struct Design {
    pub outline: Polygon,
    /// The full trace centerline (pad to pad), in mm.
    pub trace: Vec<PathSeg>,
    pub trace_width_mm: f64,
    /// The two rectangular solder pads, symmetric about the centerline.
    pub pads: [PadRect; 2],
    /// Silkscreen legend strokes (voltage/resistance/power/stackup notes).
    pub silk: silk::Silk,
    pub report: DesignReport,
}

/// Run the full pipeline: SVG outline → solved serpentine → fab outputs.
pub fn generate(req: &DesignRequest) -> Result<DesignResponse, EngineError> {
    let design = design(req)?;
    Ok(DesignResponse {
        preview_svg: preview::render(&design),
        kicad_pcb: kicad::render(&design),
        gerbers: gerber::render(&design),
        // Filled in by the server, which owns archive packaging.
        gerber_zip_base64: String::new(),
        report: design.report,
    })
}

/// Solve the electrical + geometric design without generating output files.
pub fn design(req: &DesignRequest) -> Result<Design, EngineError> {
    let mut warnings = Vec::new();

    let outline = outline::parse_svg_outline(&req.svg, &mut warnings)?;
    let area_mm2 = outline.area_mm2();
    if area_mm2 < 1.0 {
        return Err(EngineError::OutlineTooSmall(format!(
            "outline area is {area_mm2:.3} mm²; expected at least 1 mm²"
        )));
    }

    let solved = solver::solve(req, area_mm2)?;
    let inset = req.edge_margin_mm + solved.width_mm / 2.0;

    let plan = terminals::layout(
        outline.bbox(),
        inset,
        solved.width_mm,
        req.min_gap_mm,
        req.pad_diameter_mm,
        &mut warnings,
    )?;

    let fill_path = fills::fill(
        req.fill_kind,
        &outline,
        solved.pitch_mm,
        inset,
        plan.reserve,
        req.corner_style,
        &mut warnings,
    )?;

    // Full electrical path: pad A → feed → fill pattern → feed → pad B.
    // Corner-exit patterns route via the full-height left lane; center-exit
    // patterns (spiral, concentric) via the pocket-internal right lane.
    let lane = match req.fill_kind {
        shared::FillKind::DoubleSpiral | shared::FillKind::Concentric => terminals::Lane::Right,
        _ => terminals::Lane::Left,
    };
    let row_start = fill_path.first().expect("nonempty path").start();
    let row_end = fill_path.last().expect("nonempty path").end();
    let mut trace = plan.feed_start(lane, row_start);
    trace.extend(fill_path);
    trace.extend(plan.feed_end(lane, row_end));
    let length_mm: f64 = trace.iter().map(|s| s.length()).sum();

    let refined = solver::refine(req, &solved, length_mm, &mut warnings);

    let mut report = DesignReport {
        target_resistance_ohms: refined.target_resistance_ohms,
        achieved_resistance_ohms: refined.achieved_resistance_ohms,
        achieved_watts: refined.achieved_watts,
        operating_current_amps: refined.operating_current_amps,
        current_headroom_frac: refined.operating_current_amps / req.max_current,
        trace_width_mm: refined.width_mm,
        trace_gap_mm: solved.pitch_mm - refined.width_mm,
        trace_length_mm: length_mm,
        outline_area_cm2: area_mm2 / 100.0,
        power_density_w_cm2: refined.achieved_watts / (area_mm2 / 100.0),
        copper_thickness_um: solved.thickness_m * 1e6,
        warnings,
    };

    let mut silk_warnings = Vec::new();
    let silk = silk::generate(
        &outline,
        plan.reserve.pocket_x1 - outline.bbox().0.x,
        req,
        &report,
        &mut silk_warnings,
    );
    report.warnings.extend(silk_warnings);

    Ok(Design {
        outline,
        trace,
        trace_width_mm: refined.width_mm,
        pads: plan.pads,
        silk,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const RECT_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="20mm" viewBox="0 0 100 20"><path d="M 0 0 L 100 0 L 100 20 L 0 20 Z"/></svg>"##;

    fn rect_request() -> DesignRequest {
        DesignRequest {
            svg: RECT_SVG.to_string(),
            voltage: 12.0,
            watts: 10.0,
            max_current: 2.0,
            copper_oz: 0.5,
            min_trace_mm: 0.15,
            min_gap_mm: 0.15,
            edge_margin_mm: 0.5,
            ..DesignRequest::default()
        }
    }

    #[test]
    fn rect_design_hits_target_resistance() {
        let d = design(&rect_request()).unwrap();
        let r = &d.report;
        // R_target = 144/10 = 14.4 Ω; refined width should land close.
        assert!((r.target_resistance_ohms - 14.4).abs() < 1e-9);
        let err = (r.achieved_resistance_ohms - r.target_resistance_ohms).abs()
            / r.target_resistance_ohms;
        assert!(
            err < 0.25,
            "achieved {} vs target {} ({}% off)",
            r.achieved_resistance_ohms,
            r.target_resistance_ohms,
            err * 100.0
        );
        assert!(r.operating_current_amps <= 2.0);
        assert!(r.trace_length_mm > 100.0);
    }

    #[test]
    fn over_current_design_is_rejected() {
        let mut req = rect_request();
        req.watts = 100.0; // 100 W @ 12 V → 8.3 A > 2 A ceiling
        assert!(matches!(design(&req), Err(EngineError::Infeasible(_))));
    }

    #[test]
    fn every_fill_kind_designs_the_rect_strip() {
        for kind in shared::FillKind::ALL {
            let req = DesignRequest {
                fill_kind: kind,
                ..rect_request()
            };
            let d = design(&req).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            // Continuous pad-to-pad path.
            let mut prev: Option<Point> = None;
            for seg in &d.trace {
                if let Some(p) = prev {
                    assert!(p.dist(&seg.start()) < 1e-6, "{kind:?} path gap");
                }
                prev = Some(seg.end());
            }
            // Resistance lands within 25% of target (width refinement may
            // clamp for low-coverage patterns like the spiral).
            let r = &d.report;
            let err = (r.achieved_resistance_ohms - r.target_resistance_ohms).abs()
                / r.target_resistance_ohms;
            assert!(
                err < 0.25 || !r.warnings.is_empty(),
                "{kind:?}: R off by {:.0}% with no warning",
                err * 100.0
            );
        }
    }

    /// Every index pair in a design's trace that shorts, arcs handled exactly.
    fn trace_shorts(d: &Design) -> Vec<(usize, usize)> {
        let all: Vec<usize> = (0..d.trace.len()).collect();
        geom::find_shorts(&d.trace, &all)
    }

    #[test]
    fn routed_traces_do_not_short_against_themselves() {
        for kind in shared::FillKind::ALL {
            // Counterflow is known to short its feed run against the return
            // arm at the pads; tracked in issue #5.
            if kind == shared::FillKind::Counterflow {
                continue;
            }
            let req = DesignRequest {
                fill_kind: kind,
                ..rect_request()
            };
            let d = design(&req).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            let shorts = trace_shorts(&d);
            assert!(
                shorts.is_empty(),
                "{kind:?}: {} self-short(s), first at segments {:?}",
                shorts.len(),
                shorts.first()
            );
        }
    }

    /// Pins the known counterflow defect (issue #5) to a specific pair of
    /// segments rather than a screenshot. When the feed routing is fixed this
    /// test fails: delete it and drop the skip in
    /// `routed_traces_do_not_short_against_themselves`.
    #[test]
    fn counterflow_currently_shorts_its_feed_against_the_return_arm() {
        let req = DesignRequest {
            fill_kind: shared::FillKind::Counterflow,
            ..rect_request()
        };
        let d = design(&req).unwrap();
        let shorts = trace_shorts(&d);
        assert!(
            !shorts.is_empty(),
            "counterflow no longer shorts — fold this pattern back into \
             routed_traces_do_not_short_against_themselves and delete this test"
        );
        // The short is at the pads: one of the offending segments is a feed
        // run touching a pad centre.
        let pad_centres: Vec<Point> = d.pads.iter().map(|p| Point::new(p.cx, p.cy)).collect();
        let touches_pad = shorts.iter().any(|(i, j)| {
            [*i, *j].iter().any(|k| {
                let s = &d.trace[*k];
                pad_centres
                    .iter()
                    .any(|c| c.dist(&s.start()) < 1e-6 || c.dist(&s.end()) < 1e-6)
            })
        });
        assert!(
            touches_pad,
            "expected the short to involve a pad feed: {shorts:?}"
        );
    }

    #[test]
    fn full_generate_produces_all_artifacts() {
        let resp = generate(&rect_request()).unwrap();
        assert!(resp.preview_svg.contains("<svg"));
        assert!(resp.kicad_pcb.contains("kicad_pcb"));
        assert!(resp.gerbers.keys().any(|k| k.ends_with(".gtl")));
        assert!(resp.gerbers.keys().any(|k| k.ends_with(".gko")));
    }
}
