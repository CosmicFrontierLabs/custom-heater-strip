use serde::{Deserialize, Serialize};
use uuid::Uuid;
use ws_bridge::WsEndpoint;

// ---------------------------------------------------------------------------
// WebSocket endpoint definition — single source of truth for server + client
// ---------------------------------------------------------------------------

/// The main application WebSocket endpoint.
pub struct AppSocket;

impl WsEndpoint for AppSocket {
    const PATH: &'static str = "/ws";
    type ServerMsg = ServerMsg;
    type ClientMsg = ClientMsg;
}

/// Messages sent from the server to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    /// Heartbeat to keep connection alive
    Heartbeat,

    /// Error from server
    Error { message: String },

    /// Server is shutting down
    ServerShutdown {
        reason: String,
        reconnect_delay_ms: u64,
    },
}

/// Messages sent from the client to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    /// Ping — server should respond with Heartbeat
    Ping,
}

// ---------------------------------------------------------------------------
// HTTP API types
// ---------------------------------------------------------------------------

/// Health check response from `/api/health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
}

/// Example API item (matches the `items` database table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::NaiveDateTime,
}

/// Request body for creating a new item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateItemRequest {
    pub name: String,
}

// ---------------------------------------------------------------------------
// Heater design API
// ---------------------------------------------------------------------------

/// Request body for `POST /api/design`.
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

/// Request body for `POST /api/dxf`: an uploaded DXF to extract polygons from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DxfUploadRequest {
    /// The DXF file, base64-encoded (handles both ASCII and binary DXF).
    pub dxf_base64: String,
}

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

/// Response body for `POST /api/dxf`.
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
    /// A region to fill with heater trace.
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
    /// Copper thickness used, in µm.
    pub copper_thickness_um: f64,
    /// Non-fatal issues the generator noticed (concave regions, clamped widths…).
    pub warnings: Vec<String>,
}

/// Response body for `POST /api/design`.
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

/// Error response for a failed design request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignError {
    pub message: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_msg_heartbeat_roundtrip() {
        let msg = ServerMsg::Heartbeat;
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMsg = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ServerMsg::Heartbeat));
    }

    #[test]
    fn server_msg_error_roundtrip() {
        let msg = ServerMsg::Error {
            message: "something broke".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMsg = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMsg::Error { message } => assert_eq!(message, "something broke"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn server_msg_shutdown_roundtrip() {
        let msg = ServerMsg::ServerShutdown {
            reason: "restarting".to_string(),
            reconnect_delay_ms: 1000,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMsg = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMsg::ServerShutdown {
                reason,
                reconnect_delay_ms,
            } => {
                assert_eq!(reason, "restarting");
                assert_eq!(reconnect_delay_ms, 1000);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn client_msg_ping_roundtrip() {
        let msg = ClientMsg::Ping;
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ClientMsg = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ClientMsg::Ping));
    }

    #[test]
    fn item_roundtrip() {
        let item = Item {
            id: Uuid::new_v4(),
            name: "test item".to_string(),
            created_at: chrono::Utc::now().naive_utc(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: Item = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, item.id);
        assert_eq!(parsed.name, item.name);
    }
}
