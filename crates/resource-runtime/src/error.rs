use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mongodb::error::{ErrorKind, WriteFailure};
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("参数无效")]
    Validation,
    #[error("需要有效登录")]
    Unauthorized,
    #[error("当前账号没有写入权限")]
    Forbidden,
    #[error("记录不存在")]
    NotFound,
    #[error("记录版本或唯一字段冲突")]
    Conflict,
    #[error("请求过于频繁，请稍后再试")]
    RateLimited,
    #[error("请求内容过大")]
    PayloadTooLarge,
    #[error("资源模型或服务配置不匹配")]
    NotReady,
    #[error("服务暂时不可用")]
    Internal,
}
impl From<mongodb::error::Error> for Error {
    fn from(value: mongodb::error::Error) -> Self {
        if let Some(error) = value.get_custom::<Self>() {
            return error.clone();
        }
        if matches!(&*value.kind,ErrorKind::Write(WriteFailure::WriteError(error)) if error.code==11000)
            || matches!(&*value.kind,ErrorKind::Command(error) if error.code==11000)
        {
            Self::Conflict
        } else {
            Self::Internal
        }
    }
}
impl From<crate::schema::SchemaError> for Error {
    fn from(_: crate::schema::SchemaError) -> Self {
        Self::Validation
    }
}
impl Error {
    pub(crate) fn mongo(self) -> mongodb::error::Error {
        mongodb::error::Error::custom(self)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            Self::Validation => (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_FAILED"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "AUTH_REQUIRED"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "FORBIDDEN"),
            Self::NotFound => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            Self::Conflict => (StatusCode::CONFLICT, "VERSION_OR_UNIQUE_CONFLICT"),
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED"),
            Self::PayloadTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "PAYLOAD_TOO_LARGE"),
            Self::NotReady => (StatusCode::SERVICE_UNAVAILABLE, "NOT_READY"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };
        let mut response = (
            status,
            Json(serde_json::json!({"error":{"code":code,"message":self.to_string()}})),
        )
            .into_response();
        response.headers_mut().insert(
            "cache-control",
            axum::http::HeaderValue::from_static("private, no-store"),
        );
        if matches!(self, Self::RateLimited) {
            response
                .headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("60"));
        }
        response
    }
}
