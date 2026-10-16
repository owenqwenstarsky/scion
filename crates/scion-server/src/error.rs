use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub extra: serde_json::Value,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            extra: json!({}),
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "authentication required",
        )
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "forbidden")
    }

    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "error": { "code": self.code, "message": self.message }
        });
        if let Some(obj) = body["error"].as_object_mut() {
            if let Some(extra) = self.extra.as_object() {
                for (k, v) in extra {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<scion_store::Error> for ApiError {
    fn from(err: scion_store::Error) -> Self {
        match err {
            scion_store::Error::Core(scion_core::Error::NotFound) => Self::not_found(),
            scion_store::Error::Core(other) => Self::bad(other.to_string()),
            other => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store",
                other.to_string(),
            ),
        }
    }
}

impl From<scion_iam::Error> for ApiError {
    fn from(err: scion_iam::Error) -> Self {
        match err {
            scion_iam::Error::InvalidCredentials => Self::new(
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                err.to_string(),
            ),
            scion_iam::Error::AlreadyBootstrapped => Self::new(
                StatusCode::CONFLICT,
                "already_bootstrapped",
                err.to_string(),
            ),
            scion_iam::Error::NotFound => Self::not_found(),
            scion_iam::Error::Forbidden => Self::forbidden(),
            scion_iam::Error::Core(e) => Self::bad(e.to_string()),
            other => Self::new(StatusCode::BAD_REQUEST, "iam", other.to_string()),
        }
    }
}
