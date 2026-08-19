//! POST /api/dxf — extract the closed polygons from an uploaded DXF so the
//! frontend can offer them for role assignment (heater region, solder tab,
//! board outline).
//!
//! Parsing happens server-side rather than in the WASM frontend so the DXF
//! crate and its dependencies stay out of the browser bundle.

use axum::{http::StatusCode, Json};
use base64::Engine as _;
use shared::{DesignError, DxfUploadRequest, DxfUploadResponse};

/// Cap uploaded DXF size. ASCII DXF is verbose — a detailed outline with
/// hundreds of splines still lands well inside this.
const MAX_DXF_BYTES: usize = 16 * 1024 * 1024;

pub async fn dxf(
    Json(req): Json<DxfUploadRequest>,
) -> Result<Json<DxfUploadResponse>, (StatusCode, Json<DesignError>)> {
    // Base64 inflates by 4/3; check before decoding so a huge upload is
    // rejected without allocating it twice.
    if req.dxf_base64.len() / 4 * 3 > MAX_DXF_BYTES {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("DXF is larger than the {MAX_DXF_BYTES} byte limit"),
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(req.dxf_base64.trim())
        .map_err(|e| err(StatusCode::BAD_REQUEST, format!("bad base64: {e}")))?;
    if bytes.len() > MAX_DXF_BYTES {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("DXF is {} bytes; limit is {MAX_DXF_BYTES}", bytes.len()),
        ));
    }

    // Parsing and tessellation are CPU-bound; keep them off the reactor.
    let parsed = tokio::task::spawn_blocking(move || engine::dxf::extract(&bytes))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    parsed
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, e.to_string()))
}

fn err(status: StatusCode, message: String) -> (StatusCode, Json<DesignError>) {
    (status, Json(DesignError { message }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dxf_b64() -> String {
        let mut s = String::new();
        s.push_str("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n");
        s.push_str("0\nSECTION\n2\nENTITIES\n");
        s.push_str("0\nLWPOLYLINE\n8\nHEATER\n90\n4\n70\n1\n");
        for (x, y) in [(0.0, 0.0), (40.0, 0.0), (40.0, 10.0), (0.0, 10.0)] {
            s.push_str(&format!("10\n{x}\n20\n{y}\n"));
        }
        s.push_str("0\nENDSEC\n0\nEOF\n");
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[tokio::test]
    async fn extracts_polygons_from_an_upload() {
        let resp = dxf(Json(DxfUploadRequest {
            dxf_base64: sample_dxf_b64(),
        }))
        .await
        .expect("should parse");
        assert_eq!(resp.polygons.len(), 1);
        assert_eq!(resp.polygons[0].layer, "HEATER");
        assert_eq!(resp.polygons[0].suggested_role, shared::PolygonRole::Heater);
        assert_eq!(resp.units, "Millimeters");
    }

    #[tokio::test]
    async fn bad_base64_is_a_client_error() {
        let (status, _) = dxf(Json(DxfUploadRequest {
            dxf_base64: "not base64!!".into(),
        }))
        .await
        .expect_err("should reject");
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_dxf_with_no_closed_rings_is_a_client_error() {
        let empty = base64::engine::general_purpose::STANDARD
            .encode("0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n");
        let (status, body) = dxf(Json(DxfUploadRequest { dxf_base64: empty }))
            .await
            .expect_err("should reject");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.message.contains("closed rings"), "{}", body.message);
    }
}
