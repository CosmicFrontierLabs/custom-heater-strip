//! Report what a DXF turns into: `cargo run -p engine --example probe_dxf -- FILE...`
fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read");
        println!("== {path}");
        match engine::dxf::extract(&bytes) {
            Err(e) => println!("   ERROR: {e}"),
            Ok(r) => {
                println!("   units: {}", r.units);
                for w in &r.warnings {
                    println!("   warn: {w}");
                }
                for p in &r.polygons {
                    let xs: Vec<f64> = p.points.iter().map(|q| q[0]).collect();
                    let ys: Vec<f64> = p.points.iter().map(|q| q[1]).collect();
                    let (w, h) = (
                        xs.iter().cloned().fold(f64::MIN, f64::max)
                            - xs.iter().cloned().fold(f64::MAX, f64::min),
                        ys.iter().cloned().fold(f64::MIN, f64::max)
                            - ys.iter().cloned().fold(f64::MAX, f64::min),
                    );
                    println!(
                        "   #{} {:<12} {:>4} pts  {:>9.3} mm2  {:.3} x {:.3} mm",
                        p.id,
                        p.kind,
                        p.points.len(),
                        p.area_mm2,
                        w,
                        h
                    );
                }
            }
        }
    }
}
