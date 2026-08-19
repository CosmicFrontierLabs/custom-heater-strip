//! Heater strip design engine.
//!
//! Takes an SVG outline plus electrical constraints (voltage, target wattage,
//! current ceiling) and produces a serpentine copper trace that dissipates the
//! requested power, along with fab outputs: KiCad board, Gerbers, SVG preview,
//! and a numeric design report.

mod arrangement;
pub mod dxf;
mod fills;
pub mod geom;
mod gerber;
mod kicad;
mod outline;
mod preview;
mod regions;
mod silk;
mod solver;
mod terminals;

use shared::{DesignReport, DesignRequest, DesignResponse, GeometrySpec};

pub use outline::Polygon;
pub use terminals::{Pad, PadRect};

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
    #[error("DXF parse failed: {0}")]
    DxfParse(String),
    #[error("no closed rings found in the DXF")]
    NoDxfPolygons,
    #[error("geometry selection is incomplete: {0}")]
    BadGeometry(String),
    #[error("could not package the gerber archive: {0}")]
    Archive(String),
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
    /// Board profile, on Edge.Cuts.
    pub outline: Polygon,
    /// The heated regions. A plain strip has exactly one (the outline); a
    /// design from a DXF selection has one per chosen heater polygon, filled
    /// and chained in series.
    pub regions: Vec<Polygon>,
    /// The full trace centerline (pad to pad), in mm.
    pub trace: Vec<PathSeg>,
    pub trace_width_mm: f64,
    /// The two solder pads, in [input, output] order.
    pub pads: Vec<Pad>,
    /// Silkscreen legend strokes (voltage/resistance/power/stackup notes).
    pub silk: silk::Silk,
    pub report: DesignReport,
}

/// Run the full pipeline: outline → solved trace → fab outputs.
///
/// The response is complete, archive included: nothing downstream has to
/// finish assembling it, which is what lets the same call serve an HTTP
/// handler or run directly in the browser.
pub fn generate(req: &DesignRequest) -> Result<DesignResponse, EngineError> {
    let design = design(req)?;
    let gerbers = gerber::render(&design);
    let gerber_zip_base64 = zip_gerbers(&gerbers)?;
    Ok(DesignResponse {
        preview_svg: preview::render(&design),
        kicad_pcb: kicad::render(&design),
        gerbers,
        gerber_zip_base64,
        report: design.report,
    })
}

/// Bundle the gerber layer set into a base64-encoded zip, ready to hand
/// straight to a browser download.
fn zip_gerbers(
    gerbers: &std::collections::BTreeMap<String, String>,
) -> Result<String, EngineError> {
    use base64::Engine as _;
    use std::io::Write as _;

    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in gerbers {
            zip.start_file(name, opts)
                .and_then(|()| zip.write_all(body.as_bytes()).map_err(Into::into))
                .map_err(|e| EngineError::Archive(e.to_string()))?;
        }
        zip.finish()
            .map_err(|e| EngineError::Archive(e.to_string()))?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(buf))
}

/// Solve the electrical + geometric design without generating output files.
///
/// Two input paths: an SVG outline routed as a single region with
/// auto-placed pads, or an explicit DXF polygon selection whose heater
/// regions are chained in series between the user's own solder tabs.
pub fn design(req: &DesignRequest) -> Result<Design, EngineError> {
    match &req.geometry {
        Some(spec) => design_from_geometry(req, spec),
        None => design_from_svg(req),
    }
}

