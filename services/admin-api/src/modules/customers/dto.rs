use crate::common::{pagination::PageInput, request::optional_non_null};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomerStatus {
    #[default]
    Active,
    Archived,
}
impl CustomerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateCustomer {
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub contact_name: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub status: CustomerStatus,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub owner_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchCustomer {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub code: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub contact_name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub phone: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub email: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub note: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub status: Option<CustomerStatus>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub owner_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomerQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub status: Option<CustomerStatus>,
}
impl CustomerQuery {
    pub fn split(self) -> (PageInput, Option<CustomerStatus>) {
        (
            PageInput {
                page: self.page.unwrap_or(1),
                page_size: self.page_size.unwrap_or(20),
                search: self.search,
                sort: self.sort,
            },
            self.status,
        )
    }
}
