use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureInput {
    pub name: String,
    pub code: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub menu_ids: Vec<String>,
    #[serde(default)]
    pub dictionary_ids: Vec<String>,
    #[serde(default)]
    pub parameter_ids: Vec<String>,
    #[serde(default)]
    pub api_operation_ids: Vec<String>,
}
