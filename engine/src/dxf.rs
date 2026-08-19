//! DXF geometry extraction: pull every closed ring out of an uploaded DXF so
//! the user can pick which ones are heater regions and which are solder tabs.
//!
//! Coordinate handling: DXF is y-up and may be in any unit `$INSUNITS`
//! names. We scale to mm, flip to the y-down board frame, and translate the
//! whole drawing so its bounding box starts at (0, 0). Flipping about the
//! drawing's top edge (rather than negating y) keeps the board looking like
//! the CAD drawing instead of mirrored.

use dxf::entities::EntityType;
use dxf::enums::Units;
use dxf::Drawing;
use shared::{DxfPolygon, DxfUploadResponse, PolygonRole};

use crate::outline::Polygon;
use crate::{EngineError, Point};

/// Segments per full circle when tessellating arcs, circles, and ellipses.
/// At 64 a 20 mm-diameter circle is flat to under 8 µm — well inside fab
/// tolerance.
const CIRCLE_SEGMENTS: usize = 64;

/// A ring needs at least this much area (mm²) to be worth offering; below it
/// the entity is almost certainly a dimension tick or a stray dot.
const MIN_AREA_MM2: f64 = 0.01;

/// Parse a DXF and return every closed ring it contains, in board mm,
/// appending any advisories to `warnings`.
pub fn parse(bytes: &[u8], warnings: &mut Vec<String>) -> Result<Vec<DxfPolygon>, EngineError> {
    let resp = extract(bytes)?;
    warnings.extend(resp.warnings);
    Ok(resp.polygons)
}

