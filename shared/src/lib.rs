use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Heater design API
// ---------------------------------------------------------------------------

/// Everything the engine needs to design one heater.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignRequest {
    /// SVG document containing the desired heater outline as its first path.
    pub svg: String,
    /// Supply voltage across the heater, in volts.
    pub voltage: f64,
    /// Target power dissipation, in watts.
    pub watts: f64,
    /// Maximum allowed current draw, in amps.
    pub max_current: f64,
    /// Copper weight in oz/ft² (flex is typically 0.5 or 1.0).
    pub copper_oz: f64,
    /// Minimum manufacturable trace width, in mm.
    pub min_trace_mm: f64,
    /// Minimum manufacturable trace-to-trace gap, in mm.
    pub min_gap_mm: f64,
    /// Clearance between the trace and the board outline, in mm.
    pub edge_margin_mm: f64,
    /// Diameter of the solder terminal pads at the trace ends, in mm.
    #[serde(default = "default_pad_diameter")]
    pub pad_diameter_mm: f64,
    /// How serpentine turnarounds are drawn.
    #[serde(default)]
    pub corner_style: CornerStyle,
    /// Which fill pattern routes the heater trace.
    #[serde(default)]
    pub fill_kind: FillKind,
    /// Explicit polygon geometry from a DXF upload. When set, this supersedes
    /// `svg`: the listed heater regions are filled and chained in series and
    /// the tab polygons become the solder pads.
    #[serde(default)]
    pub geometry: Option<GeometrySpec>,
}

/// Trace fill pattern. All patterns produce one continuous non-crossing
/// path at uniform pitch with both ends at the terminal zone; see
/// docs/fill-patterns.md for the research behind the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FillKind {
    /// Boustrophedon rows — the classic strip heater.
    #[default]
    Serpentine,
    /// Serpentine with sinusoidal rows: same electrical behavior, much
    /// better flex-fatigue life.
    WavySerpentine,
    /// Out-and-back interleaved serpentine (bifilar): non-inductive,
    /// current counterflows everywhere.
    Counterflow,
    /// Generalized Hilbert space-filling curve: best thermal isotropy.
    /// Rectangular outlines only.
    Hilbert,
    /// Two interleaved Archimedean spiral arms joined at the center.
    /// Fills the inscribed circle; best for round outlines.
    DoubleSpiral,
    /// Concentric outline insets spliced into one path: best coverage of
    /// irregular outlines.
    Concentric,
}

impl FillKind {
    pub const ALL: [FillKind; 6] = [
        FillKind::Serpentine,
        FillKind::WavySerpentine,
        FillKind::Counterflow,
        FillKind::Hilbert,
        FillKind::DoubleSpiral,
        FillKind::Concentric,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            FillKind::Serpentine => "Serpentine",
            FillKind::WavySerpentine => "Wavy serpentine",
            FillKind::Counterflow => "Counterflow (bifilar)",
            FillKind::Hilbert => "Hilbert curve",
            FillKind::DoubleSpiral => "Double spiral",
            FillKind::Concentric => "Concentric",
        }
    }
}

/// Serpentine turnaround geometry, matching the corner options in most EDA
/// tools. Smooth is the default: arcs avoid current crowding at the turns,
/// which matters on a trace that is deliberately run hot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CornerStyle {
    /// Square 90° turnarounds.
    Rectangular,
    /// 45° chamfered turnarounds.
    Mitered,
    /// Semicircular arc turnarounds (true arcs in the Gerber/KiCad output).
    #[default]
    Smooth,
}

impl CornerStyle {
    pub const ALL: [CornerStyle; 3] = [
        CornerStyle::Smooth,
        CornerStyle::Mitered,
        CornerStyle::Rectangular,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            CornerStyle::Rectangular => "Rectangular",
            CornerStyle::Mitered => "Mitered (45°)",
            CornerStyle::Smooth => "Smooth (arcs)",
        }
    }
}

fn default_pad_diameter() -> f64 {
    2.5
}

fn one() -> usize {
    1
}

impl Default for DesignRequest {
    fn default() -> Self {
        Self {
            svg: String::new(),
            voltage: 12.0,
            watts: 10.0,
            max_current: 2.0,
            copper_oz: 0.5,
            min_trace_mm: 0.15,
            min_gap_mm: 0.15,
            edge_margin_mm: 0.5,
            pad_diameter_mm: default_pad_diameter(),
            corner_style: CornerStyle::default(),
            fill_kind: FillKind::default(),
            geometry: None,
        }
    }
}

