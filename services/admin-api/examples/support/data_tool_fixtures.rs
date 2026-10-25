use admin_api::{
    common::model::DataScope,
    modules::{
        data_tools::{imports, models::PreviewInput, retention},
        rbac::shared::Actor,
        users::{dto::CreateUser, service as users},
    },
    state::AppState,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::json;
pub async fn seed(state: &AppState, password: &str) -> Result<(), Box<dyn std::error::Error>> {
    let database = state.require_database()?;
    if !database.name().starts_with("tri_admin_e2e_") {
        return Err("retention UI fixtures require an isolated E2E database".into());
    }
    let owner = database
        .collection::<Document>("users")
        .find_one(doc! {"usernameKey":"admin"})
        .await?
        .ok_or("missing fixture administrator")?
        .get_object_id("_id")?;
    let role = database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"owner"})
        .await?
        .ok_or("missing fixture role")?
        .get_object_id("_id")?;
    let actor = |id| Actor {
        id,
        permissions: vec!["*".into()],
        scope: DataScope::All,
        scope_role_ids: vec![],
        role_ids: vec![role],
    };
    let input: CreateUser = serde_json::from_value(
        json!({"username":"retained-history-fixture","displayName":"过期导入验证","password":password,"roleIds":[role.to_hex()]}),
    )?;
    let user = users::create(database, actor(owner), input).await?;
    let id = ObjectId::parse_str(&user.id)?;
    let input: PreviewInput = serde_json::from_value(
        json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"csv","contentBase64":STANDARD.encode("name,key,valueType,value\nRetained,fixture.retained.first,string,PRIVATE_RETAINED_SENTINEL\nExpired,fixture.retained.second,string,PRIVATE_RETAINED_SENTINEL\n")}),
    )?;
    let (fingerprint, rows) = imports::prepare(&input)?;
    let preview = imports::create(database, actor(id), input, fingerprint, rows).await?;
    let job = ObjectId::parse_str(&preview.summary.id)?;
    imports::apply(
        database,
        actor(id),
        job,
        serde_json::from_value(json!({"rows":[0]}))?,
    )
    .await?;
    let now = DateTime::now().timestamp_millis();
    database.collection::<Document>("data_imports").update_one(doc!{"_id":job},doc!{"$set":{"createdAt":DateTime::from_millis(now-2*86_400_000),"expiresAt":DateTime::from_millis(now-86_400_000)}}).await?;
    retention::maintenance_once(database).await?;
    Ok(())
}