/// Parse a DXF into the upload response the picker consumes.
///
/// Decodes the file once: the drawing units live in the same header as the
/// entities, so there is no reason to read it twice.
pub fn extract(bytes: &[u8]) -> Result<DxfUploadResponse, EngineError> {
    let warnings = &mut Vec::new();
    let mut cursor = std::io::BufReader::new(std::io::Cursor::new(bytes));
    let drawing = Drawing::load(&mut cursor).map_err(|e| EngineError::DxfParse(e.to_string()))?;

    let units = drawing.header.default_drawing_units;
    let units_label = format!("{units:?}");
    let scale = units_to_mm(units);
    if units == Units::Unitless {
        warnings.push(
            "DXF header has no $INSUNITS; interpreting coordinates as \
             millimeters. Set the drawing units in your CAD tool if the \
             board comes out the wrong size."
                .to_string(),
        );
    }

    // Collect rings in the DXF's own frame first — the y-flip needs the
    // drawing's overall extent, which we only know after reading everything.
    let mut raw: Vec<(String, String, Vec<Point>)> = Vec::new();
    let mut skipped_open = 0usize;
    // Bare LINE segments, gathered to be stitched into rings afterwards.
    let mut loose: Vec<(String, Point, Point)> = Vec::new();
    let (mut z_lo, mut z_hi) = (f64::INFINITY, f64::NEG_INFINITY);

    for entity in drawing.entities() {
        let layer = entity.common.layer.clone();
        let (kind, ring) = match &entity.specific {
            EntityType::LwPolyline(p) => {
                if !p.is_closed() {
                    skipped_open += 1;
                    continue;
                }
                let verts: Vec<(Point, f64)> = p
                    .vertices
                    .iter()
                    .map(|v| (Point::new(v.x, v.y), v.bulge))
                    .collect();
                ("LWPOLYLINE", tessellate_bulges(&verts))
            }
            EntityType::Polyline(p) => {
                if !p.is_closed() {
                    skipped_open += 1;
                    continue;
                }
                let verts: Vec<(Point, f64)> = p
                    .vertices()
                    .map(|v| (Point::new(v.location.x, v.location.y), v.bulge))
                    .collect();
                ("POLYLINE", tessellate_bulges(&verts))
            }
            EntityType::Circle(c) => (
                "CIRCLE",
                circle_ring(Point::new(c.center.x, c.center.y), c.radius, c.radius, 0.0),
            ),
            EntityType::Ellipse(e) => {
                // major_axis is a vector from the center; minor is that
                // rotated 90° and scaled by minor_axis_ratio.
                let major = (e.major_axis.x.powi(2) + e.major_axis.y.powi(2)).sqrt();
                let rot = e.major_axis.y.atan2(e.major_axis.x);
                (
                    "ELLIPSE",
                    circle_ring(
                        Point::new(e.center.x, e.center.y),
                        major,
                        major * e.minor_axis_ratio,
                        rot,
                    ),
                )
            }
            EntityType::Spline(s) => {
                if !s.is_closed() {
                    skipped_open += 1;
                    continue;
                }
                // Control points approximate a closed spline well enough for
                // a heater boundary; exact NURBS evaluation is overkill here.
                let pts: Vec<Point> = s
                    .control_points
                    .iter()
                    .map(|p| Point::new(p.x, p.y))
                    .collect();
                if pts.len() >= 3 {
                    warnings.push(format!(
                        "spline on layer \"{layer}\" approximated by its \
                         {} control points",
                        pts.len()
                    ));
                }
                ("SPLINE", pts)
            }
            // A bare LINE is not a ring by itself, but a great many exports
            // are nothing but LINEs — an outline drawn as separate segments.
            // Gather them and try to stitch rings out of them below.
            EntityType::Line(l) => {
                z_lo = z_lo.min(l.p1.z).min(l.p2.z);
                z_hi = z_hi.max(l.p1.z).max(l.p2.z);
                loose.push((
                    layer,
                    Point::new(l.p1.x, l.p1.y),
                    Point::new(l.p2.x, l.p2.y),
                ));
                continue;
            }
            _ => continue,
        };

        if ring.len() >= 3 {
            raw.push((layer, kind.to_string(), ring));
        }
    }

    if !loose.is_empty() {
        if z_hi - z_lo > 1e-9 {
            warnings.push(format!(
                "the line work spans {:.3} in z, so this is a 3D drawing; only \
                 its flat xy projection can be used as a board outline",
                z_hi - z_lo
            ));
        }
        for (layer, ring) in stitch_lines(&loose, warnings) {
            raw.push((layer, "LINES".to_string(), ring));
        }
    }

    if skipped_open > 0 {
        warnings.push(format!(
            "{skipped_open} open path(s) ignored; only closed rings can be \
             heater regions or tabs"
        ));
    }

    if raw.is_empty() {
        return Err(EngineError::NoDxfPolygons);
    }

    // Drawing extent in DXF units, for the flip + translate.
    let (mut min_x, mut min_y_extent) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (_, _, ring) in &raw {
        for p in ring {
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y_extent = min_y_extent.min(p.y);
            max_y = max_y.max(p.y);
        }
    }

    let mut out = Vec::new();
    let mut tiny = 0usize;
    for (layer, kind, ring) in raw {
        let points: Vec<[f64; 2]> = ring
            .iter()
            .map(|p| [(p.x - min_x) * scale, (max_y - p.y) * scale])
            .collect();
        let poly = Polygon {
            points: points.iter().map(|p| Point::new(p[0], p[1])).collect(),
        };
        let area = poly.area_mm2();
        if area < MIN_AREA_MM2 {
            tiny += 1;
            continue;
        }
        out.push(DxfPolygon {
            id: out.len() as u32,
            suggested_role: guess_role(&layer),
            layer,
            kind,
            points,
            area_mm2: area,
        });
    }

    if tiny > 0 {
        warnings.push(format!(
            "{tiny} ring(s) smaller than {MIN_AREA_MM2} mm² ignored"
        ));
    }
    if out.is_empty() {
        // Everything was found and then thrown away for being minute, which
        // almost always means the file is in metres or inches and said nothing
        // about it. Saying "no closed rings" would send the user looking for
        // the wrong problem entirely.
        if tiny > 0 {
            let (w, h) = ((max_x - min_x) * scale, (max_y - min_y_extent) * scale);
            let guess = unit_hint(w, h)
                .map(|h| format!(" Or {h}."))
                .unwrap_or_default();
            return Err(EngineError::DxfParse(format!(
                "found {tiny} closed outline(s), but at the {units_label} units \
                 this drawing declares it is only {w:.3} × {h:.3} mm — too small \
                 to be a board.{guess}"
            )));
        }
        return Err(EngineError::NoDxfPolygons);
    }

    let (draw_w, draw_h) = ((max_x - min_x) * scale, (max_y - min_y_extent) * scale);
    if draw_w.max(draw_h) < 5.0 {
        if let Some(hint) = unit_hint(draw_w, draw_h) {
            warnings.push(format!(
                "the whole drawing is only {draw_w:.3} × {draw_h:.3} mm; {hint}"
            ));
        }
    }

    // Largest first: the board outline and the big heater regions land at the
    // top of the picker, where they're easiest to click.
    out.sort_by(|a, b| b.area_mm2.partial_cmp(&a.area_mm2).unwrap());
    for (i, p) in out.iter_mut().enumerate() {
        p.id = i as u32;
    }
    Ok(DxfUploadResponse {
        polygons: out,
        units: units_label,
        warnings: std::mem::take(warnings),
    })
}

