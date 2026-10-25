use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("请求 JSON 格式无效")]
    BadRequest,
    #[error("请求方法不受支持")]
    MethodNotAllowed,
    #[error("请求内容过大")]
    PayloadTooLarge,
    #[error("请求数据格式不受支持")]
    UnsupportedMediaType,
    #[error("请先登录")]
    Unauthorized,
    #[error("账号或密码错误")]
    InvalidCredentials,
    #[error("请求过于频繁，请稍后重试")]
    RateLimited,
    #[error("权限不足")]
    Forbidden,
    #[error("资源不存在")]
    NotFound,
    #[error("服务尚未就绪")]
    NotReady,
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Configuration(String),
    #[error("数据模型版本不受支持，请检查部署版本")]
    SchemaMismatch,
    #[error("数据库暂不可用")]
    Database(#[from] mongodb::error::Error),
    #[error("服务内部错误")]
    Internal,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::BadRequest => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            Self::MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, "METHOD_NOT_ALLOWED"),
            Self::PayloadTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "PAYLOAD_TOO_LARGE"),
            Self::UnsupportedMediaType => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "UNSUPPORTED_MEDIA_TYPE")
            }
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "UNAUTHENTICATED"),
            Self::InvalidCredentials => (StatusCode::UNAUTHORIZED, "INVALID_CREDENTIALS"),
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "FORBIDDEN"),
            Self::NotFound => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            Self::NotReady => (StatusCode::SERVICE_UNAVAILABLE, "SERVICE_NOT_READY"),
            Self::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
            Self::Database(_) => (StatusCode::SERVICE_UNAVAILABLE, "DATABASE_UNAVAILABLE"),
            Self::Configuration(_) | Self::SchemaMismatch | Self::Internal => {
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
            }
        };
        (
            status,
            Json(json!({"error":{"code":code,"message":self.to_string()}})),
        )
            .into_response()
    }
}
