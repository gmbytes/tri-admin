use crate::common::{
    pagination::PageInput,
    request::{optional_non_null, optional_nullable},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;
#[derive(Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
pub enum Kind {
    #[serde(rename = "mongodb")]
    MongoDb,
    #[serde(rename = "localStorage")]
    LocalStorage,
    #[serde(rename = "s3")]
    S3,
    #[serde(rename = "smtp")]
    Smtp,
    #[serde(rename = "http")]
    Http,
}
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MongoDb => "mongodb",
            Self::LocalStorage => "localStorage",
            Self::S3 => "s3",
            Self::Smtp => "smtp",
            Self::Http => "http",
        }
    }
    pub fn storage(self) -> bool {
        matches!(self, Self::LocalStorage | Self::S3)
    }
}
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Host {
    pub host: String,
    pub port: u16,
}
#[derive(Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MailSecurity {
    Tls,
    StartTls,
    Plain,
}
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Integration {
    #[serde(rename = "mongodb", rename_all = "camelCase")]
    MongoDb {
        hosts: Vec<Host>,
        database: String,
        auth_database: String,
        replica_set: Option<String>,
        tls: bool,
        direct_connection: bool,
    },
    #[serde(rename_all = "camelCase")]
    LocalStorage { subdirectory: String },
    #[serde(rename = "s3", rename_all = "camelCase")]
    S3 {
        endpoint: String,
        bucket: String,
        region: String,
        #[serde(default)]
        prefix: String,
        #[serde(default)]
        path_style: bool,
    },
    #[serde(rename_all = "camelCase")]
    Smtp {
        host: String,
        port: u16,
        security: MailSecurity,
        from_address: String,
    },
    #[serde(rename_all = "camelCase")]
    Http { base_url: String },
}
impl Integration {
    pub fn kind(&self) -> Kind {
        match self {
            Self::MongoDb { .. } => Kind::MongoDb,
            Self::LocalStorage { .. } => Kind::LocalStorage,
            Self::S3 { .. } => Kind::S3,
            Self::Smtp { .. } => Kind::Smtp,
            Self::Http { .. } => Kind::Http,
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Credentials {
    #[serde(rename = "mongodb")]
    MongoDb {
        username: String,
        password: String,
    },
    #[serde(rename = "s3", rename_all = "camelCase")]
    S3 {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
    Smtp {
        username: String,
        password: String,
    },
    #[serde(rename_all = "camelCase")]
    Http {
        header_name: String,
        value: String,
    },
}
impl Credentials {
    pub fn kind(&self) -> Kind {
        match self {
            Self::MongoDb { .. } => Kind::MongoDb,
            Self::S3 { .. } => Kind::S3,
            Self::Smtp { .. } => Kind::Smtp,
            Self::Http { .. } => Kind::Http,
        }
    }
}
impl Drop for Credentials {
    fn drop(&mut self) {
        match self {
            Self::MongoDb { username, password } | Self::Smtp { username, password } => {
                username.zeroize();
                password.zeroize();
            }
            Self::S3 {
                access_key_id,
                secret_access_key,
                session_token,
            } => {
                access_key_id.zeroize();
                secret_access_key.zeroize();
                session_token.zeroize();
            }
            Self::Http { header_name, value } => {
                header_name.zeroize();
                value.zeroize();
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Create {
    pub name: String,
    pub code: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    pub configuration: Integration,
    pub secret: Option<Credentials>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Patch {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub configuration: Option<Integration>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub secret: Option<Option<Credentials>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServicePatch {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub display_name: Option<String>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub login_limit_per_ip_override: Option<Option<i64>>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub storage_profile_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub mail_profile_id: Option<Option<String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionInput {
    pub version: i64,
}
fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListInput {
    #[serde(default = "first")]
    pub page: u64,
    #[serde(default = "size")]
    pub page_size: u64,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub kind: Option<Kind>,
}
impl ListInput {
    pub fn page(&self) -> PageInput {
        PageInput {
            page: self.page,
            page_size: self.page_size,
            search: self.search.clone(),
            sort: self.sort.clone(),
        }
    }
}