/// The single-region path: one SVG outline, pads placed in a pocket at the
/// left edge, feeds routed through the pocket lanes.
fn design_from_svg(req: &DesignRequest) -> Result<Design, EngineError> {
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
        fills::FillSpec {
            kind: req.fill_kind,
            outline: &outline,
            pitch_mm: solved.pitch_mm,
            inset_mm: inset,
            reserve: plan.reserve,
            style: req.corner_style,
            terminals: fills::Terminals::SameSide,
        },
        &mut warnings,
    )?;

    // Full electrical path: pad A → feed → fill pattern → feed → pad B.
    let row_start = fill_path.first().expect("nonempty path").start();
    let row_end = fill_path.last().expect("nonempty path").end();
    let (feed_in, feed_out) = match req.fill_kind {
        // The bifilar pattern finishes with both arms side by side, one pitch
        // apart, so a single shared lane would run the two feeds on top of
        // each other. Nested lanes instead.
        shared::FillKind::Counterflow => plan.feeds_adjacent(row_start, row_end),
        // Center-exit patterns leave from inside the pocket, so they use the
        // pocket-internal right lane; everything else uses the full-height
        // corridor left of the pads.
        kind => {
            let lane = match kind {
                shared::FillKind::DoubleSpiral | shared::FillKind::Concentric => {
                    terminals::Lane::Right
                }
                _ => terminals::Lane::Left,
            };
            (
                plan.feed_start(lane, row_start),
                plan.feed_end(lane, row_end),
            )
        }
    };
    let mut trace = feed_in;
    trace.extend(fill_path);
    trace.extend(feed_out);
    let length_mm: f64 = trace.iter().map(|s| s.length()).sum();

    let refined = solver::refine(req, &solved, length_mm, &mut warnings);
    check_on_board(
        &trace,
        &outline,
        refined.width_mm,
        req.edge_margin_mm,
        &mut warnings,
    )?;

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
        region_count: 1,
        link_length_mm: 0.0,
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
        regions: vec![outline.clone()],
        outline,
        trace,
        trace_width_mm: refined.width_mm,
        pads: plan.pads.into_iter().map(Pad::Rect).collect(),
        silk,
        report,
    })
}

/// The DXF path: fill each selected heater polygon and chain them in series
/// between the two tab polygons, which become the pad copper themselves.
fn design_from_geometry(req: &DesignRequest, spec: &GeometrySpec) -> Result<Design, EngineError> {
    let mut warnings = Vec::new();

    let to_polygon = |ring: &Vec<[f64; 2]>| Polygon {
        points: ring.iter().map(|p| Point::new(p[0], p[1])).collect(),
    };

    let heaters: Vec<Polygon> = spec.heaters.iter().map(to_polygon).collect();
    if heaters.is_empty() {
        return Err(EngineError::BadGeometry(
            "select at least one polygon as a heater region".into(),
        ));
    }
    for h in &heaters {
        if h.points.len() < 3 {
            return Err(EngineError::BadGeometry(
                "a heater region has fewer than three points".into(),
            ));
        }
    }

    let (Some(tab_in_ring), Some(tab_out_ring)) = (&spec.tab_in, &spec.tab_out) else {
        return Err(EngineError::BadGeometry(
            "both an input and an output solder tab must be selected".into(),
        ));
    };
    let tab_in = Pad::Poly(to_polygon(tab_in_ring));
    let tab_out = Pad::Poly(to_polygon(tab_out_ring));

    // Heated area is the sum over regions; the electrical solve is unchanged
    // because resistance depends on total routed length, not on how many
    // pieces the copper is spread across.
    let area_mm2: f64 = heaters.iter().map(|h| h.area_mm2()).sum();
    if area_mm2 < 1.0 {
        return Err(EngineError::OutlineTooSmall(format!(
            "selected heater area is {area_mm2:.3} mm²; expected at least 1 mm²"
        )));
    }

    let solved = solver::solve(req, area_mm2)?;
    let inset = req.edge_margin_mm + solved.width_mm / 2.0;
    // Fill must clear pad copper by an edge margin plus half a trace.
    let pad_clearance = req.edge_margin_mm + solved.width_mm / 2.0 + req.min_gap_mm;

    let chain = regions::plan(&heaters, &tab_in, &tab_out, pad_clearance, &mut warnings)?;
    let routed = regions::route(
        regions::RouteSpec {
            chain: &chain,
            kind: req.fill_kind,
            pitch_mm: solved.pitch_mm,
            inset_mm: inset,
            style: req.corner_style,
            tab_in: &tab_in,
            tab_out: &tab_out,
        },
        &mut warnings,
    )?;
    let regions::Routed {
        trace,
        link_indices,
        link_length_mm,
    } = routed;

    // Straight links are correct when a tab sits near the terminals it feeds.
    // When it does not, say so rather than shipping a short.
    let crossings = regions::count_link_crossings(&trace, &link_indices);
    if crossings > 0 {
        warnings.push(format!(
            "{crossings} place(s) where a connecting run crosses other copper. \
             Move the solder tabs nearer the edge of their heater region, or \
             reorder the regions, before sending this to fab."
        ));
    }

    let length_mm: f64 = trace.iter().map(|s| s.length()).sum();
    let refined = solver::refine(req, &solved, length_mm, &mut warnings);

    let outline = match &spec.outline {
        Some(ring) => to_polygon(ring),
        // No explicit profile: wrap everything with an edge margin.
        None => bounding_outline(
            heaters
                .iter()
                .chain(std::iter::once(&Polygon {
                    points: tab_in.ring(),
                }))
                .chain(std::iter::once(&Polygon {
                    points: tab_out.ring(),
                })),
            req.edge_margin_mm,
        ),
    };

    check_on_board(
        &trace,
        &outline,
        refined.width_mm,
        req.edge_margin_mm,
        &mut warnings,
    )?;

    if link_length_mm > 0.25 * length_mm {
        warnings.push(format!(
            "{:.0}% of the trace length is interconnect between regions and \
             tabs, which heats outside the heater areas",
            100.0 * link_length_mm / length_mm
        ));
    }

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
        region_count: chain.regions.len(),
        link_length_mm,
        copper_thickness_um: solved.thickness_m * 1e6,
        warnings,
    };

    // The legend goes on the board, clear of the copper: reserve the width of
    // the leftmost heater region so it lands beside the artwork, not over it.
    let mut silk_warnings = Vec::new();
    let silk = silk::generate(&outline, 0.0, req, &report, &mut silk_warnings);
    report.warnings.extend(silk_warnings);

    Ok(Design {
        outline,
        regions: chain.regions.into_iter().map(|r| r.polygon).collect(),
        trace,
        trace_width_mm: refined.width_mm,
        pads: vec![tab_in, tab_out],
        silk,
        report,
    })
}

