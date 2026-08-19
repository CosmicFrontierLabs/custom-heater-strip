//! Render the README hero: a heater routed into the shape of a letter S.
//!
//! ```sh
//! cargo run -p engine --example hero > docs/hero-s.svg
//! ```
//!
//! The S is a deliberate stress case as well as a nice picture. It is deeply
//! concave, but concave in the one way the scanline fill handles well: every
//! horizontal row crosses exactly one span of the shape — the full-width bars
//! at top, middle and bottom, the left stroke between the first two, the right
//! stroke between the last two. A letter with two spans on one row (an H, a U)
//! would lose coverage and say so in a warning.

use shared::{CornerStyle, DesignRequest, FillKind};

/// Board size and stroke thickness of the letter, in mm.
const W: f64 = 76.0;
const H: f64 = 116.0;
const T: f64 = 24.0;

/// The letter as one closed ring, traced around the outside.
///
/// Regions: three full-width bars (top, middle, bottom), a left stroke joining
/// the top two and a right stroke joining the bottom two. `x = W` runs
/// unbroken from the middle bar to the bottom edge, and `x = 0` from the middle
/// bar to the top, which is what makes this a single ring rather than two.
fn letter_s() -> Vec<(f64, f64)> {
    let my0 = (H - T) / 2.0; // top of the middle bar
    let my1 = my0 + T; // bottom of the middle bar
    vec![
        (0.0, 0.0),
        (W, 0.0),
        (W, T),
        (T, T),
        (T, my0),
        (W, my0),
        (W, H),
        (0.0, H),
        (0.0, H - T),
        (W - T, H - T),
        (W - T, my1),
        (0.0, my1),
    ]
}

fn main() {
    let ring = letter_s();
    let d: String = ring
        .iter()
        .enumerate()
        .map(|(i, (x, y))| format!("{}{x} {y}", if i == 0 { 'M' } else { 'L' }))
        .collect::<Vec<_>>()
        .join("");
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}mm" height="{H}mm" viewBox="0 0 {W} {H}"><path d="{d}Z"/></svg>"##
    );

    let req = DesignRequest {
        svg,
        voltage: 12.0,
        watts: 12.0,
        max_current: 2.0,
        copper_oz: 0.5,
        min_trace_mm: 0.15,
        min_gap_mm: 0.15,
        edge_margin_mm: 0.6,
        corner_style: CornerStyle::Smooth,
        fill_kind: FillKind::Concentric,
        ..DesignRequest::default()
    };

    let resp = engine::generate(&req).expect("the S should be routable");
    let r = &resp.report;
    eprintln!(
        "{:.2} V / {:.1} W → {:.2} Ω (target {:.2} Ω), {:.2} A ({:.0}% of ceiling)",
        req.voltage,
        r.achieved_watts,
        r.achieved_resistance_ohms,
        r.target_resistance_ohms,
        r.operating_current_amps,
        r.current_headroom_frac * 100.0
    );
    eprintln!(
        "{:.3} mm trace × {:.2} m over {:.1} cm², {:.2} W/cm²",
        r.trace_width_mm,
        r.trace_length_mm / 1000.0,
        r.outline_area_cm2,
        r.power_density_w_cm2
    );
    for w in &r.warnings {
        eprintln!("warning: {w}");
    }
    print!("{}", on_dark_backdrop(&resp.preview_svg));
}

/// Drop a dark card behind the preview.
///
/// The silkscreen legend is drawn in near-white, because that is what it looks
/// like on a board — which makes it invisible against GitHub's light theme and
/// the board itself washed out. Painting the app's own background behind it
/// makes the image read the same either way, and look like the app it came
/// from.
fn on_dark_backdrop(svg: &str) -> String {
    let view_box = svg
        .split_once("viewBox=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(v, _)| v)
        .expect("preview always carries a viewBox");
    let n: Vec<f64> = view_box
        .split_whitespace()
        .map(|t| t.parse().expect("numeric viewBox"))
        .collect();
    let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
    let head_end = svg.find('>').expect("opening svg tag") + 1;
    format!(
        "{}<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" fill=\"#16213e\"/>{}",
        &svg[..head_end],
        &svg[head_end..]
    )
}
