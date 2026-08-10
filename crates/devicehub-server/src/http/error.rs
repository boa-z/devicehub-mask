//! Stable JSON errors shared by HTTP host adapters.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use devicehub_core::{ManagedOperationError, OperationSuggestedAction};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub suggested_action: Option<OperationSuggestedAction>,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    status: StatusCode,
    body: ApiErrorBody,
}

impl ApiError {
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ApiErrorBody {
                error: ApiErrorDetail {
                    code: code.into(),
                    message: message.into(),
                    retryable: false,
                    suggested_action: None,
                },
            },
        }
    }

    pub fn retryable(mut self, suggested_action: OperationSuggestedAction) -> Self {
        self.body.error.retryable = true;
        self.body.error.suggested_action = Some(suggested_action);
        self
    }

    pub fn from_operation(status: StatusCode, error: ManagedOperationError) -> Self {
        Self {
            status,
            body: ApiErrorBody {
                error: ApiErrorDetail {
                    code: error.code.as_str().into(),
                    message: error.message,
                    retryable: error.retryable,
                    suggested_action: error.suggested_action,
                },
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