/// A fab's flex-PCB process limits, used to prefill the design form.
#[derive(Debug, Clone, PartialEq)]
pub struct FabPreset {
    pub name: &'static str,
    pub copper_oz: f64,
    pub min_trace_mm: f64,
    pub min_gap_mm: f64,
}

/// Published flex capabilities of popular fabs (with a little margin over
/// their absolute minimums). Sources: jlcpcb.com/capabilities/flex-pcb-capabilities,
/// docs.oshpark.com/services/flex/.
pub const FAB_PRESETS: &[FabPreset] = &[
    // 18 µm copper, 3.5/3.5 mil (0.089 mm) minimum
    FabPreset {
        name: "JLCPCB flex · 0.5 oz",
        copper_oz: 0.5,
        min_trace_mm: 0.09,
        min_gap_mm: 0.09,
    },
    // 35 µm copper, 4/4 mil (0.102 mm) minimum
    FabPreset {
        name: "JLCPCB flex · 1 oz",
        copper_oz: 1.0,
        min_trace_mm: 0.11,
        min_gap_mm: 0.11,
    },
    // Fixed 1 oz / Felios PI stackup, 6/6 mil (0.152 mm) minimum
    FabPreset {
        name: "OSH Park flex · 1 oz",
        copper_oz: 1.0,
        min_trace_mm: 0.16,
        min_gap_mm: 0.16,
    },
];

// ---------------------------------------------------------------------------
// DXF geometry API
// ---------------------------------------------------------------------------

/// One closed ring pulled out of a DXF, in board mm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DxfPolygon {
    /// Stable index used by the picker to address this polygon.
    pub id: u32,
    /// DXF layer the entity came from.
    pub layer: String,
    /// Source entity type, shown in the picker ("LWPOLYLINE", "CIRCLE", …).
    pub kind: String,
    /// Closed ring in mm, y-down, translated so the drawing starts at (0,0).
    pub points: Vec<[f64; 2]>,
    pub area_mm2: f64,
    /// Role guessed from the layer name; the user can override it.
    pub suggested_role: PolygonRole,
}

/// The polygons pulled out of an uploaded DXF, ready for role assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DxfUploadResponse {
    pub polygons: Vec<DxfPolygon>,
    /// The `$INSUNITS` value the coordinates were scaled from.
    pub units: String,
    pub warnings: Vec<String>,
}

/// What a DXF polygon contributes to the design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PolygonRole {
    /// Ignored entirely (construction lines, dimensions, title block…).
    #[default]
    Unused,
    /// The board profile on Edge.Cuts. Defaults to the heaters' bounds.
    Outline,
    /// A region to fill with heater trace. Several regions chain in series.
    Heater,
    /// The supply-side solder tab; its polygon becomes the pad copper.
    TabIn,
    /// The return-side solder tab.
    TabOut,
}

