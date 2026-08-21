//! Fill patterns: each routes one continuous non-crossing trace at uniform
//! pitch through the outline, avoiding the solder-pad pocket and delivering
//! both path ends where the terminal feeds can reach them. Catalog and
//! research notes live in docs/fill-patterns.md.

pub mod concentric;
pub mod gilbert;
mod offset;
pub mod serpentine;
pub mod spiral;

use shared::{CornerStyle, FillKind};

use crate::{outline::Polygon, EngineError, PathSeg};

pub use offset::reverse_path;

/// The area reserved for the terminals. The fill stays dense: only rows
/// crossing the pocket's y-band give way to the pads; everywhere else the
/// pattern may run as far left as `lane_edge` (a thin corridor for the
/// serpentine-family feed lane).
#[derive(Debug, Clone, Copy)]
pub struct Reserve {
    /// Fill never goes left of this outside the pocket band.
    pub lane_edge: f64,
    /// Fill never goes left of this inside the pocket band.
    pub pocket_x1: f64,
    /// Pocket band (pads + clearance), y-down.
    pub pocket_y0: f64,
    pub pocket_y1: f64,
}

impl Reserve {
    /// No reservation at all: the pattern may use the whole polygon. Only the
    /// pattern tests want this; every real design reserves a terminal zone.
    #[cfg(test)]
    pub fn none() -> Self {
        Reserve {
            lane_edge: f64::NEG_INFINITY,
            pocket_x1: f64::NEG_INFINITY,
            pocket_y0: 0.0,
            pocket_y1: 0.0,
        }
    }

    /// Full-height column reservation at `x` (used by Hilbert and tests).
    pub fn column(x: f64) -> Self {
        Reserve {
            lane_edge: x,
            pocket_x1: x,
            pocket_y0: f64::NEG_INFINITY,
            pocket_y1: f64::INFINITY,
        }
    }

    /// The left bound applying to a row at `y`.
    pub fn left_bound(&self, y: f64) -> f64 {
        if y >= self.pocket_y0 && y <= self.pocket_y1 {
            self.pocket_x1
        } else {
            self.lane_edge
        }
    }

    /// Grow every reserved edge outward by `d` (used by the counterflow
    /// base path, whose offsets expand it by half a pitch).
    pub fn expand(&self, d: f64) -> Self {
        Reserve {
            lane_edge: self.lane_edge + d,
            pocket_x1: self.pocket_x1 + d,
            pocket_y0: self.pocket_y0 - d,
            pocket_y1: self.pocket_y1 + d,
        }
    }
}

/// One fill request: which pattern, over what polygon, at what spacing.
pub struct FillSpec<'a> {
    pub kind: FillKind,
    pub outline: &'a Polygon,
    /// Centerline-to-centerline spacing.
    pub pitch_mm: f64,
    /// Clearance from the outline (edge margin + half trace width).
    pub inset_mm: f64,
    pub reserve: Reserve,
    pub style: CornerStyle,
}

/// How much of a region a scanline pattern can actually reach, in `[0, 1]`.
///
/// The scanline fills route one span per row, so a row that crosses the shape
/// more than once loses everything but its widest section. This measures that
/// loss before committing to an orientation.
///
/// It matters more than it sounds. Sweeping a U across its arms gives two
/// spans per row and throws away one arm; sweeping it along them gives one
/// span per row and loses nothing. Same shape, same pattern — the difference
/// is entirely which way the rows run, which is free to choose.
pub fn scanline_coverage(outline: &Polygon, pitch_mm: f64, inset_mm: f64, reserve: Reserve) -> f64 {
    let (min, max) = outline.bbox();
    let (y_lo, y_hi) = (min.y + inset_mm, max.y - inset_mm);
    if y_hi <= y_lo || pitch_mm <= 0.0 {
        return 0.0;
    }
    let (mut widest, mut total) = (0.0f64, 0.0f64);
    let mut y = y_lo;
    while y <= y_hi {
        let bound = reserve.left_bound(y);
        let spans: Vec<f64> = outline
            .scanline_hits(y)
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| ((c[0] + inset_mm).max(bound), c[1] - inset_mm))
            .filter(|(a, b)| b > a)
            .map(|(a, b)| b - a)
            .collect();
        if let Some(best) = spans
            .iter()
            .cloned()
            .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
        {
            widest += best;
            total += spans.iter().sum::<f64>();
        }
        y += pitch_mm;
    }
    if total <= 0.0 {
        0.0
    } else {
        widest / total
    }
}

