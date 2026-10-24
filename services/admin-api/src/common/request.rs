use super::error::AppError;
use crate::state::AppState;
use axum::{
    Json,
    extract::{FromRequest, Request},
};
use serde::{Deserialize, de::DeserializeOwned};

pub async fn json<T: DeserializeOwned>(request: Request, state: &AppState) -> Result<T, AppError> {
    Json::<T>::from_request(request, state)
        .await
        .map(|Json(value)| value)
        .map_err(|_| AppError::Validation("请求 JSON 格式或字段无效".into()))
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
