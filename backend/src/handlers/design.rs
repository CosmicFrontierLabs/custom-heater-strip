//! POST /api/design — run the heater design engine on an uploaded SVG + specs.

use axum::{http::StatusCode, Json};
use shared::{DesignError, DesignRequest, DesignResponse};

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
        Ok(resp) => Ok(Json(resp)),
        Err(e @ engine::EngineError::Infeasible(_)) => {
            Err(err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

fn err(status: StatusCode, message: String) -> (StatusCode, Json<DesignError>) {
    (status, Json(DesignError { message }))
}
