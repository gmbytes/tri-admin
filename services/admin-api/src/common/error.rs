use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

pub fn normalize_framework(response: Response) -> Response {
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error())
        || response.extensions().get::<ResponseError>().is_some()
    {
        return response;
    }
    let error = match status.as_u16() {
        400 => AppError::BadRequest,
        401 => AppError::Unauthorized,
        403 => AppError::Forbidden,
        404 => AppError::NotFound,
        405 => AppError::MethodNotAllowed,
        409 => AppError::Conflict("请求与当前状态冲突".into()),
        413 => AppError::PayloadTooLarge,
        415 => AppError::UnsupportedMediaType,
        422 => AppError::Validation("请求字段无效".into()),
        429 => AppError::RateLimited,
        503 => AppError::NotReady,
        _ if status.is_server_error() => AppError::Internal,
        _ => AppError::BadRequest,
    };
    let (mut original, _) = response.into_parts();
    let (safe, body) = error.into_response().into_parts();
    original.headers.remove(axum::http::header::CONTENT_LENGTH);
    original.headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    original.extensions.extend(safe.extensions);
    Response::from_parts(original, body)
}

#[derive(Clone, Copy)]
pub struct ResponseError {
    pub code: &'static str,
}

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
    #[error("配置密钥库暂不可用")]
    SecretStoreUnavailable,
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
            Self::SecretStoreUnavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, "SECRET_STORE_UNAVAILABLE")
            }
            Self::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
            Self::Database(_) => (StatusCode::SERVICE_UNAVAILABLE, "DATABASE_UNAVAILABLE"),
            Self::Configuration(_) | Self::SchemaMismatch | Self::Internal => {
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
            }
        };
        let mut response = (
            status,
            Json(json!({"error":{"code":code,"message":self.to_string()}})),
        )
            .into_response();
        response.extensions_mut().insert(ResponseError { code });
        response
    }
}
