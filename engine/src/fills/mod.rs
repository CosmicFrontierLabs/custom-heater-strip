//! Fill patterns: each routes one continuous non-crossing trace at uniform
//! pitch through the outline, starting and ending at the terminal zone's
//! edge (start above the pads, end below the start). Catalog and research
//! notes live in docs/fill-patterns.md.

pub mod concentric;
pub mod gilbert;
mod offset;
pub mod serpentine;
pub mod spiral;

use shared::{CornerStyle, FillKind};

use crate::{outline::Polygon, EngineError, PathSeg};

/// Route the heater trace with the requested pattern.
///
/// `pitch_mm` is the centerline-to-centerline spacing, `inset_mm` the
/// clearance from the outline (edge margin + half trace width), and
/// `left_reserved_mm` the terminal zone width kept free at the left edge.
pub fn fill(
    kind: FillKind,
    outline: &Polygon,
    pitch_mm: f64,
    inset_mm: f64,
    left_reserved_mm: f64,
    style: CornerStyle,
    warnings: &mut Vec<String>,
) -> Result<Vec<PathSeg>, EngineError> {
    match kind {
        FillKind::Serpentine => serpentine::fill(
            outline,
            pitch_mm,
            inset_mm,
            left_reserved_mm,
            style,
            serpentine::RowParity::Even,
            warnings,
        ),
        FillKind::WavySerpentine => serpentine::fill_wavy(
            outline,
            pitch_mm,
            inset_mm,
            left_reserved_mm,
            style,
            warnings,
        ),
        FillKind::Counterflow => serpentine::fill_counterflow(
            outline,
            pitch_mm,
            inset_mm,
            left_reserved_mm,
            style,
            warnings,
        ),
        FillKind::Hilbert => gilbert::fill(outline, pitch_mm, inset_mm, left_reserved_mm, warnings),
        FillKind::DoubleSpiral => {
            spiral::fill(outline, pitch_mm, inset_mm, left_reserved_mm, warnings)
        }
        FillKind::Concentric => {
            concentric::fill(outline, pitch_mm, inset_mm, left_reserved_mm, warnings)
        }
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
