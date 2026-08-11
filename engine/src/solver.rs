//! Electrical solver: pick trace width/pitch so a serpentine fill of the
//! outline hits the target resistance R = V²/P without exceeding the
//! current ceiling.

use shared::DesignRequest;

use crate::EngineError;

/// Copper resistivity at 20 °C, Ω·m.
pub const COPPER_RESISTIVITY: f64 = 1.724e-8;
/// Thickness of 1 oz/ft² copper, meters.
pub const OZ_COPPER_THICKNESS_M: f64 = 34.8e-6;

pub struct Solved {
    pub target_resistance_ohms: f64,
    pub width_mm: f64,
    pub pitch_mm: f64,
    pub thickness_m: f64,
}

pub struct Refined {
    pub target_resistance_ohms: f64,
    pub achieved_resistance_ohms: f64,
    pub achieved_watts: f64,
    pub operating_current_amps: f64,
    pub width_mm: f64,
}

/// Analytic first pass. A serpentine of pitch p = w + g filling area A has
/// length L ≈ A/p, so R = ρL/(w·t) = ρA/((w+g)·w·t). Solving for w:
/// w² + g·w − ρA/(R·t) = 0.
pub fn solve(req: &DesignRequest, area_mm2: f64) -> Result<Solved, EngineError> {
    if req.voltage <= 0.0 || req.watts <= 0.0 || req.max_current <= 0.0 {
        return Err(EngineError::Infeasible(
            "voltage, watts, and max current must all be positive".into(),
        ));
    }

    let target_r = req.voltage * req.voltage / req.watts;
    let operating_current = req.voltage / target_r; // == watts / voltage
    if operating_current > req.max_current {
        return Err(EngineError::Infeasible(format!(
            "{:.1} W at {:.1} V draws {:.2} A, over the {:.2} A ceiling; \
             raise the voltage, lower the wattage, or raise the ceiling",
            req.watts, req.voltage, operating_current, req.max_current
        )));
    }

    let t = req.copper_oz * OZ_COPPER_THICKNESS_M;
    let area_m2 = area_mm2 * 1e-6;
    let g = req.min_gap_mm * 1e-3;

    let c = COPPER_RESISTIVITY * area_m2 / (target_r * t);
    let w = (-g + (g * g + 4.0 * c).sqrt()) / 2.0;
    let w_mm = w * 1e3;

    if w_mm < req.min_trace_mm {
        return Err(EngineError::Infeasible(format!(
            "hitting {target_r:.2} Ω needs a {w_mm:.3} mm trace, below the \
             {:.3} mm fab minimum; use thinner copper, a smaller outline, \
             lower voltage, or more watts",
            req.min_trace_mm
        )));
    }

    Ok(Solved {
        target_resistance_ohms: target_r,
        width_mm: w_mm,
        pitch_mm: w_mm + req.min_gap_mm,
        thickness_m: t,
    })
}

/// Second pass: the serpentine's real length differs from the A/p estimate
/// (edge margins, connectors, dropped scanlines). Re-pick the width so the
/// actual path hits target R, keeping the already-generated pitch.
pub fn refine(
    req: &DesignRequest,
    solved: &Solved,
    actual_length_mm: f64,
    warnings: &mut Vec<String>,
) -> Refined {
    let length_m = actual_length_mm * 1e-3;
    let ideal_w_m =
        COPPER_RESISTIVITY * length_m / (solved.target_resistance_ohms * solved.thickness_m);
    let mut w_mm = ideal_w_m * 1e3;

    let max_w = solved.pitch_mm - req.min_gap_mm;
    if w_mm > max_w {
        warnings.push(format!(
            "trace width clamped from {w_mm:.3} mm to {max_w:.3} mm to keep the \
             minimum gap; heater will run above target resistance"
        ));
        w_mm = max_w;
    }
    if w_mm < req.min_trace_mm {
        warnings.push(format!(
            "trace width clamped up from {w_mm:.3} mm to the {:.3} mm fab \
             minimum; heater will run below target resistance (more power)",
            req.min_trace_mm
        ));
        w_mm = req.min_trace_mm;
    }

    let achieved_r = COPPER_RESISTIVITY * length_m / (w_mm * 1e-3 * solved.thickness_m);
    let achieved_watts = req.voltage * req.voltage / achieved_r;
    let operating_current = req.voltage / achieved_r;

    if operating_current > req.max_current {
        warnings.push(format!(
            "achieved resistance {achieved_r:.2} Ω draws {operating_current:.2} A, \
             over the {:.2} A ceiling",
            req.max_current
        ));
    }

    Refined {
        target_resistance_ohms: solved.target_resistance_ohms,
        achieved_resistance_ohms: achieved_r,
        achieved_watts,
        operating_current_amps: operating_current,
        width_mm: w_mm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_req() -> DesignRequest {
        DesignRequest {
            svg: String::new(),
            voltage: 12.0,
            watts: 10.0,
            max_current: 2.0,
            copper_oz: 0.5,
            min_trace_mm: 0.15,
            min_gap_mm: 0.15,
            edge_margin_mm: 0.5,
            ..DesignRequest::default()
        }
    }

    #[test]
    fn analytic_solution_satisfies_quadratic() {
        let area_mm2 = 2000.0; // 100 × 20 mm strip
        let s = solve(&base_req(), area_mm2).unwrap();
        // Plug w back in: R = ρA/((w+g)·w·t) should equal target.
        let w = s.width_mm * 1e-3;
        let g = 0.15e-3;
        let r = COPPER_RESISTIVITY * area_mm2 * 1e-6 / ((w + g) * w * s.thickness_m);
        assert!((r - s.target_resistance_ohms).abs() / s.target_resistance_ohms < 1e-9);
    }

    #[test]
    fn current_ceiling_enforced() {
        let mut req = base_req();
        req.watts = 30.0; // 2.5 A at 12 V
        assert!(matches!(
            solve(&req, 2000.0),
            Err(EngineError::Infeasible(_))
        ));
    }

    #[test]
    fn refine_hits_target_exactly_when_unclamped() {
        let req = base_req();
        let s = solve(&req, 2000.0).unwrap();
        // Pretend the serpentine came out exactly at the analytic estimate.
        let est_len = 2000.0 / s.pitch_mm;
        let r = refine(&req, &s, est_len, &mut Vec::new());
        assert!(
            (r.achieved_resistance_ohms - r.target_resistance_ohms).abs()
                / r.target_resistance_ohms
                < 1e-6
        );
    }
}
