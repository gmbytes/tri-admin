use super::error::AppError;
use crate::state::AppState;
use axum::{
    body::Bytes,
    extract::{FromRequest, Request},
    http::StatusCode,
};
use serde::{Deserialize, de::DeserializeOwned};

pub async fn json<T: DeserializeOwned>(request: Request, state: &AppState) -> Result<T, AppError> {
    let content_type = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok());
    if !content_type.is_some_and(|value| {
        value.type_() == mime::APPLICATION
            && (value.subtype() == mime::JSON || value.suffix() == Some(mime::JSON))
    }) {
        return Err(AppError::UnsupportedMediaType);
    }
    let bytes =
        Bytes::from_request(request, state)
            .await
            .map_err(|error| match error.status() {
                StatusCode::PAYLOAD_TOO_LARGE => AppError::PayloadTooLarge,
                _ => AppError::BadRequest,
            })?;
    // Validate the entire document without allocating a JSON value tree. A field error
    // must not hide a later syntax error, and unknown values must not be echoed.
    serde_json::from_slice::<serde::de::IgnoredAny>(&bytes).map_err(|_| AppError::BadRequest)?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::Validation("请求 JSON 字段无效".into()))
}

pub fn optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

pub fn optional_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
