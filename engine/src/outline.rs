//! SVG outline extraction: pull the board outline polygon out of an uploaded
//! SVG document. Coordinates are converted to millimeters (96 dpi, matching
//! Inkscape's user-unit convention).

use usvg::tiny_skia_path::PathSegment;

use crate::{EngineError, Point};

/// Millimeters per SVG user unit (px) at 96 dpi.
const MM_PER_PX: f64 = 25.4 / 96.0;

/// Segments used to flatten each quadratic/cubic Bézier span.
const BEZIER_STEPS: usize = 16;

/// A closed polygon in mm, y-down.
#[derive(Debug, Clone)]
pub struct Polygon {
    pub points: Vec<Point>,
}

impl Polygon {
    /// Unsigned shoelace area, mm².
    pub fn area_mm2(&self) -> f64 {
        signed_area(&self.points).abs()
    }

    pub fn bbox(&self) -> (Point, Point) {
        let mut min = Point::new(f64::INFINITY, f64::INFINITY);
        let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in &self.points {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
        (min, max)
    }

    /// Even-odd x-intersections of the horizontal line at `y`, sorted.
    pub fn scanline_hits(&self, y: f64) -> Vec<f64> {
        let pts = &self.points;
        let n = pts.len();
        let mut xs = Vec::new();
        for i in 0..n {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            // Half-open rule so a vertex shared by two edges counts once.
            if (a.y <= y && b.y > y) || (b.y <= y && a.y > y) {
                let t = (y - a.y) / (b.y - a.y);
                xs.push(a.x + t * (b.x - a.x));
            }
        }
        xs.sort_by(|p, q| p.partial_cmp(q).unwrap());
        xs
    }
}

fn signed_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    let mut acc = 0.0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        acc += a.x * b.y - b.x * a.y;
    }
    acc / 2.0
}

/// Parse an SVG document and return its dominant closed path as the board
/// outline, in mm.
pub fn parse_svg_outline(svg: &str, warnings: &mut Vec<String>) -> Result<Polygon, EngineError> {
    let opts = usvg::Options::default();
    let tree =
        usvg::Tree::from_str(svg, &opts).map_err(|e| EngineError::SvgParse(e.to_string()))?;

    if !has_physical_units(svg) {
        warnings.push(
            "SVG has no physical units on its root width/height; interpreting \
             coordinates at 96 dpi (1 px = 0.2646 mm). Set width/height in mm \
             for exact sizing."
                .into(),
        );
    }

    let mut rings: Vec<Vec<Point>> = Vec::new();
    collect_rings(tree.root(), &mut rings);

    if rings.is_empty() {
        return Err(EngineError::NoOutline);
    }
    if rings.len() > 1 {
        warnings.push(format!(
            "SVG contains {} closed subpaths; using the largest as the outline \
             (holes/cutouts are not yet supported)",
            rings.len()
        ));
    }

    let ring = rings
        .into_iter()
        .max_by(|a, b| {
            signed_area(a)
                .abs()
                .partial_cmp(&signed_area(b).abs())
                .unwrap()
        })
        .unwrap();

    Ok(Polygon { points: ring })
}

fn has_physical_units(svg: &str) -> bool {
    // Cheap sniff of the root element only.
    let head = &svg[..svg.len().min(2048)];
    ["mm", "cm", "in", "pt"]
        .iter()
        .any(|u| head.contains(&format!("{u}\"")) || head.contains(&format!("{u}'")))
}

fn collect_rings(group: &usvg::Group, rings: &mut Vec<Vec<Point>>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => collect_rings(g, rings),
            usvg::Node::Path(p) => extract_path_rings(p, rings),
            _ => {}
        }
    }
}

fn extract_path_rings(path: &usvg::Path, rings: &mut Vec<Vec<Point>>) {
    let ts = path.abs_transform();
    let to_mm = |x: f32, y: f32| {
        let px = ts.sx * x + ts.kx * y + ts.tx;
        let py = ts.ky * x + ts.sy * y + ts.ty;
        Point::new(px as f64 * MM_PER_PX, py as f64 * MM_PER_PX)
    };

    let mut current: Vec<Point> = Vec::new();
    for seg in path.data().segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                finish_ring(&mut current, rings);
                current.push(to_mm(p.x, p.y));
            }
            PathSegment::LineTo(p) => current.push(to_mm(p.x, p.y)),
            PathSegment::QuadTo(c, p) => {
                let start = *current.last().expect("QuadTo without current point");
                let c = to_mm(c.x, c.y);
                let p = to_mm(p.x, p.y);
                for i in 1..=BEZIER_STEPS {
                    let t = i as f64 / BEZIER_STEPS as f64;
                    let u = 1.0 - t;
                    current.push(Point::new(
                        u * u * start.x + 2.0 * u * t * c.x + t * t * p.x,
                        u * u * start.y + 2.0 * u * t * c.y + t * t * p.y,
                    ));
                }
            }
            PathSegment::CubicTo(c1, c2, p) => {
                let start = *current.last().expect("CubicTo without current point");
                let c1 = to_mm(c1.x, c1.y);
                let c2 = to_mm(c2.x, c2.y);
                let p = to_mm(p.x, p.y);
                for i in 1..=BEZIER_STEPS {
                    let t = i as f64 / BEZIER_STEPS as f64;
                    let u = 1.0 - t;
                    current.push(Point::new(
                        u.powi(3) * start.x
                            + 3.0 * u * u * t * c1.x
                            + 3.0 * u * t * t * c2.x
                            + t.powi(3) * p.x,
                        u.powi(3) * start.y
                            + 3.0 * u * u * t * c1.y
                            + 3.0 * u * t * t * c2.y
                            + t.powi(3) * p.y,
                    ));
                }
            }
            PathSegment::Close => finish_ring(&mut current, rings),
        }
    }
    // Treat an unclosed trailing subpath as closed if it has area — many
    // CAD exports omit the explicit Z.
    finish_ring(&mut current, rings);
}

fn finish_ring(current: &mut Vec<Point>, rings: &mut Vec<Vec<Point>>) {
    if current.len() >= 3 && signed_area(current).abs() > 1e-9 {
        rings.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_outline_parses_to_mm() {
        let mut w = Vec::new();
        let poly = parse_svg_outline(crate::tests::RECT_SVG, &mut w).unwrap();
        assert!(
            (poly.area_mm2() - 2000.0).abs() < 1.0,
            "{}",
            poly.area_mm2()
        );
        let (min, max) = poly.bbox();
        assert!((max.x - min.x - 100.0).abs() < 0.1);
        assert!((max.y - min.y - 20.0).abs() < 0.1);
    }

    #[test]
    fn scanline_hits_rect() {
        let poly = Polygon {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, 5.0),
                Point::new(0.0, 5.0),
            ],
        };
        let hits = poly.scanline_hits(2.5);
        assert_eq!(hits.len(), 2);
        assert!((hits[0] - 0.0).abs() < 1e-12 && (hits[1] - 10.0).abs() < 1e-12);
    }

    #[test]
    fn unitless_svg_warns() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20"><path d="M 0 0 L 100 0 L 100 20 L 0 20 Z"/></svg>"##;
        let mut w = Vec::new();
        parse_svg_outline(svg, &mut w).unwrap();
        assert!(w.iter().any(|s| s.contains("96 dpi")));
    }
}
