//! POST /api/design — run the heater design engine on an uploaded SVG + specs.

use axum::{http::StatusCode, Json};
use base64::Engine as _;
use shared::{DesignError, DesignRequest, DesignResponse};
use std::io::Write as _;

/// Cap uploaded SVG size well below anything a sane outline needs.
const MAX_SVG_BYTES: usize = 4 * 1024 * 1024;

pub async fn design(
    Json(req): Json<DesignRequest>,
) -> Result<Json<DesignResponse>, (StatusCode, Json<DesignError>)> {
    if req.svg.len() > MAX_SVG_BYTES {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("SVG is {} bytes; limit is {MAX_SVG_BYTES}", req.svg.len()),
        ));
    }

    // Engine work is CPU-bound (SVG parse + fill); keep it off the reactor.
    let result = tokio::task::spawn_blocking(move || engine::generate(&req))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match result {
        Ok(mut resp) => {
            resp.gerber_zip_base64 = zip_gerbers(&resp)
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Ok(Json(resp))
        }
        Err(e @ engine::EngineError::Infeasible(_)) => {
            Err(err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

/// Bundle the gerber layer set into a base64-encoded zip for download.
fn zip_gerbers(resp: &DesignResponse) -> anyhow::Result<String> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in &resp.gerbers {
            zip.start_file(name, opts)?;
            zip.write_all(body.as_bytes())?;
        }
        zip.finish()?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(buf))
}

fn err(status: StatusCode, message: String) -> (StatusCode, Json<DesignError>) {
    (status, Json(DesignError { message }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_contains_every_gerber_layer() {
        let req = DesignRequest {
            svg: r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="20mm" viewBox="0 0 100 20"><path d="M 0 0 L 100 0 L 100 20 L 0 20 Z"/></svg>"##.to_string(),
            ..DesignRequest::default()
        };
        let resp = engine::generate(&req).unwrap();
        let b64 = zip_gerbers(&resp).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        for expected in resp.gerbers.keys() {
            assert!(names.contains(expected), "{expected} missing from zip");
        }
    }
}
