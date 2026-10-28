//! Direct isolated fixture setup. Configuration-center HTTP routes remain absent.
use admin_api::{
    common::{canonical, vault},
    config::Config,
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
pub async fn seed(app: &crate::support::TestApp, input: Value) -> Value {
    let id = ObjectId::new();
    let configuration = canonical::json(&input["configuration"]).unwrap();
    let kind = input["configuration"]["kind"].as_str().unwrap();
    let secret = if input["secret"].is_null() {
        mongodb::bson::Bson::Null
    } else {
        let config = Config::from_lookup(|key| match key {
            "TRI_JWT_SIGNING_KEY" => Some(crate::support::KEY.into()),
            "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
            "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
            _ => None,
        })
        .unwrap();
        let aad = format!(
            "tri-admin/settings/profile/{}/{}/{}",
            id.to_hex(),
            kind,
            canonical::checksum(configuration.as_bytes())
        );
        let plaintext = zeroize::Zeroizing::new(canonical::json(&input["secret"]).unwrap());
        mongodb::bson::serialize_to_bson(
            &vault::seal(&config, aad.as_bytes(), plaintext.as_bytes()).unwrap(),
        )
        .unwrap()
    };
    let now = DateTime::now();
    let name = input["name"].as_str().unwrap();
    let code = input["code"].as_str().unwrap();
    app.database.collection::<Document>("integration_profiles").insert_one(doc!{"_id":id,"name":name,"code":code,"kind":kind,"configurationJson":configuration,"secretBox":secret,"enabled":input["enabled"].as_bool().unwrap_or(true),"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).await.unwrap();
    json!({"id":id.to_hex(),"name":name,"code":code,"version":1})
}
