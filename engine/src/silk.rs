//! Silkscreen legend: electrical + stackup notes drawn as stroked text.
//!
//! Gerber has no native text, so glyphs are simple polyline strokes on a
//! 1×2 grid cell, scaled to fit the outline. The same strokes feed the
//! Gerber legend layer, the KiCad F.SilkS layer, and the SVG preview, so
//! what you see is exactly what gets printed.

use shared::{DesignReport, DesignRequest};

use crate::{outline::Polygon, Point};

/// Stroked silkscreen artwork, in board mm.
pub struct Silk {
    pub strokes: Vec<Vec<Point>>,
    pub stroke_mm: f64,
}

/// Minimum legible/fab-safe silk text height.
const MIN_CHAR_H_MM: f64 = 1.2;
const MAX_CHAR_H_MM: f64 = 4.0;
/// Horizontal advance per character, as a fraction of char height.
const ADVANCE: f64 = 0.7;
/// Line spacing, as a fraction of char height.
const LINE_SPACING: f64 = 1.45;

pub fn generate(
    outline: &Polygon,
    left_reserved_mm: f64,
    req: &DesignRequest,
    report: &DesignReport,
    warnings: &mut Vec<String>,
) -> Silk {
    let lines = vec![
        format!(
            "{:.0}V {:.2}OHM {:.1}W MAX",
            req.voltage, report.achieved_resistance_ohms, report.achieved_watts
        ),
        format!(
            "CU {:.1}OZ {:.0}UM",
            req.copper_oz, report.copper_thickness_um
        ),
    ];

    let (min, max) = outline.bbox();
    // Keep the legend clear of the terminal zone at the left edge — silk
    // over mask-opened pads is a fab violation.
    let left = min.x + left_reserved_mm;
    let avail_w = max.x - left - 2.0;
    let avail_h = max.y - min.y - 2.0;
    let longest = lines.iter().map(|l| l.len()).max().unwrap() as f64;

    // Fit both lines inside the outline with margin, clamped to sane sizes.
    let char_h = (avail_w / (longest * ADVANCE))
        .min(avail_h / (lines.len() as f64 * LINE_SPACING))
        .min(MAX_CHAR_H_MM);

    if char_h < MIN_CHAR_H_MM {
        warnings.push(format!(
            "outline too small for a legible silkscreen legend \
             ({char_h:.2} mm text); silk layer left empty"
        ));
        return Silk {
            strokes: Vec::new(),
            stroke_mm: 0.15,
        };
    }

    let stroke_mm = (0.12 * char_h).max(0.15);
    let cx = (left + max.x) / 2.0;
    let block_h = lines.len() as f64 * char_h * LINE_SPACING - char_h * (LINE_SPACING - 1.0);
    let mut y = (min.y + max.y) / 2.0 - block_h / 2.0;

    let mut strokes = Vec::new();
    for line in &lines {
        let line_w = line.len() as f64 * char_h * ADVANCE;
        let mut x = cx - line_w / 2.0;
        for ch in line.chars() {
            for stroke in glyph(ch) {
                strokes.push(
                    stroke
                        .iter()
                        .map(|&(gx, gy)| Point::new(x + gx * char_h / 2.0, y + gy * char_h / 2.0))
                        .collect(),
                );
            }
            x += char_h * ADVANCE;
        }
        y += char_h * LINE_SPACING;
    }

    Silk { strokes, stroke_mm }
}

