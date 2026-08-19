//! Render one design to SVG on stdout: `cargo run -p engine --example preview_cf -- <FillKind>`
use shared::{CornerStyle, DesignRequest, FillKind};
fn main() {
    let want = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Counterflow".into());
    let kind = FillKind::ALL
        .into_iter()
        .find(|k| format!("{k:?}") == want)
        .expect("unknown fill kind");
    let req = DesignRequest {
        svg: r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="20mm" viewBox="0 0 100 20"><path d="M 0 0 L 100 0 L 100 20 L 0 20 Z"/></svg>"##.to_string(),
        corner_style: CornerStyle::Smooth,
        fill_kind: kind,
        ..DesignRequest::default()
    };
    let resp = engine::generate(&req).expect("design");
    print!("{}", resp.preview_svg);
}