impl PolygonRole {
    /// Click-cycle order in the picker.
    pub const ALL: [PolygonRole; 5] = [
        PolygonRole::Unused,
        PolygonRole::Heater,
        PolygonRole::TabIn,
        PolygonRole::TabOut,
        PolygonRole::Outline,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            PolygonRole::Unused => "Unused",
            PolygonRole::Outline => "Board outline",
            PolygonRole::Heater => "Heater region",
            PolygonRole::TabIn => "Tab (in)",
            PolygonRole::TabOut => "Tab (out)",
        }
    }

    /// Colour the picker draws this role in (also used in the legend).
    pub fn color(&self) -> &'static str {
        match self {
            PolygonRole::Unused => "#565f89",
            PolygonRole::Outline => "#bb9af7",
            PolygonRole::Heater => "#d98f3d",
            PolygonRole::TabIn => "#9ece6a",
            PolygonRole::TabOut => "#7dcfff",
        }
    }

    /// Next role when the user clicks a polygon.
    pub fn next(&self) -> PolygonRole {
        let i = Self::ALL.iter().position(|r| r == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Explicit polygon geometry, used instead of SVG outline extraction when the
/// design came from a DXF with roles assigned in the picker.
///
/// Rings are closed, in mm, y-down. Heater regions are filled in the order
/// given and chained in series; the tabs become the actual pad copper.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeometrySpec {
    /// Board profile. When `None`, the bounding box of everything else is used.
    #[serde(default)]
    pub outline: Option<Vec<[f64; 2]>>,
    pub heaters: Vec<Vec<[f64; 2]>>,
    #[serde(default)]
    pub tab_in: Option<Vec<[f64; 2]>>,
    #[serde(default)]
    pub tab_out: Option<Vec<[f64; 2]>>,
}

/// Computed electrical + geometric summary of a generated heater design.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignReport {
    /// Target resistance implied by V²/P, in ohms.
    pub target_resistance_ohms: f64,
    /// Resistance the generated trace actually achieves at 20 °C, in ohms.
    pub achieved_resistance_ohms: f64,
    /// Power at the supply voltage with the achieved resistance, in watts.
    pub achieved_watts: f64,
    /// Operating current at the supply voltage, in amps.
    pub operating_current_amps: f64,
    /// Fraction of the current ceiling used (operating / max).
    pub current_headroom_frac: f64,
    /// Trace width, in mm.
    pub trace_width_mm: f64,
    /// Trace-to-trace gap, in mm.
    pub trace_gap_mm: f64,
    /// Total trace length, in mm.
    pub trace_length_mm: f64,
    /// Heated outline area, in cm².
    pub outline_area_cm2: f64,
    /// Average power density over the outline, in W/cm².
    pub power_density_w_cm2: f64,
    /// How many heater regions were chained in series (1 for a plain strip).
    #[serde(default = "one")]
    pub region_count: usize,
    /// Length of the traces linking the regions to each other and to the
    /// tabs, in mm — resistance that heats the interconnect, not the board.
    #[serde(default)]
    pub link_length_mm: f64,
    /// Copper thickness used, in µm.
    pub copper_thickness_um: f64,
    /// Non-fatal issues the generator noticed (concave regions, clamped widths…).
    pub warnings: Vec<String>,
}

/// A finished design: preview, fab outputs, and the numbers behind them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignResponse {
    pub report: DesignReport,
    /// SVG rendering of the routed heater for in-browser preview.
    pub preview_svg: String,
    /// Complete `.kicad_pcb` file contents.
    pub kicad_pcb: String,
    /// Gerber layers keyed by filename (e.g. "heater-F_Cu.gtl").
    pub gerbers: std::collections::BTreeMap<String, String>,
    /// The same gerber set as a base64-encoded .zip, ready to download.
    #[serde(default)]
    pub gerber_zip_base64: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_request_roundtrips_with_geometry() {
        let req = DesignRequest {
            geometry: Some(GeometrySpec {
                outline: None,
                heaters: vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]]],
                tab_in: Some(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]),
                tab_out: None,
            }),
            ..DesignRequest::default()
        };
        let parsed: DesignRequest =
            serde_json::from_str(&serde_json::to_string(&req).unwrap()).expect("roundtrip");
        let g = parsed.geometry.expect("geometry survives");
        assert_eq!(g.heaters.len(), 1);
        assert_eq!(g.tab_in.unwrap().len(), 3);
        assert!(g.tab_out.is_none());
    }

    /// Older payloads have no `geometry` key at all; they must still load and
    /// take the single-region SVG path rather than failing to deserialise.
    #[test]
    fn a_request_without_geometry_defaults_to_none() {
        let json = r#"{"svg":"<svg/>","voltage":12.0,"watts":10.0,"max_current":2.0,
                       "copper_oz":0.5,"min_trace_mm":0.15,"min_gap_mm":0.15,
                       "edge_margin_mm":0.5}"#;
        let req: DesignRequest = serde_json::from_str(json).expect("defaults apply");
        assert!(req.geometry.is_none());
        assert_eq!(req.pad_diameter_mm, default_pad_diameter());
        assert_eq!(req.fill_kind, FillKind::Serpentine);
    }

    #[test]
    fn role_cycling_visits_every_role_and_returns_to_the_start() {
        let mut seen = vec![PolygonRole::Unused];
        let mut r = PolygonRole::Unused;
        for _ in 0..PolygonRole::ALL.len() {
            r = r.next();
            seen.push(r);
        }
        // Back where it started after a full lap.
        assert_eq!(r, PolygonRole::Unused);
        for role in PolygonRole::ALL {
            assert!(seen.contains(&role), "{role:?} never appears in the cycle");
        }
    }

    #[test]
    fn every_role_has_a_distinct_colour() {
        let mut colours: Vec<&str> = PolygonRole::ALL.iter().map(|r| r.color()).collect();
        colours.sort_unstable();
        let n = colours.len();
        colours.dedup();
        assert_eq!(colours.len(), n, "two roles share a colour");
    }
}