/// If a drawing is implausibly small, name the unit that would make it
/// plausible. A file that forgot `$INSUNITS` but was drawn in metres reads as
/// a fraction of a millimetre, and "too small" on its own sends the user
/// looking for the wrong problem.
fn unit_hint(w_mm: f64, h_mm: f64) -> Option<String> {
    [("metres", 1000.0), ("inches", 25.4), ("centimetres", 10.0)]
        .into_iter()
        .find(|(_, f)| (5.0..=500.0).contains(&(w_mm * f).max(h_mm * f)))
        .map(|(unit, f)| {
            format!(
                "drawn in {unit} it would be {:.1} × {:.1} mm, which looks right \
                 — set $INSUNITS accordingly and re-export",
                w_mm * f,
                h_mm * f
            )
        })
}

/// Turn bare `LINE` segments into closed composite polygons.
///
/// Plenty of exporters emit an outline as a pile of disconnected segments
/// rather than a polyline, and flattened 3D models add crossings, exact
/// duplicates and interior facet edges on top. All of that is handed to
/// [`crate::arrangement`], which splits the segments at their crossings and
/// traces the boundary of each connected piece.
fn stitch_lines(
    loose: &[(String, Point, Point)],
    warnings: &mut Vec<String>,
) -> Vec<(String, Vec<Point>)> {
    let segments: Vec<(Point, Point)> = loose.iter().map(|(_, a, b)| (*a, *b)).collect();
    let pieces = crate::arrangement::compose(&segments, warnings);
    if pieces.is_empty() {
        return Vec::new();
    }
    warnings.push(format!(
        "composed {} closed outline(s) from {} loose line segment(s)",
        pieces.len(),
        loose.len()
    ));
    // Loose lines carry no meaningful shared layer, so label by the commonest.
    let layer = loose.first().map(|(l, _, _)| l.clone()).unwrap_or_default();
    pieces
        .into_iter()
        .map(|p| (layer.clone(), p.ring))
        .collect()
}

/// Seed a role from the layer name so conventionally-named files come in
/// pre-assigned. The user can always override in the picker.
fn guess_role(layer: &str) -> PolygonRole {
    let l = layer.to_ascii_uppercase();
    let has = |needles: &[&str]| needles.iter().any(|n| l.contains(n));
    // Order matters: "TAB_IN" must not match the bare "TAB" branch first.
    if has(&["TAB_IN", "TABIN", "TAB-IN", "INPUT", "VCC", "SUPPLY"]) {
        PolygonRole::TabIn
    } else if has(&["TAB_OUT", "TABOUT", "TAB-OUT", "OUTPUT", "GND", "RETURN"]) {
        PolygonRole::TabOut
    } else if has(&["HEAT", "FILL", "ZONE"]) {
        PolygonRole::Heater
    } else if has(&["OUTLINE", "EDGE", "PROFILE", "BOARD"]) {
        PolygonRole::Outline
    } else {
        PolygonRole::Unused
    }
}

