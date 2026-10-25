use serde_json::{Value, json};

pub fn bundle() -> Value {
    json!({"format":"tri-admin/config-bundle","schemaVersion":1,"release":{"name":"Import","code":"import-demo","description":"","capturedAt":"2026-10-25T00:00:00.000Z"},"resources":{"menus":[{"label":"Child","path":"/import-child","parentPath":"/import-parent","viewKey":"parameters","icon":null,"permission":"parameters.read","sort":1,"enabled":true},{"label":"Parent","path":"/import-parent","parentPath":null,"viewKey":null,"icon":null,"permission":null,"sort":0,"enabled":true}],"dictionaries":[{"name":"状态","key":"import-status","description":"","enabled":true,"items":[{"label":"启用","value":"enabled","sort":1,"enabled":true}]}],"parameters":[{"name":"开关","key":"import.enabled","group":"general","description":"","valueType":"boolean","value":false}]},"requiredApis":[{"operationId":"parameters.get","method":"GET","path":"/api/v1/parameters/{id}","requiredPermissions":["parameters.read"]}]})
}
