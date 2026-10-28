use serde::{Deserialize, Serialize};
use zeroize::Zeroize;
#[derive(Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
pub enum Kind {
    #[serde(rename = "localStorage")]
    LocalStorage,
    #[serde(rename = "s3")]
    S3,
}
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalStorage => "localStorage",
            Self::S3 => "s3",
        }
    }
    pub fn storage(self) -> bool {
        true
    }
}
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Integration {
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
}
impl Integration {
    pub fn kind(&self) -> Kind {
        match self {
            Self::LocalStorage { .. } => Kind::LocalStorage,
            Self::S3 { .. } => Kind::S3,
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Credentials {
    #[serde(rename = "s3", rename_all = "camelCase")]
    S3 {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
}
impl Credentials {
    pub fn kind(&self) -> Kind {
        Kind::S3
    }
}
impl Drop for Credentials {
    fn drop(&mut self) {
        let Self::S3 {
            access_key_id,
            secret_access_key,
            session_token,
        } = self;
        access_key_id.zeroize();
        secret_access_key.zeroize();
        session_token.zeroize();
    }
}