/// Reject a routed trace whose copper does not fit on the board, and warn when
/// it fits but tighter than asked for.
///
/// This is not a theoretical check. The terminal pocket and feed lanes are
/// placed from the outline's *bounding box*, which silently assumes the
/// outline fills it — true for a rectangle, false for anything concave. On a
/// letter-S outline the serpentine's feed lane ran 10 mm off the board,
/// through the empty space beside the lower stroke, and every output file was
/// produced without complaint.
fn check_on_board(
    trace: &[PathSeg],
    outline: &Polygon,
    width_mm: f64,
    edge_margin_mm: f64,
    warnings: &mut Vec<String>,
) -> Result<(), EngineError> {
    // Copper physically leaving the board: unmanufacturable, so refuse it.
    let off = geom::find_escapes(trace, outline, width_mm, 0.0);
    if let Some(worst) = off
        .iter()
        .min_by(|a, b| a.clearance_mm.partial_cmp(&b.clearance_mm).unwrap())
    {
        return Err(EngineError::Infeasible(format!(
            "the routed trace leaves the board outline at ({:.2}, {:.2}) mm, by \
             {:.2} mm, in {} place(s). The terminal pocket and feed lanes are \
             laid out from the outline's bounding box, so a concave outline can \
             put them off the board — try the concentric or double-spiral fill, \
             which route from the outline itself.",
            worst.at.x,
            worst.at.y,
            -worst.clearance_mm,
            off.len()
        )));
    }

    // On the board, but closer to the edge than the requested margin.
    let tight = geom::find_escapes(trace, outline, width_mm, edge_margin_mm);
    if let Some(worst) = tight
        .iter()
        .min_by(|a, b| a.clearance_mm.partial_cmp(&b.clearance_mm).unwrap())
    {
        warnings.push(format!(
            "trace comes within {:.3} mm of the board edge at ({:.2}, {:.2}) mm, \
             inside the {edge_margin_mm:.2} mm margin requested ({} place(s))",
            worst.clearance_mm - width_mm / 2.0,
            worst.at.x,
            worst.at.y,
            tight.len()
        ));
    }
    Ok(())
}

