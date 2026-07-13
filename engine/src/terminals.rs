//! Terminal layout: a reserved zone at the left edge holding two rectangular
//! solder pads, stacked symmetrically about the board's horizontal
//! centerline, with feed runs connecting them to the serpentine's ends.
//!
//! ```text
//!   ┌─────────────────────────────┐
//!   │ │   ┌──────── row 0 ──────► │
//!   │ │   └──◄─────────────────┐  │
//!   │ │ ┌────┐ ...             │  │
//!   │ └─►pad A│                ┆  │
//!   │   ├────┤ │ feed B        ┆  │
//!   │ ┌─►pad B◄┘               │  │
//!   │ │   ┌──────── row n-1 ◄──┘  │
//!   └─────────────────────────────┘
//! ```
//! Feed A runs up the left lane to row 0; feed B runs up the right lane
//! (between the pads and the fill) from the last row. Both lanes stay clear
//! of each other because pad A connects at the top half and pad B at the
//! bottom half.

use crate::{EngineError, PathSeg, Point};

/// A rectangular solder pad, center + size, board mm.
#[derive(Debug, Clone, Copy)]
pub struct PadRect {
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
}

pub struct TerminalPlan {
    /// Width of the reserved strip at the left of the fill area.
    pub zone_width_mm: f64,
    /// [pad A (upper), pad B (lower)], symmetric about the centerline.
    pub pads: [PadRect; 2],
    /// x of the left feed lane (pad A) and right feed lane (pad B).
    x_a: f64,
    x_b: f64,
}

pub fn layout(
    bbox: (Point, Point),
    inset_mm: f64,
    trace_width_mm: f64,
    gap_mm: f64,
    pad_size_mm: f64,
    warnings: &mut Vec<String>,
) -> Result<TerminalPlan, EngineError> {
    let (min, max) = bbox;

    let mut s = pad_size_mm;
    let floor = (trace_width_mm * 2.0).max(1.0);
    if s < floor {
        warnings.push(format!(
            "solder pad size raised from {s:.2} mm to {floor:.2} mm so the \
             pad stays solderable"
        ));
        s = floor;
    }
    let pad_w = 1.6 * s;
    let pad_h = s;
    let pad_gap = (2.0 * gap_mm).max(0.8);

    let lane = (2.0 * (trace_width_mm + gap_mm)).max(1.2);
    let zone_w = 2.0 * lane + pad_w;

    let usable_w = max.x - min.x - 2.0 * inset_mm;
    let usable_h = max.y - min.y - 2.0 * inset_mm;
    if zone_w > usable_w / 2.0 {
        return Err(EngineError::OutlineTooSmall(format!(
            "terminal zone ({zone_w:.1} mm) doesn't leave room to route; \
             outline is only {usable_w:.1} mm wide inside margins"
        )));
    }
    if 2.0 * pad_h + pad_gap > usable_h {
        return Err(EngineError::OutlineTooSmall(format!(
            "two {pad_h:.1} mm pads don't fit the {usable_h:.1} mm outline \
             height; shrink the solder pad size"
        )));
    }

    let x0 = min.x + inset_mm;
    let cy = (min.y + max.y) / 2.0;
    let pcx = x0 + lane + pad_w / 2.0;

    Ok(TerminalPlan {
        zone_width_mm: zone_w,
        pads: [
            PadRect {
                cx: pcx,
                cy: cy - (pad_gap + pad_h) / 2.0,
                w: pad_w,
                h: pad_h,
            },
            PadRect {
                cx: pcx,
                cy: cy + (pad_gap + pad_h) / 2.0,
                w: pad_w,
                h: pad_h,
            },
        ],
        x_a: x0 + lane / 2.0,
        x_b: x0 + lane + pad_w + lane / 2.0,
    })
}

impl TerminalPlan {
    /// Path from pad A's center up the left lane to the serpentine's start.
    pub fn feed_start(&self, row_start: Point) -> Vec<PathSeg> {
        let a = self.pads[0];
        let corner1 = Point::new(self.x_a, a.cy);
        let corner2 = Point::new(self.x_a, row_start.y);
        vec![
            PathSeg::Line {
                a: Point::new(a.cx, a.cy),
                b: corner1,
            },
            PathSeg::Line {
                a: corner1,
                b: corner2,
            },
            PathSeg::Line {
                a: corner2,
                b: row_start,
            },
        ]
    }

    /// Path from the serpentine's end up the right lane into pad B.
    pub fn feed_end(&self, row_end: Point) -> Vec<PathSeg> {
        let b = self.pads[1];
        let corner1 = Point::new(self.x_b, row_end.y);
        let corner2 = Point::new(self.x_b, b.cy);
        vec![
            PathSeg::Line {
                a: row_end,
                b: corner1,
            },
            PathSeg::Line {
                a: corner1,
                b: corner2,
            },
            PathSeg::Line {
                a: corner2,
                b: Point::new(b.cx, b.cy),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> TerminalPlan {
        layout(
            (Point::new(0.0, 0.0), Point::new(100.0, 20.0)),
            0.6,
            0.3,
            0.15,
            2.5,
            &mut Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn pads_are_symmetric_about_centerline() {
        let p = plan();
        let cy = 10.0;
        assert!((cy - p.pads[0].cy - (p.pads[1].cy - cy)).abs() < 1e-9);
        assert_eq!(p.pads[0].cx, p.pads[1].cx);
        assert_eq!(p.pads[0].w, p.pads[1].w);
        // Adjacent: the gap between them is small relative to pad height.
        let gap = (p.pads[1].cy - p.pads[1].h / 2.0) - (p.pads[0].cy + p.pads[0].h / 2.0);
        assert!(gap > 0.0 && gap < p.pads[0].h, "gap {gap}");
    }

    #[test]
    fn feeds_connect_pads_to_rows_continuously() {
        let p = plan();
        let row_start = Point::new(p.zone_width_mm + 0.6, 1.0);
        let row_end = Point::new(p.zone_width_mm + 0.6, 19.0);
        let fs = p.feed_start(row_start);
        assert!(
            fs.first()
                .unwrap()
                .start()
                .dist(&Point::new(p.pads[0].cx, p.pads[0].cy))
                < 1e-9
        );
        assert!(fs.last().unwrap().end().dist(&row_start) < 1e-9);
        let fe = p.feed_end(row_end);
        assert!(fe.first().unwrap().start().dist(&row_end) < 1e-9);
        assert!(
            fe.last()
                .unwrap()
                .end()
                .dist(&Point::new(p.pads[1].cx, p.pads[1].cy))
                < 1e-9
        );
        // Lanes are inside the zone, left of the fill area.
        for seg in fs.iter().chain(fe.iter()) {
            for pt in [seg.start(), seg.end()] {
                assert!(pt.x <= p.zone_width_mm + 0.6 + 1e-9);
            }
        }
    }

    #[test]
    fn tiny_pad_size_gets_floored() {
        let mut w = Vec::new();
        let p = layout(
            (Point::new(0.0, 0.0), Point::new(100.0, 20.0)),
            0.6,
            0.3,
            0.15,
            0.1,
            &mut w,
        )
        .unwrap();
        assert!(p.pads[0].h >= 1.0);
        assert_eq!(w.len(), 1);
    }
}
