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
    /// No reservation at all: the pattern may use the whole polygon. Used by
    /// multi-region routing, where tab keepouts are cut out of the region
    /// beforehand rather than reserved during the fill.
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

/// Where a pattern should leave its two path ends relative to each other.
///
/// A lone heater wants both ends together, next to the pad pocket. A region
/// in the middle of a series chain wants them at opposite ends, so the run
/// coming in and the run going out do not have to cross the fill to reach
/// their neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Terminals {
    /// Both ends on the same side.
    #[default]
    SameSide,
    /// Ends on opposite sides. Only the plain serpentine can honour this;
    /// other patterns ignore it, and the caller warns if that costs a
    /// crossing.
    OppositeSides,
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
    pub terminals: Terminals,
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
        terminals,
    } = spec;
    match kind {
        // Row count parity decides which side the last row ends on: an even
        // number of rows returns to the starting side, an odd number crosses.
        FillKind::Serpentine => serpentine::fill(
            outline,
            pitch_mm,
            inset_mm,
            reserve,
            style,
            match terminals {
                Terminals::SameSide => serpentine::RowParity::Even,
                Terminals::OppositeSides => serpentine::RowParity::Odd,
            },
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
