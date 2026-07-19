//! The HTTP error envelope, shared by every inbound adapter.
//!
//! ```json
//! {"error": {"code": "unauthorized", "message": "invalid API key"}}
//! ```
//!
//! A stable machine-readable `code` plus a human `message`. Clients
//! branch on the code; the message is for the person reading the log.
//!
//! This lives in `shared` rather than in a context because the envelope
//! is a cross-cutting transport shape: `memories` and `understanding`
//! must return the identical structure, and duplicating it per context is
//! how APIs end up with three spellings of "not found".

use crate::shared::error::RaError;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorDetail {
    pub code: &'static str,
    pub message: String,
}

impl IntoResponse for RaError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            RaError::NotFound(message) => (StatusCode::NOT_FOUND, "not_found", message),
            RaError::Validation(message) => (StatusCode::BAD_REQUEST, "validation_failed", message),
            RaError::Unauthorized(message) => (StatusCode::UNAUTHORIZED, "unauthorized", message),
            RaError::Forbidden(message) => (StatusCode::FORBIDDEN, "forbidden", message),
            RaError::Conflict(message) => (StatusCode::CONFLICT, "conflict", message),
            RaError::Internal(detail) => {
                // Internal messages carry file paths, SQL and driver text.
                // They belong in the log, not in a response body.
                tracing::error!(error = %detail, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal error".to_string(),
                )
            }
        };

        let body = Json(ApiErrorBody {
            error: ApiErrorDetail { code, message },
        });

        if status == StatusCode::UNAUTHORIZED {
            // RFC 9110: a 401 must say how to authenticate.
            return (
                status,
                [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
                body,
            )
                .into_response();
        }

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn parts(error: RaError) -> (StatusCode, serde_json::Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn maps_each_error_to_its_status_and_code() {
        let cases = [
            (
                RaError::NotFound("user not found".into()),
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                RaError::Validation("handle is empty".into()),
                StatusCode::BAD_REQUEST,
                "validation_failed",
            ),
            (
                RaError::Unauthorized("invalid API key".into()),
                StatusCode::UNAUTHORIZED,
                "unauthorized",
            ),
            (
                RaError::Forbidden("missing write scope".into()),
                StatusCode::FORBIDDEN,
                "forbidden",
            ),
            (
                RaError::Conflict("already exists".into()),
                StatusCode::CONFLICT,
                "conflict",
            ),
        ];

        for (error, expected_status, expected_code) in cases {
            let message = error.to_string();
            let (status, body) = parts(error).await;
            assert_eq!(status, expected_status, "for {message}");
            assert_eq!(body["error"]["code"], expected_code);
            assert!(body["error"]["message"].is_string());
        }
    }

    #[tokio::test]
    async fn internal_errors_never_leak_their_detail() {
        let (status, body) = parts(RaError::Internal(
            "database error: no such table /home/alex/.recordagent/data.db".into(),
        ))
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["code"], "internal");
        assert_eq!(body["error"]["message"], "internal error");
    }

    #[tokio::test]
    async fn unauthorized_advertises_the_scheme() {
        let response = RaError::Unauthorized("invalid API key".into()).into_response();

        assert_eq!(
            response
                .headers()
                .get(axum::http::header::WWW_AUTHENTICATE)
                .unwrap(),
            "Bearer"
        );
    }
}
