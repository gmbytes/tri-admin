use super::error::AppError;
use axum::{
    Json,
    response::{IntoResponse, Response},
};
use mongodb::bson::{Document, doc};
use serde::{Deserialize, Serialize};

fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageInput {
    #[serde(default = "first")]
    pub page: u64,
    #[serde(default = "size")]
    pub page_size: u64,
    pub search: Option<String>,
    pub sort: Option<String>,
}

impl PageInput {
    pub fn validate(&self) -> Result<u64, AppError> {
        if self.page == 0
            || !(1..=100).contains(&self.page_size)
            || self
                .search
                .as_ref()
                .is_some_and(|value| value.chars().count() > 100)
        {
            return Err(AppError::Validation("分页或搜索参数无效".into()));
        }
        self.page
            .checked_sub(1)
            .and_then(|page| page.checked_mul(self.page_size))
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(AppError::Validation("分页范围过大".into()))
    }
    pub fn sort_document(&self, allowed: &[&str]) -> Result<Document, AppError> {
        let raw = self.sort.as_deref().unwrap_or("-createdAt");
        let (field, direction) = raw
            .strip_prefix('-')
            .map(|value| (value, -1))
            .unwrap_or((raw, 1));
        if !allowed.contains(&field) {
            return Err(AppError::Validation("不支持的排序字段".into()));
        }
        let mut sort = Document::new();
        sort.insert(field, direction);
        sort.insert("_id", direction);
        Ok(sort)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageMeta {
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}
#[derive(Serialize)]
pub struct Page<T: Serialize> {
    pub data: Vec<T>,
    pub meta: PageMeta,
}
impl<T: Serialize> Page<T> {
    pub fn new(data: Vec<T>, input: &PageInput, total: u64) -> Self {
        Self {
            data,
            meta: PageMeta {
                page: input.page,
                page_size: input.page_size,
                total,
            },
        }
    }
}
impl<T: Serialize> IntoResponse for Page<T> {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}