/// Glyph strokes on a 1-wide × 2-tall grid, y-down. Unknown characters
/// render as nothing (space).
fn glyph(ch: char) -> Vec<Vec<(f64, f64)>> {
    match ch {
        '0' | 'O' => vec![vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 2.0),
            (0.0, 2.0),
            (0.0, 0.0),
        ]],
        '1' => vec![vec![(0.5, 0.0), (0.5, 2.0)]],
        '2' => vec![vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
            (0.0, 2.0),
            (1.0, 2.0),
        ]],
        '3' => vec![
            vec![(0.0, 0.0), (1.0, 0.0), (1.0, 2.0), (0.0, 2.0)],
            vec![(0.3, 1.0), (1.0, 1.0)],
        ],
        '4' => vec![
            vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)],
            vec![(1.0, 0.0), (1.0, 2.0)],
        ],
        '5' => vec![vec![
            (1.0, 0.0),
            (0.0, 0.0),
            (0.0, 1.0),
            (1.0, 1.0),
            (1.0, 2.0),
            (0.0, 2.0),
        ]],
        '6' => vec![vec![
            (1.0, 0.0),
            (0.0, 0.0),
            (0.0, 2.0),
            (1.0, 2.0),
            (1.0, 1.0),
            (0.0, 1.0),
        ]],
        '7' => vec![vec![(0.0, 0.0), (1.0, 0.0), (0.5, 2.0)]],
        '8' => vec![
            vec![(0.0, 0.0), (1.0, 0.0), (1.0, 2.0), (0.0, 2.0), (0.0, 0.0)],
            vec![(0.0, 1.0), (1.0, 1.0)],
        ],
        '9' => vec![vec![
            (1.0, 1.0),
            (0.0, 1.0),
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 2.0),
            (0.0, 2.0),
        ]],
        'A' => vec![
            vec![(0.0, 2.0), (0.0, 0.0), (1.0, 0.0), (1.0, 2.0)],
            vec![(0.0, 1.0), (1.0, 1.0)],
        ],
        'C' => vec![vec![(1.0, 0.0), (0.0, 0.0), (0.0, 2.0), (1.0, 2.0)]],
        'H' => vec![
            vec![(0.0, 0.0), (0.0, 2.0)],
            vec![(1.0, 0.0), (1.0, 2.0)],
            vec![(0.0, 1.0), (1.0, 1.0)],
        ],
        'M' => vec![vec![
            (0.0, 2.0),
            (0.0, 0.0),
            (0.5, 1.0),
            (1.0, 0.0),
            (1.0, 2.0),
        ]],
        'U' => vec![vec![(0.0, 0.0), (0.0, 2.0), (1.0, 2.0), (1.0, 0.0)]],
        'V' => vec![vec![(0.0, 0.0), (0.5, 2.0), (1.0, 0.0)]],
        'W' => vec![vec![
            (0.0, 0.0),
            (0.25, 2.0),
            (0.5, 1.0),
            (0.75, 2.0),
            (1.0, 0.0),
        ]],
        'X' => vec![vec![(0.0, 0.0), (1.0, 2.0)], vec![(1.0, 0.0), (0.0, 2.0)]],
        'Z' => vec![vec![(0.0, 0.0), (1.0, 0.0), (0.0, 2.0), (1.0, 2.0)]],
        '.' => vec![vec![(0.45, 1.8), (0.55, 2.0)]],
        '-' => vec![vec![(0.2, 1.0), (0.8, 1.0)]],
        '/' => vec![vec![(0.0, 2.0), (1.0, 0.0)]],
        _ => Vec::new(), // space and anything unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: f64, h: f64) -> Polygon {
        Polygon {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(w, 0.0),
                Point::new(w, h),
                Point::new(0.0, h),
            ],
        }
    }

    fn dummy_report() -> DesignReport {
        DesignReport {
            target_resistance_ohms: 14.4,
            achieved_resistance_ohms: 14.4,
            achieved_watts: 10.0,
            operating_current_amps: 0.83,
            current_headroom_frac: 0.42,
            trace_width_mm: 0.29,
            trace_gap_mm: 0.17,
            trace_length_mm: 4000.0,
            outline_area_cm2: 20.0,
            power_density_w_cm2: 0.5,
            copper_thickness_um: 17.4,
            warnings: vec![],
        }
    }

    #[test]
    fn legend_fits_inside_outline() {
        let outline = rect(100.0, 20.0);
        let mut w = Vec::new();
        let silk = generate(
            &outline,
            0.0,
            &DesignRequest::default(),
            &dummy_report(),
            &mut w,
        );
        assert!(w.is_empty(), "{w:?}");
        assert!(!silk.strokes.is_empty());
        for stroke in &silk.strokes {
            for p in stroke {
                assert!(p.x >= 0.0 && p.x <= 100.0, "x={} outside", p.x);
                assert!(p.y >= 0.0 && p.y <= 20.0, "y={} outside", p.y);
            }
        }
    }

    #[test]
    fn tiny_outline_skips_silk_with_warning() {
        let outline = rect(10.0, 5.0);
        let mut w = Vec::new();
        let silk = generate(
            &outline,
            0.0,
            &DesignRequest::default(),
            &dummy_report(),
            &mut w,
        );
        assert!(silk.strokes.is_empty());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn all_legend_characters_have_glyphs() {
        // Every char the legend can emit must render as strokes.
        for ch in "0123456789.VWAOHMCUZX- /".chars() {
            if ch == ' ' {
                continue;
            }
            assert!(!glyph(ch).is_empty(), "no glyph for {ch:?}");
        }
    }
}
