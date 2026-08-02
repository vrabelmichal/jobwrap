//! HTTP API errors.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::json;

use jobwrap_protocol::ApiErrorCode;

/// A typed API error that maps to a stable JSON shape and HTTP status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub code: ApiErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ApiErrorCode::BadRequest, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(ApiErrorCode::Unauthorized, message)
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(ApiErrorCode::PermissionDenied, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ApiErrorCode::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ApiErrorCode::Conflict, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ApiErrorCode::Internal, message)
    }

    fn status(&self) -> StatusCode {
        match self.code {
            ApiErrorCode::BadRequest => StatusCode::BAD_REQUEST,
            ApiErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiErrorCode::PermissionDenied => StatusCode::FORBIDDEN,
            ApiErrorCode::NotFound => StatusCode::NOT_FOUND,
            ApiErrorCode::Conflict => StatusCode::CONFLICT,
            ApiErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ApiErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ApiError {}

/// The wire error body.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

/// Response adapter: OK with a payload or an ApiError.
pub enum HttpError {
    Ok(StatusCode),
    Err(ApiError),
}

impl From<Result<(), ApiError>> for HttpError {
    fn from(result: Result<(), ApiError>) -> Self {
        match result {
            Ok(()) => HttpError::Ok(StatusCode::NO_CONTENT),
            Err(e) => HttpError::Err(e),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "error": {
                "code": self.code.as_str(),
                "message": self.message,
            }
        }));
        (self.status(), body).into_response()
    }
}
