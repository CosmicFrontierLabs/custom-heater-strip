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
        // The engine returns a finished response, archive included.
        Ok(resp) => Ok(Json(resp)),
        Err(e @ engine::EngineError::Infeasible(_)) => {
            Err(err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        Err(e @ engine::EngineError::Archive(_)) => {
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

fn err(status: StatusCode, message: String) -> (StatusCode, Json<DesignError>) {
    (status, Json(DesignError { message }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn an_oversized_svg_is_rejected_before_the_engine_runs() {
        let (status, _) = design(Json(DesignRequest {
            svg: "x".repeat(MAX_SVG_BYTES + 1),
            ..DesignRequest::default()
        }))
        .await
        .expect_err("should reject");
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn an_infeasible_design_is_unprocessable_not_a_bad_request() {
        let (status, _) = design(Json(DesignRequest {
            svg: r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="20mm" viewBox="0 0 100 20"><path d="M 0 0 L 100 0 L 100 20 L 0 20 Z"/></svg>"##.to_string(),
            // 100 W at 12 V draws 8.3 A, far over the 2 A ceiling.
            watts: 100.0,
            ..DesignRequest::default()
        }))
        .await
        .expect_err("should reject");
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
