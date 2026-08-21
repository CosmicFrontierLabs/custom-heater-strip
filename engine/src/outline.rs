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

    /// Area centroid (shoelace). Falls back to the vertex mean for degenerate
    /// rings, so it is always inside a convex tab.
    pub fn centroid(&self) -> Point {
        let pts = &self.points;
        let n = pts.len();
        let a = signed_area(pts);
        if a.abs() < 1e-12 {
            let (sx, sy) = pts
                .iter()
                .fold((0.0, 0.0), |(sx, sy), p| (sx + p.x, sy + p.y));
            return Point::new(sx / n as f64, sy / n as f64);
        }
        let (mut cx, mut cy) = (0.0, 0.0);
        for i in 0..n {
            let (p, q) = (pts[i], pts[(i + 1) % n]);
            let cross = p.x * q.y - q.x * p.y;
            cx += (p.x + q.x) * cross;
            cy += (p.y + q.y) * cross;
        }
        Point::new(cx / (6.0 * a), cy / (6.0 * a))
    }

    /// Even-odd containment test (ray cast to +x).
    pub fn contains(&self, p: Point) -> bool {
        let hits = self.scanline_hits(p.y);
        hits.iter().filter(|x| **x > p.x).count() % 2 == 1
    }

    /// Does any part of `other`'s ring fall inside this polygon?
    pub fn overlaps(&self, other: &Polygon) -> bool {
        other.points.iter().any(|p| self.contains(*p))
            || self.points.iter().any(|p| other.contains(*p))
    }

    /// Is every point of `other` inside this polygon?
    pub fn contains_polygon(&self, other: &Polygon) -> bool {
        other.points.iter().all(|p| self.contains(*p))
    }

    /// Distance from `p` to the nearest point on this ring, ignoring which
    /// side of it `p` is on.
    pub fn distance_to_boundary(&self, p: Point) -> f64 {
        let n = self.points.len();
        (0..n)
            .map(|i| point_segment_distance(p, self.points[i], self.points[(i + 1) % n]))
            .fold(f64::INFINITY, f64::min)
    }

    /// Signed clearance from `p` to the boundary: positive inside, negative
    /// outside. This is what tells you whether copper of a given width fits.
    pub fn clearance(&self, p: Point) -> f64 {
        let d = self.distance_to_boundary(p);
        if self.contains(p) {
            d
        } else {
            -d
        }
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

/// Result of cutting one polygon out of another.
pub struct Subtraction {
    /// The largest remaining piece.
    pub largest: Polygon,
    /// How many disjoint pieces the cut produced (>1 means the subtraction
    /// split the region and coverage was dropped).
    pub pieces: usize,
}

/// Convert to a cavalier_contours polyline, wound counter-clockwise as its
/// boolean ops expect.
fn to_pline(poly: &Polygon) -> cavalier_contours::polyline::Polyline {
    use cavalier_contours::polyline::{PlineSource, PlineSourceMut, Polyline};
    let mut pl = Polyline::new_closed();
    for p in &poly.points {
        pl.add(p.x, p.y, 0.0);
    }
    if pl.area() < 0.0 {
        pl.invert_direction_mut();
    }
    pl
}

fn from_pline(pl: &cavalier_contours::polyline::Polyline) -> Polygon {
    use cavalier_contours::polyline::PlineSource;
    let flat = pl.arcs_to_approx_lines(0.01).unwrap_or_else(|| pl.clone());
    Polygon {
        points: (0..flat.vertex_count())
            .map(|i| {
                let v = flat.at(i);
                Point::new(v.x, v.y)
            })
            .collect(),
    }
}

/// The union of a set of polygons, largest piece first.
///
/// Built on [`crate::arrangement`] rather than a boolean `Or`, because the
/// case that matters most here is the one boolean ops get wrong: two polygons
/// sharing an edge exactly have a zero-area intersection, so `Or` reports them
/// as disjoint. Since "contiguous" for a heater selection means precisely
/// "abutting", that is the common case, not a corner one.
///
/// Feeding every edge to the arrangement sidesteps it entirely. Shared walls
/// appear twice, collapse to one edge, and end up interior to the traced
/// boundary — so abutting, overlapping and crossing inputs all merge by the
/// same mechanism.
///
/// Returning every piece rather than merging blindly is the point: a caller
/// that requires its inputs to touch can check for exactly one piece, which
/// makes "are these contiguous?" the same question as "did the union stay
/// whole?" — no separate adjacency test, no second tolerance to tune.
///
/// Holes are not preserved; the result is each piece's silhouette. That
/// matches the fills, which route one simply-connected region.
pub fn union_all(polys: &[Polygon]) -> Vec<Polygon> {
    let mut segments = Vec::new();
    for poly in polys {
        let n = poly.points.len();
        for i in 0..n {
            segments.push((poly.points[i], poly.points[(i + 1) % n]));
        }
    }
    let mut out: Vec<Polygon> = crate::arrangement::compose(&segments, &mut Vec::new())
        .into_iter()
        .map(|piece| Polygon { points: piece.ring })
        .filter(|p| p.points.len() >= 3)
        .collect();
    out.sort_by(|a, b| b.area_mm2().partial_cmp(&a.area_mm2()).unwrap());
    out
}

impl Polygon {
    /// Subtract `hole` from this polygon. Returns `None` when nothing is left.
    ///
    /// Only the largest surviving piece is kept: the fill patterns route a
    /// single continuous path through one region, so a cut that fragments the
    /// region necessarily loses the smaller fragments. `pieces` lets the
    /// caller warn about that.
    pub fn subtract(&self, hole: &Polygon) -> Option<Subtraction> {
        use cavalier_contours::polyline::{BooleanOp, PlineSource};

        let result = to_pline(self).boolean(&to_pline(hole), BooleanOp::Not);
        let pieces = result.pos_plines.len();
        let largest = result
            .pos_plines
            .into_iter()
            .map(|r| r.pline)
            .max_by(|a, b| a.area().abs().partial_cmp(&b.area().abs()).unwrap())?;
        let poly = from_pline(&largest);
        if poly.points.len() < 3 {
            return None;
        }
        Some(Subtraction {
            largest: poly,
            pieces,
        })
    }
}

fn point_segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-18 {
        return p.dist(&a);
    }
    // Projection parameter, clamped to the segment.
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(&Point::new(a.x + t * dx, a.y + t * dy))
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
mod union_tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon {
            points: vec![
                Point::new(x0, y0),
                Point::new(x1, y0),
                Point::new(x1, y1),
                Point::new(x0, y1),
            ],
        }
    }

    #[test]
    fn abutting_rectangles_merge_into_one() {
        let u = union_all(&[
            rect(0.0, 0.0, 10.0, 10.0),
            rect(10.0, 0.0, 20.0, 10.0),
            rect(20.0, 0.0, 30.0, 10.0),
        ]);
        assert_eq!(u.len(), 1, "abutting rectangles must merge");
        assert!(
            (u[0].area_mm2() - 300.0).abs() < 1e-6,
            "{}",
            u[0].area_mm2()
        );
    }

    #[test]
    fn overlapping_rectangles_merge_without_double_counting() {
        let u = union_all(&[rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 0.0, 15.0, 10.0)]);
        assert_eq!(u.len(), 1);
        assert!(
            (u[0].area_mm2() - 150.0).abs() < 1e-6,
            "{}",
            u[0].area_mm2()
        );
    }

    /// The whole point: separated inputs stay separated, so "is this selection
    /// contiguous?" is answered by counting pieces.
    #[test]
    fn separated_rectangles_stay_separate() {
        let u = union_all(&[rect(0.0, 0.0, 10.0, 10.0), rect(50.0, 0.0, 60.0, 10.0)]);
        assert_eq!(u.len(), 2);
    }

    /// Order must not matter: a middle piece introduced last still glues the
    /// two ends together.
    #[test]
    fn a_bridging_piece_added_last_still_merges_everything() {
        let u = union_all(&[
            rect(0.0, 0.0, 10.0, 10.0),
            rect(20.0, 0.0, 30.0, 10.0),
            rect(10.0, 0.0, 20.0, 10.0),
        ]);
        assert_eq!(u.len(), 1, "the bridge should have joined all three");
        assert!(
            (u[0].area_mm2() - 300.0).abs() < 1e-6,
            "{}",
            u[0].area_mm2()
        );
    }

    #[test]
    fn an_l_shape_keeps_its_area() {
        let u = union_all(&[rect(0.0, 0.0, 30.0, 10.0), rect(0.0, 10.0, 10.0, 30.0)]);
        assert_eq!(u.len(), 1);
        assert!(
            (u[0].area_mm2() - 500.0).abs() < 1e-6,
            "{}",
            u[0].area_mm2()
        );
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
    fn rect_element_with_rounded_corners_parses() {
        // The frontend's "rectangle" outline mode emits a <rect rx=…>;
        // usvg must normalize it to a path we can fill.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="12mm" viewBox="0 0 60 12"><rect width="60" height="12" rx="3"/></svg>"##;
        let mut w = Vec::new();
        let poly = parse_svg_outline(svg, &mut w).unwrap();
        // Full rect is 720 mm²; r=3 corners shave 4r² − πr² ≈ 7.7 mm².
        assert!(
            (poly.area_mm2() - (720.0 - (4.0 - std::f64::consts::PI) * 9.0)).abs() < 1.0,
            "{}",
            poly.area_mm2()
        );
    }

    #[test]
    fn unitless_svg_warns() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20"><path d="M 0 0 L 100 0 L 100 20 L 0 20 Z"/></svg>"##;
        let mut w = Vec::new();
        parse_svg_outline(svg, &mut w).unwrap();
        assert!(w.iter().any(|s| s.contains("96 dpi")));
    }
}
