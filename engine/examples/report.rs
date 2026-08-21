//! Render a visual audit of the merged-region router to a self-contained HTML
//! page: every selection shape against every fill pattern, with copper faults
//! marked where they occur.
//!
//! ```sh
//! cargo run -p engine --example report > /tmp/report.html
//! ```
//!
//! Faults are drawn, not just counted, because a number tells you a design is
//! wrong and a marker tells you where to look.

use shared::{DesignRequest, FillKind, GeometrySpec};
use std::fmt::Write as _;

/// A closed ring, as the geometry API takes them.
type Ring = Vec<[f64; 2]>;
/// A named selection: title, one-line explanation, and its polygons.
type Case = (&'static str, &'static str, Vec<Ring>);

fn ring(x0: f64, y0: f64, x1: f64, y1: f64) -> Ring {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// Selections that are all legal under the contiguity rule: every polygon
/// touches another, and both tabs sit inside.
fn cases() -> Vec<Case> {
    let row = |n: usize| -> Vec<Ring> {
        (0..n)
            .map(|i| {
                let x = 10.0 + i as f64 * 40.0;
                ring(x, 5.0, x + 40.0, 35.0)
            })
            .collect()
    };
    vec![
        (
            "One panel",
            "a single rectangle, both tabs inside it",
            row(1),
        ),
        (
            "Three abutting panels",
            "three rectangles sharing edges — the case that used to need links between regions",
            row(3),
        ),
        (
            "L shape",
            "two rectangles meeting at a corner; the union is concave",
            vec![ring(10.0, 5.0, 130.0, 35.0), ring(10.0, 35.0, 50.0, 95.0)],
        ),
        (
            "Staircase",
            "three overlapping panels stepping diagonally",
            vec![
                ring(10.0, 5.0, 60.0, 35.0),
                ring(50.0, 25.0, 100.0, 55.0),
                ring(90.0, 45.0, 140.0, 75.0),
            ],
        ),
        (
            "U shape",
            "two arms and a base — rows across the arms cross the shape twice, rows along them do not, so the orientation search has to find the second one",
            vec![
                ring(10.0, 5.0, 40.0, 85.0),
                ring(90.0, 5.0, 120.0, 85.0),
                ring(10.0, 85.0, 120.0, 115.0),
            ],
        ),
        (
            "H shape",
            "multi-span whichever way you sweep it by eye — but not once the corridor is taken into account",
            vec![
                ring(10.0, 5.0, 40.0, 115.0),
                ring(90.0, 5.0, 120.0, 115.0),
                ring(40.0, 50.0, 90.0, 70.0),
            ],
        ),
        (
            "Plus",
            "a centre panel with arms on all four sides — rows here cross the shape more than once",
            vec![ring(45.0, 5.0, 85.0, 105.0), ring(5.0, 45.0, 125.0, 65.0)],
        ),
    ]
}

/// Place both tabs inside the selection's first polygon, near its middle.
/// Deriving them per case rather than hard-coding coordinates keeps every
/// selection legal under the tabs-inside rule regardless of where it sits.
fn tabs_for(heaters: &[Ring]) -> (Ring, Ring) {
    let first = &heaters[0];
    let xs: Vec<f64> = first.iter().map(|p| p[0]).collect();
    let ys: Vec<f64> = first.iter().map(|p| p[1]).collect();
    let cx = (xs.iter().cloned().fold(f64::MAX, f64::min)
        + xs.iter().cloned().fold(f64::MIN, f64::max))
        / 2.0;
    let cy = (ys.iter().cloned().fold(f64::MAX, f64::min)
        + ys.iter().cloned().fold(f64::MIN, f64::max))
        / 2.0;
    (
        ring(cx - 3.0, cy - 5.0, cx + 3.0, cy - 1.0),
        ring(cx - 3.0, cy + 1.0, cx + 3.0, cy + 5.0),
    )
}

fn request(heaters: Vec<Ring>, kind: FillKind) -> DesignRequest {
    let (tab_in, tab_out) = tabs_for(&heaters);
    DesignRequest {
        geometry: Some(GeometrySpec {
            outline: None,
            heaters,
            tab_in: Some(tab_in),
            tab_out: Some(tab_out),
        }),
        voltage: 24.0,
        watts: 30.0,
        max_current: 3.0,
        copper_oz: 0.5,
        min_trace_mm: 0.15,
        min_gap_mm: 0.15,
        edge_margin_mm: 0.5,
        fill_kind: kind,
        ..DesignRequest::default()
    }
}

/// Overlay markers on a preview at each fault location.
fn annotate(svg: &str, shorts: &[engine::Point], escapes: &[engine::Point], scale: f64) -> String {
    let r = (scale * 0.012).max(1.2);
    let mut marks = String::new();
    for p in shorts {
        let _ = write!(
            marks,
            r##"<circle cx="{:.3}" cy="{:.3}" r="{r:.3}" fill="none" stroke="#f7768e" stroke-width="{:.3}"/>"##,
            p.x,
            p.y,
            r * 0.35
        );
    }
    for p in escapes {
        let _ = write!(
            marks,
            r##"<rect x="{:.3}" y="{:.3}" width="{:.3}" height="{:.3}" fill="none" stroke="#e0af68" stroke-width="{:.3}"/>"##,
            p.x - r,
            p.y - r,
            r * 2.0,
            r * 2.0,
            r * 0.35
        );
    }
    svg.replace("</svg>", &format!("{marks}</svg>"))
}

fn main() {
    let mut body = String::new();
    let mut totals = (0usize, 0usize, 0usize);

    for (title, blurb, heaters) in cases() {
        let _ = write!(
            body,
            "<h2>{title}</h2><p class=\"blurb\">{blurb}</p><div class=\"grid\">"
        );
        for kind in FillKind::ALL {
            let req = request(heaters.clone(), kind);
            totals.0 += 1;
            let (card, ok) = match engine::design(&req) {
                Err(e) => (
                    format!(
                        "<div class=\"card skip\"><h3>{}</h3><p class=\"msg\">refused: {}</p></div>",
                        kind.label(),
                        html(&e.to_string())
                    ),
                    None,
                ),
                Ok(d) => {
                    let all: Vec<usize> = (0..d.trace.len()).collect();
                    let short_pairs = engine::geom::find_shorts(&d.trace, &all);
                    let mut short_pts = Vec::new();
                    for (i, j) in &short_pairs {
                        short_pts
                            .extend(engine::geom::intersections(&d.trace[*i], &d.trace[*j]));
                    }
                    let escapes = engine::geom::find_escapes(
                        &d.trace,
                        &d.outline,
                        d.trace_width_mm,
                        0.0,
                    );
                    let esc_pts: Vec<engine::Point> = escapes.iter().map(|e| e.at).collect();
                    let (lo, hi) = d.outline.bbox();
                    let scale = (hi.x - lo.x).max(hi.y - lo.y);

                    let resp = engine::generate(&req).expect("already designed");
                    let svg = annotate(&resp.preview_svg, &short_pts, &esc_pts, scale);
                    let r = &d.report;
                    let verdict = if short_pairs.is_empty() && escapes.is_empty() {
                        "<span class=\"ok\">clean</span>".to_string()
                    } else {
                        format!(
                            "<span class=\"bad\">{} short(s), {} off-board</span>",
                            short_pairs.len(),
                            escapes.len()
                        )
                    };
                    if short_pairs.is_empty() && escapes.is_empty() {
                        totals.1 += 1;
                    }
                    (
                        format!(
                            "<div class=\"card\"><h3>{}</h3><div class=\"prev\">{svg}</div>\
                             <table><tr><td>faults</td><td>{verdict}</td></tr>\
                             <tr><td>resistance</td><td>{:.2} Ω / {:.2} Ω target</td></tr>\
                             <tr><td>trace</td><td>{:.2} m at {:.3} mm</td></tr>\
                             <tr><td>feed share</td><td>{:.1} %</td></tr>\
                             <tr><td>area</td><td>{:.1} cm² at {:.2} W/cm²</td></tr></table>\
                             {}</div>",
                            kind.label(),
                            r.achieved_resistance_ohms,
                            r.target_resistance_ohms,
                            r.trace_length_mm / 1000.0,
                            r.trace_width_mm,
                            100.0 * r.link_length_mm / r.trace_length_mm,
                            r.outline_area_cm2,
                            r.power_density_w_cm2,
                            warn_list(&r.warnings)
                        ),
                        Some(()),
                    )
                }
            };
            if ok.is_none() {
                totals.2 += 1;
            }
            body.push_str(&card);
        }
        body.push_str("</div>");
    }

    let (n, clean, refused) = totals;
    print!(
        r##"<!DOCTYPE html><html><head><meta charset="utf-8">
<title>Heater router audit</title><style>
:root {{ color-scheme: dark; }}
body {{ font: 14px/1.5 system-ui, sans-serif; background:#1a1b26; color:#c0caf5;
        margin:0; padding:2rem 2.5rem 4rem; }}
h1 {{ color:#7aa2f7; margin-bottom:.2rem; }}
h2 {{ color:#e0af68; margin-top:2.5rem; border-bottom:1px solid #2a2f45; padding-bottom:.3rem; }}
h3 {{ margin:0 0 .5rem; font-size:.95rem; color:#7dcfff; }}
.lede, .blurb {{ color:#9aa5ce; max-width:70ch; }}
.blurb {{ margin-top:-.2rem; font-size:.9rem; }}
.grid {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(330px,1fr)); gap:1.1rem; margin-top:1rem; }}
.card {{ background:#16213e; border:1px solid #2a2f45; border-radius:8px; padding:.9rem; }}
.card.skip {{ opacity:.75; }}
.prev {{ background:#0f1830; border-radius:6px; padding:.4rem; }}
.prev svg {{ width:100%; height:auto; display:block; }}
table {{ width:100%; border-collapse:collapse; margin-top:.6rem; font-size:.85rem; }}
td {{ padding:.15rem 0; vertical-align:top; }}
td:first-child {{ color:#7f8bb0; width:8.5em; }}
.ok {{ color:#9ece6a; font-weight:600; }}
.bad {{ color:#f7768e; font-weight:600; }}
.msg {{ color:#f7768e; font-size:.85rem; }}
.warns {{ margin:.5rem 0 0; padding-left:1.1rem; color:#e0af68; font-size:.8rem; }}
.legend {{ display:flex; gap:1.4rem; margin-top:1rem; font-size:.85rem; color:#9aa5ce; }}
.swatch {{ display:inline-block; width:.8rem; height:.8rem; border-radius:50%; margin-right:.35rem;
           border:2px solid #f7768e; vertical-align:-1px; }}
.swatch.sq {{ border-radius:2px; border-color:#e0af68; }}
</style></head><body>
<h1>Heater router audit</h1>
<p class="lede">Every legal selection shape against every fill pattern, after
merging contiguous polygons into one region. <strong>{clean} of {n}</strong>
routed designs are clean; {refused} were refused outright rather than emitted.
Faults are marked on the artwork.</p>
<div class="legend">
  <span><span class="swatch"></span>copper touching copper</span>
  <span><span class="swatch sq"></span>copper off the board</span>
</div>
{body}</body></html>"##
    );
}

fn warn_list(warnings: &[String]) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    let items: String = warnings
        .iter()
        .map(|w| format!("<li>{}</li>", html(w)))
        .collect();
    format!("<ul class=\"warns\">{items}</ul>")
}

fn html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