/// mm per unit for a `$INSUNITS` value.
fn units_to_mm(u: Units) -> f64 {
    match u {
        // Unitless drawings are assumed already-metric; the caller warns.
        Units::Unitless | Units::Millimeters => 1.0,
        Units::Inches => 25.4,
        Units::Feet => 304.8,
        Units::Miles => 1_609_344.0,
        Units::Centimeters => 10.0,
        Units::Meters => 1000.0,
        Units::Kilometers => 1_000_000.0,
        Units::Microinches => 25.4e-6,
        Units::Mils => 0.0254,
        Units::Yards => 914.4,
        Units::Angstroms => 1e-7,
        Units::Nanometers => 1e-6,
        Units::Microns => 1e-3,
        Units::Decimeters => 100.0,
        Units::Decameters => 10_000.0,
        Units::Hectometers => 100_000.0,
        Units::Gigameters => 1e12,
        Units::AstronomicalUnits => 1.495_978_707e14,
        Units::LightYears => 9.460_730_472_580_8e18,
        Units::Parsecs => 3.085_677_581e19,
        Units::USSurveyFeet => 304.800_609_601_219,
        Units::USSurveyInch => 25.400_050_800_101_6,
        Units::USSurveyYard => 914.401_828_803_658,
        Units::USSurveyMile => 1_609_347.218_694_437,
    }
}

/// Expand DXF bulge values into arc segments.
///
/// A bulge is tan(θ/4) for the arc sweeping from this vertex to the next,
/// signed: positive is counter-clockwise in the DXF's y-up frame. Zero means
/// a straight edge.
fn tessellate_bulges(verts: &[(Point, f64)]) -> Vec<Point> {
    let n = verts.len();
    if n < 2 {
        return verts.iter().map(|(p, _)| *p).collect();
    }
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let (a, bulge) = verts[i];
        let (b, _) = verts[(i + 1) % n];
        out.push(a);
        if bulge.abs() < 1e-12 || a.dist(&b) < 1e-12 {
            continue;
        }
        // θ = 4·atan(bulge); radius from the chord and the included angle.
        let theta = 4.0 * bulge.atan();
        let chord = a.dist(&b);
        let radius = chord / (2.0 * (theta / 2.0).sin()).abs();
        // Center sits on the chord's perpendicular bisector, offset by the
        // sagitta complement; sign follows the bulge's handedness.
        let mid = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let h = (radius * radius - (chord / 2.0).powi(2)).max(0.0).sqrt();
        let (dx, dy) = ((b.x - a.x) / chord, (b.y - a.y) / chord);
        // Perpendicular, flipped when the arc is the major one (|θ| > π).
        let sign = if theta > 0.0 { 1.0 } else { -1.0 };
        let major = theta.abs() > std::f64::consts::PI;
        let hs = if major { -h } else { h };
        let center = Point::new(mid.x + sign * hs * dy, mid.y - sign * hs * dx);

        let a0 = (a.y - center.y).atan2(a.x - center.x);
        let steps = ((theta.abs() / std::f64::consts::TAU) * CIRCLE_SEGMENTS as f64)
            .ceil()
            .max(2.0) as usize;
        // Interior points only — the next loop iteration pushes `b`.
        for s in 1..steps {
            let ang = a0 + theta * (s as f64 / steps as f64);
            out.push(Point::new(
                center.x + radius * ang.cos(),
                center.y + radius * ang.sin(),
            ));
        }
    }
    out
}