/// Area of the region the reserve keeps the fill out of, in mm².
///
/// The terminal corridor and the tab pocket are real holes in the heated area:
/// copper never goes there, so that area produces no heat. Measuring it lets
/// the electrical solve size the trace against what can actually be filled
/// rather than against the outline, which otherwise asks for a trace too thin
/// to manufacture and then gets clamped, landing the design off target.
pub fn reserved_area(outline: &Polygon, pitch_mm: f64, inset_mm: f64, reserve: Reserve) -> f64 {
    let (min, max) = outline.bbox();
    let (y_lo, y_hi) = (min.y + inset_mm, max.y - inset_mm);
    if y_hi <= y_lo || pitch_mm <= 0.0 {
        return 0.0;
    }
    let mut blocked = 0.0;
    let mut y = y_lo;
    while y <= y_hi {
        let bound = reserve.left_bound(y);
        for c in outline.scanline_hits(y).chunks(2) {
            let [a, b] = *c else { continue };
            // How much of this span lies left of the bound.
            blocked += (bound.min(b) - a).clamp(0.0, b - a);
        }
        y += pitch_mm;
    }
    blocked * pitch_mm
}

/// Does this pattern route by horizontal scanlines, and therefore care which
/// way the rows run?
pub fn is_scanline(kind: FillKind) -> bool {
    matches!(
        kind,
        FillKind::Serpentine | FillKind::WavySerpentine | FillKind::Counterflow
    )
}

/// Route the heater trace with the requested pattern.
pub fn fill(spec: FillSpec<'_>, warnings: &mut Vec<String>) -> Result<Vec<PathSeg>, EngineError> {
    let FillSpec {
        kind,
        outline,
        pitch_mm,
        inset_mm,
        reserve,
        style,
    } = spec;
    match kind {
        // Even row count brings the path's two ends back to the same side,
        // which is where the terminal corridor is.
        FillKind::Serpentine => serpentine::fill(
            outline,
            pitch_mm,
            inset_mm,
            reserve,
            style,
            serpentine::RowParity::Even,
            warnings,
        ),
        FillKind::WavySerpentine => {
            serpentine::fill_wavy(outline, pitch_mm, inset_mm, reserve, style, warnings)
        }
        FillKind::Counterflow => {
            serpentine::fill_counterflow(outline, pitch_mm, inset_mm, reserve, style, warnings)
        }
        // The gilbert grid can't wrap a pocket; it reserves the full column.
        FillKind::Hilbert => gilbert::fill(
            outline,
            pitch_mm,
            inset_mm,
            Reserve::column(reserve.pocket_x1),
            warnings,
        ),
        FillKind::DoubleSpiral => spiral::fill(outline, pitch_mm, inset_mm, reserve, warnings),
        FillKind::Concentric => concentric::fill(outline, pitch_mm, inset_mm, reserve, warnings),
    }
}

/// Shared sanity checks used by pattern tests: the path is continuous and
/// every segment endpoint stays inside the given bounds.
#[cfg(test)]
pub(crate) fn assert_path_well_formed(
    path: &[PathSeg],
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
) {
    assert!(path.len() >= 2, "path too short: {} segs", path.len());
    let mut prev: Option<crate::Point> = None;
    for seg in path {
        if let Some(p) = prev {
            assert!(
                p.dist(&seg.start()) < 1e-6,
                "gap in path at ({:.4},{:.4}) → ({:.4},{:.4})",
                p.x,
                p.y,
                seg.start().x,
                seg.start().y
            );
        }
        prev = Some(seg.end());
        for pt in [seg.start(), seg.end()] {
            assert!(
                pt.x >= min_x - 1e-6 && pt.x <= max_x + 1e-6,
                "x={} outside [{min_x},{max_x}]",
                pt.x
            );
            assert!(
                pt.y >= min_y - 1e-6 && pt.y <= max_y + 1e-6,
                "y={} outside [{min_y},{max_y}]",
                pt.y
            );
        }
    }
}
