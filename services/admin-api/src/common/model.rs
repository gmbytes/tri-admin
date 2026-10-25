use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum DataScope {
    #[serde(rename = "all")]
    All,
    #[default]
    #[serde(rename = "self")]
    Own,
    #[serde(rename = "roles")]
    Roles,
}
impl DataScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Own => "self",
            Self::Roles => "roles",
        }
    }
}