/// Tessellated ellipse ring (a circle when both radii match).
fn circle_ring(center: Point, rx: f64, ry: f64, rotation: f64) -> Vec<Point> {
    let (sr, cr) = rotation.sin_cos();
    (0..CIRCLE_SEGMENTS)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / CIRCLE_SEGMENTS as f64;
            let (x, y) = (rx * t.cos(), ry * t.sin());
            Point::new(center.x + x * cr - y * sr, center.y + x * sr + y * cr)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal ASCII DXF with a closed LWPOLYLINE rectangle on layer HEATER
    /// and a circle on layer TAB_IN, in millimeters.
    fn sample_dxf() -> Vec<u8> {
        let mut s = String::new();
        s.push_str("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n");
        s.push_str("0\nSECTION\n2\nENTITIES\n");
        // 40 × 10 rectangle at origin.
        s.push_str("0\nLWPOLYLINE\n8\nHEATER\n90\n4\n70\n1\n");
        for (x, y) in [(0.0, 0.0), (40.0, 0.0), (40.0, 10.0), (0.0, 10.0)] {
            s.push_str(&format!("10\n{x}\n20\n{y}\n"));
        }
        // r=2 circle.
        s.push_str("0\nCIRCLE\n8\nTAB_IN\n10\n50.0\n20\n5.0\n40\n2.0\n");
        s.push_str("0\nENDSEC\n0\nEOF\n");
        s.into_bytes()
    }

    #[test]
    fn extracts_rectangle_and_circle_with_roles() {
        let mut w = Vec::new();
        let polys = parse(&sample_dxf(), &mut w).unwrap();
        assert_eq!(polys.len(), 2, "{polys:#?}");

        // Sorted largest first: the 400 mm² rectangle, then the ~12.5 mm² circle.
        let rect = &polys[0];
        assert_eq!(rect.layer, "HEATER");
        assert_eq!(rect.kind, "LWPOLYLINE");
        assert_eq!(rect.suggested_role, PolygonRole::Heater);
        assert!((rect.area_mm2 - 400.0).abs() < 1e-6, "{}", rect.area_mm2);

        let circle = &polys[1];
        assert_eq!(circle.suggested_role, PolygonRole::TabIn);
        // Tessellated area is slightly under the true πr² = 12.566.
        assert!(
            (circle.area_mm2 - 12.566).abs() < 0.02,
            "{}",
            circle.area_mm2
        );
    }

    #[test]
    fn coordinates_land_in_a_y_down_frame_at_the_origin() {
        let mut w = Vec::new();
        let polys = parse(&sample_dxf(), &mut w).unwrap();
        let all: Vec<[f64; 2]> = polys.iter().flat_map(|p| p.points.clone()).collect();
        let min_x = all.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
        let min_y = all.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
        assert!(min_x.abs() < 1e-9, "min x should be 0, got {min_x}");
        assert!(min_y.abs() < 1e-9, "min y should be 0, got {min_y}");

        // The rectangle's DXF y=0 edge is the drawing's bottom, so after the
        // flip it must be the *largest* board y.
        let rect = &polys[0];
        let max_y = rect.points.iter().map(|p| p[1]).fold(0.0, f64::max);
        assert!((max_y - 10.0).abs() < 1e-9, "{max_y}");
    }

    #[test]
    fn inch_drawings_are_scaled_to_mm() {
        let dxf = String::from_utf8(sample_dxf())
            .unwrap()
            .replace("$INSUNITS\n70\n4", "$INSUNITS\n70\n1");
        let mut w = Vec::new();
        let polys = parse(dxf.as_bytes(), &mut w).unwrap();
        // 40 in × 10 in → 1016 mm × 254 mm → 258064 mm².
        assert!(
            (polys[0].area_mm2 - 400.0 * 25.4 * 25.4).abs() < 1e-3,
            "{}",
            polys[0].area_mm2
        );
    }

    #[test]
    fn open_polylines_are_reported_not_returned() {
        let dxf = String::from_utf8(sample_dxf())
            .unwrap()
            // 70/1 is the closed flag; 70/0 makes the rectangle an open path.
            .replace("90\n4\n70\n1", "90\n4\n70\n0");
        let mut w = Vec::new();
        let polys = parse(dxf.as_bytes(), &mut w).unwrap();
        assert_eq!(polys.len(), 1, "only the circle should survive");
        assert!(
            w.iter().any(|m| m.contains("open path")),
            "expected an open-path warning, got {w:?}"
        );
    }

    #[test]
    fn bulge_vertices_become_arc_points() {
        // Two vertices, bulge 1.0 = a half turn: a closed semicircle pair.
        let verts = vec![(Point::new(0.0, 0.0), 1.0), (Point::new(10.0, 0.0), 1.0)];
        let ring = tessellate_bulges(&verts);
        assert!(ring.len() > 20, "expected tessellation, got {}", ring.len());
        // Every point should sit on one of the two r=5 circles.
        for p in &ring {
            let d1 = p.dist(&Point::new(5.0, 0.0));
            assert!((d1 - 5.0).abs() < 1e-6 || d1 < 1e-6, "{p:?} d={d1}");
        }
        // A full circle of radius 5 → area ≈ 78.5.
        let area = Polygon {
            points: ring.clone(),
        }
        .area_mm2();
        assert!((area - 78.54).abs() < 0.5, "{area}");
    }

    /// A square drawn as four separate LINE entities, the way many exporters
    /// emit an outline. Before the arrangement went in, this parsed to nothing.
    #[test]
    fn a_square_of_loose_lines_is_composed_into_a_ring() {
        let mut s = String::from("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n");
        s.push_str("0\nSECTION\n2\nENTITIES\n");
        let corners = [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)];
        for i in 0..4 {
            let (x0, y0) = corners[i];
            let (x1, y1) = corners[(i + 1) % 4];
            s.push_str(&format!(
                "0\nLINE\n8\nOUTLINE\n10\n{x0}\n20\n{y0}\n30\n0.0\n11\n{x1}\n21\n{y1}\n31\n0.0\n"
            ));
        }
        s.push_str("0\nENDSEC\n0\nEOF\n");

        let mut w = Vec::new();
        let polys = parse(s.as_bytes(), &mut w).unwrap();
        assert_eq!(polys.len(), 1, "{polys:#?}");
        assert_eq!(polys[0].kind, "LINES");
        assert!(
            (polys[0].area_mm2 - 1200.0).abs() < 1e-6,
            "{}",
            polys[0].area_mm2
        );
        assert!(
            w.iter().any(|m| m.contains("composed 1 closed outline")),
            "{w:?}"
        );
    }

    /// A flattened extrusion: the same square top and bottom, joined by
    /// verticals that collapse to points when projected. It must still yield
    /// exactly one ring, not two stacked ones.
    #[test]
    fn a_flattened_extrusion_collapses_to_one_ring() {
        let mut s = String::from("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n");
        s.push_str("0\nSECTION\n2\nENTITIES\n");
        let corners = [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)];
        for z in [0.0, 12.0] {
            for i in 0..4 {
                let (x0, y0) = corners[i];
                let (x1, y1) = corners[(i + 1) % 4];
                s.push_str(&format!(
                    "0\nLINE\n8\nBOX\n10\n{x0}\n20\n{y0}\n30\n{z}\n11\n{x1}\n21\n{y1}\n31\n{z}\n"
                ));
            }
        }
        for (x, y) in corners {
            s.push_str(&format!(
                "0\nLINE\n8\nBOX\n10\n{x}\n20\n{y}\n30\n0.0\n11\n{x}\n21\n{y}\n31\n12.0\n"
            ));
        }
        s.push_str("0\nENDSEC\n0\nEOF\n");

        let mut w = Vec::new();
        let polys = parse(s.as_bytes(), &mut w).unwrap();
        assert_eq!(polys.len(), 1, "{polys:#?}");
        assert!(
            (polys[0].area_mm2 - 1200.0).abs() < 1e-6,
            "{}",
            polys[0].area_mm2
        );
        assert!(w.iter().any(|m| m.contains("3D drawing")), "{w:?}");
    }

    /// A drawing in metres with no `$INSUNITS` reads as a fraction of a
    /// millimetre. Saying "no closed rings" would be actively misleading, so
    /// the error names the unit that would make it plausible.
    #[test]
    fn a_metre_scale_drawing_is_diagnosed_not_dismissed() {
        let mut s = String::from("0\nSECTION\n2\nENTITIES\n");
        let corners = [(0.0, 0.0), (0.1, 0.0), (0.1, 0.08), (0.0, 0.08)];
        for i in 0..4 {
            let (x0, y0) = corners[i];
            let (x1, y1) = corners[(i + 1) % 4];
            s.push_str(&format!(
                "0\nLINE\n8\nM\n10\n{x0}\n20\n{y0}\n30\n0.0\n11\n{x1}\n21\n{y1}\n31\n0.0\n"
            ));
        }
        s.push_str("0\nENDSEC\n0\nEOF\n");

        let err = parse(s.as_bytes(), &mut Vec::new()).expect_err("too small");
        let msg = err.to_string();
        assert!(msg.contains("too small to be a board"), "{msg}");
        assert!(msg.contains("metres"), "{msg}");
    }

    #[test]
    fn empty_dxf_is_an_error() {
        let dxf = b"0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n";
        let mut w = Vec::new();
        assert!(matches!(
            parse(dxf, &mut w),
            Err(EngineError::NoDxfPolygons)
        ));
    }
}