/// Axis-aligned box around a set of polygons, grown by `margin_mm`.
fn bounding_outline<'a>(polys: impl Iterator<Item = &'a Polygon>, margin_mm: f64) -> Polygon {
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for poly in polys {
        let (lo, hi) = poly.bbox();
        min.x = min.x.min(lo.x);
        min.y = min.y.min(lo.y);
        max.x = max.x.max(hi.x);
        max.y = max.y.max(hi.y);
    }
    let m = margin_mm;
    Polygon {
        points: vec![
            Point::new(min.x - m, min.y - m),
            Point::new(max.x + m, min.y - m),
            Point::new(max.x + m, max.y + m),
            Point::new(min.x - m, max.y + m),
        ],
    }
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

    fn ring(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    /// Three 30×20 regions in a row with tabs off either end.
    fn chained_request(regions: usize) -> DesignRequest {
        let heaters = (0..regions)
            .map(|i| {
                let x = 10.0 + i as f64 * 40.0;
                ring(x, 5.0, x + 30.0, 25.0)
            })
            .collect();
        DesignRequest {
            geometry: Some(GeometrySpec {
                outline: None,
                heaters,
                tab_in: Some(ring(2.0, 12.0, 8.0, 18.0)),
                tab_out: Some(ring(
                    10.0 + regions as f64 * 40.0,
                    12.0,
                    16.0 + regions as f64 * 40.0,
                    18.0,
                )),
            }),
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
    fn chained_regions_form_one_continuous_trace() {
        let d = design(&chained_request(3)).unwrap();
        assert_eq!(d.report.region_count, 3);
        assert_eq!(d.regions.len(), 3);
        // Continuity is what makes it a single heater: every segment must
        // start where the previous one ended.
        let mut prev: Option<Point> = None;
        for seg in &d.trace {
            if let Some(p) = prev {
                assert!(
                    p.dist(&seg.start()) < 1e-6,
                    "gap at ({:.4},{:.4})",
                    p.x,
                    p.y
                );
            }
            prev = Some(seg.end());
        }
        // The chain must physically start on one pad and end on the other.
        let start = d.trace.first().unwrap().start();
        let end = d.trace.last().unwrap().end();
        assert!(start.dist(&d.pads[0].center()) < 1e-6, "{start:?}");
        assert!(end.dist(&d.pads[1].center()) < 1e-6, "{end:?}");
    }

    #[test]
    fn chained_design_hits_target_resistance() {
        let d = design(&chained_request(3)).unwrap();
        let r = &d.report;
        assert!((r.target_resistance_ohms - 14.4).abs() < 1e-9);
        let err = (r.achieved_resistance_ohms - r.target_resistance_ohms).abs()
            / r.target_resistance_ohms;
        assert!(
            err < 0.25,
            "achieved {} vs target {}",
            r.achieved_resistance_ohms,
            r.target_resistance_ohms
        );
        // Heated area is the sum of the three 30×20 regions = 1800 mm².
        assert!(
            (r.outline_area_cm2 - 18.0).abs() < 0.01,
            "{}",
            r.outline_area_cm2
        );
        // Links exist (tabs sit outside the regions) but stay a small share.
        assert!(r.link_length_mm > 0.0);
        assert!(r.link_length_mm < 0.25 * r.trace_length_mm);
    }

    #[test]
    fn tabs_become_polygon_pads_in_every_output() {
        let resp = generate(&chained_request(2)).unwrap();
        // Polygon pads are Gerber regions, not aperture flashes.
        let cu = &resp.gerbers["heater-F_Cu.gtl"];
        assert!(cu.contains("G36*"), "no region fill for the polygon pads");
        assert!(cu.contains("G37*"));
        assert!(
            resp.gerbers["heater-F_Mask.gts"].contains("G36*"),
            "mask openings should be regions too"
        );
        // KiCad gets filled polygons on both copper and mask.
        assert_eq!(
            resp.kicad_pcb.matches("(gr_poly (pts").count(),
            4,
            "expected 2 pads × (F.Cu + F.Mask)"
        );
        let open = resp.kicad_pcb.matches('(').count();
        assert_eq!(
            open,
            resp.kicad_pcb.matches(')').count(),
            "unbalanced sexpr"
        );
    }

    #[test]
    fn a_single_region_with_tabs_still_works() {
        // One 60×40 region: a lone 30×20 patch cannot dissipate 10 W at 12 V
        // without going under the fab's trace minimum, which the solver
        // rightly rejects.
        let req = DesignRequest {
            geometry: Some(GeometrySpec {
                outline: None,
                heaters: vec![ring(10.0, 5.0, 70.0, 45.0)],
                tab_in: Some(ring(2.0, 22.0, 8.0, 28.0)),
                tab_out: Some(ring(72.0, 22.0, 78.0, 28.0)),
            }),
            ..chained_request(1)
        };
        let d = design(&req).unwrap();
        assert_eq!(d.report.region_count, 1);
        assert!(d.report.trace_length_mm > 100.0);
        // Both tabs are polygons the user supplied, not auto-placed rects.
        assert!(matches!(d.pads[0], Pad::Poly(_)));
        assert!(matches!(d.pads[1], Pad::Poly(_)));
    }

    #[test]
    fn every_fill_kind_chains_across_regions() {
        for kind in shared::FillKind::ALL {
            let req = DesignRequest {
                fill_kind: kind,
                ..chained_request(2)
            };
            let d = design(&req).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            let mut prev: Option<Point> = None;
            for seg in &d.trace {
                if let Some(p) = prev {
                    assert!(p.dist(&seg.start()) < 1e-6, "{kind:?} path gap");
                }
                prev = Some(seg.end());
            }
            assert_eq!(d.report.region_count, 2, "{kind:?}");
        }
    }

    /// The payoff of splitting a region's terminals to opposite edges: the
    /// canonical layout — regions in a row, a tab off each end — routes with
    /// no copper touching copper anywhere.
    #[test]
    fn a_chain_of_regions_in_a_row_routes_without_shorting() {
        for regions in [1usize, 2, 3, 4] {
            let req = DesignRequest {
                voltage: 24.0,
                watts: 30.0,
                max_current: 3.0,
                ..chained_request(regions)
            };
            let d = match design(&req) {
                Ok(d) => d,
                // Small region counts can be electrically infeasible at these
                // specs; that is the solver's business, not the router's.
                Err(EngineError::Infeasible(_)) => continue,
                Err(e) => panic!("{regions} regions: {e}"),
            };
            let shorts = trace_shorts(&d);
            assert!(
                shorts.is_empty(),
                "{regions} regions: {} short(s), first at {:?}",
                shorts.len(),
                shorts.first()
            );
            assert!(
                d.report.warnings.iter().all(|w| !w.contains("crosses")),
                "{regions} regions warned about crossings: {:?}",
                d.report.warnings
            );
        }
    }

    #[test]
    fn geometry_without_both_tabs_is_rejected() {
        let mut req = chained_request(1);
        req.geometry.as_mut().unwrap().tab_out = None;
        assert!(matches!(design(&req), Err(EngineError::BadGeometry(_))));

        let mut req = chained_request(1);
        req.geometry.as_mut().unwrap().heaters.clear();
        assert!(matches!(design(&req), Err(EngineError::BadGeometry(_))));
    }

    #[test]
    fn an_interior_tab_is_notched_out_and_warned_about_if_it_splits() {
        // Tab planted in the middle of a single big region.
        let req = DesignRequest {
            geometry: Some(GeometrySpec {
                outline: None,
                heaters: vec![ring(0.0, 0.0, 60.0, 40.0)],
                tab_in: Some(ring(28.0, 18.0, 32.0, 22.0)),
                tab_out: Some(ring(65.0, 18.0, 70.0, 22.0)),
            }),
            ..chained_request(1)
        };
        let d = design(&req).unwrap();
        // The fill must not cover the pad it feeds.
        let pad_centre = d.pads[0].center();
        assert!(
            !d.regions[0].contains(pad_centre),
            "interior tab was left under copper"
        );
        assert!(d.report.trace_length_mm > 100.0);
    }

    #[test]
    fn explicit_outline_is_used_for_the_board_profile() {
        let mut req = chained_request(2);
        req.geometry.as_mut().unwrap().outline = Some(ring(-5.0, -5.0, 200.0, 60.0));
        let d = design(&req).unwrap();
        let (lo, hi) = d.outline.bbox();
        assert!(
            (lo.x + 5.0).abs() < 1e-9 && (hi.x - 200.0).abs() < 1e-9,
            "{lo:?} {hi:?}"
        );
    }

    /// Every index pair in a design's trace that shorts, arcs handled exactly.
    fn trace_shorts(d: &Design) -> Vec<(usize, usize)> {
        let all: Vec<usize> = (0..d.trace.len()).collect();
        geom::find_shorts(&d.trace, &all)
    }

    #[test]
    fn routed_traces_do_not_short_against_themselves() {
        for kind in shared::FillKind::ALL {
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

    /// The bifilar feeds must be *nested*, not overlaid. Before this was fixed
    /// they shared one lane `x`, which left the two runs collinear on top of
    /// each other for the whole height of the board -- so assert the two
    /// vertical traversals sit at genuinely different, manufacturably-spaced x.
    #[test]
    fn counterflow_feeds_run_in_two_separate_lanes() {
        let req = DesignRequest {
            fill_kind: shared::FillKind::Counterflow,
            ..rect_request()
        };
        let d = design(&req).unwrap();
        let pitch = d.report.trace_width_mm + d.report.trace_gap_mm;

        // The tall vertical runs are the lane traversals.
        let mut lanes: Vec<f64> = d
            .trace
            .iter()
            .filter_map(|s| match s {
                PathSeg::Line { a, b } if (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() > 5.0 => {
                    Some(a.x)
                }
                _ => None,
            })
            .collect();
        lanes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        lanes.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        assert_eq!(lanes.len(), 2, "expected two distinct lanes, got {lanes:?}");
        let separation = lanes[1] - lanes[0];
        assert!(
            separation >= pitch - 1e-9,
            "lanes {separation:.4} mm apart, tighter than the {pitch:.4} mm \
             routing pitch the fab can hold"
        );
    }

    /// A letter S: deeply concave, but concave in the way the scanline fill
    /// handles — every row crosses exactly one span of the shape.
    fn letter_s_svg() -> String {
        let (w, h, t) = (76.0, 116.0, 24.0);
        let (my0, my1) = ((h - t) / 2.0, (h - t) / 2.0 + t);
        let pts = [
            (0.0, 0.0),
            (w, 0.0),
            (w, t),
            (t, t),
            (t, my0),
            (w, my0),
            (w, h),
            (0.0, h),
            (0.0, h - t),
            (w - t, h - t),
            (w - t, my1),
            (0.0, my1),
        ];
        let d: String = pts
            .iter()
            .enumerate()
            .map(|(i, (x, y))| format!("{}{x} {y}", if i == 0 { 'M' } else { 'L' }))
            .collect::<Vec<_>>()
            .join("");
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}mm" height="{h}mm" viewBox="0 0 {w} {h}"><path d="{d}Z"/></svg>"##
        )
    }

    #[test]
    fn copper_stays_on_the_board_for_every_pattern() {
        for kind in shared::FillKind::ALL {
            let d = design(&DesignRequest {
                fill_kind: kind,
                ..rect_request()
            })
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            let escapes = geom::find_escapes(&d.trace, &d.outline, d.trace_width_mm, 0.0);
            assert!(
                escapes.is_empty(),
                "{kind:?}: copper leaves the board in {} place(s), worst {:?}",
                escapes.len(),
                escapes
                    .iter()
                    .min_by(|a, b| a.clearance_mm.partial_cmp(&b.clearance_mm).unwrap())
            );
        }
    }

    /// The bounding-box terminal placement puts the pocket and feed lanes off
    /// the board on a concave outline. That must be refused, not shipped: for
    /// a long time the letter-S serpentine emitted a full set of fab files
    /// with its feed lane running 10 mm beside the board.
    #[test]
    fn a_concave_outline_that_pushes_copper_off_the_board_is_rejected() {
        let req = DesignRequest {
            svg: letter_s_svg(),
            watts: 12.0,
            edge_margin_mm: 0.6,
            fill_kind: shared::FillKind::Serpentine,
            ..rect_request()
        };
        match design(&req) {
            Err(EngineError::Infeasible(msg)) => {
                assert!(msg.contains("leaves the board outline"), "{msg}");
            }
            Err(e) => panic!("wrong error: {e}"),
            Ok(_) => panic!("off-board copper was accepted"),
        }
    }

    /// The outline-following patterns handle the same shape correctly, which is
    /// what the rejection message tells the user to reach for.
    #[test]
    fn outline_following_patterns_route_the_letter_s_cleanly() {
        for kind in [shared::FillKind::Concentric, shared::FillKind::DoubleSpiral] {
            let d = design(&DesignRequest {
                svg: letter_s_svg(),
                watts: 12.0,
                edge_margin_mm: 0.6,
                fill_kind: kind,
                ..rect_request()
            })
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert!(
                geom::find_escapes(&d.trace, &d.outline, d.trace_width_mm, 0.0).is_empty(),
                "{kind:?} put copper off the board"
            );
            assert!(trace_shorts(&d).is_empty(), "{kind:?} shorts");
        }
    }

    #[test]
    fn the_gerber_zip_contains_every_layer() {
        use base64::Engine as _;

        let resp = generate(&rect_request()).unwrap();
        assert!(
            !resp.gerber_zip_base64.is_empty(),
            "generate must return a finished archive"
        );
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&resp.gerber_zip_base64)
            .unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        for expected in resp.gerbers.keys() {
            assert!(names.contains(expected), "{expected} missing from zip");
        }
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
