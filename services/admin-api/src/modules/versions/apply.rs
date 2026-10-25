use super::{
    import_model::*,
    import_state, import_validation, import_write,
    models::{Bundle, Counts, time},
    preview, store,
};
use crate::{
    common::{
        canonical,
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared::{self, Actor},
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

pub async fn apply(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: ApplyInput,
) -> Result<ApplyView, AppError> {
    if input.checksum.len() != 64 || !input.checksum.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::Validation(
            "校验和需为 64 位 SHA-256 十六进制字符串".into(),
        ));
    }
    transaction::run_snapshot(database,(database.clone(),actor,input),|session,(database,actor,input)|async move {
        shared::lock(database,"versions",session).await?;
        let record = database.collection::<PreviewRecord>("version_previews").find_one(doc!{"_id":id,"actorId":actor.id}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if record.checksum != input.checksum { return Err(fail(Failure::Conflict("配置包校验和与预览不符"))); }
        if record.status == "applied" {
            return serde_json::from_str(record.result_json.as_deref().ok_or_else(||fail(Failure::NotReady))?).map_err(|_|fail(Failure::NotReady));
        }
        if record.status != "pending" || record.expires_at <= DateTime::now() { return Err(fail(Failure::Conflict("预览已过期或不可应用，请重新预览"))); }
        let bundle: Bundle = serde_json::from_str(record.bundle_json.as_deref().ok_or_else(||fail(Failure::NotReady))?).map_err(|_|fail(Failure::NotReady))?;
        let validated = import_validation::validate(actor,bundle).map_err(|error|match error {AppError::Forbidden=>fail(Failure::Forbidden),_=>fail(Failure::Conflict("配置包或目标接口能力已变化，请重新预览"))})?;
        if validated.checksum != record.checksum { return Err(fail(Failure::Conflict("配置包校验失败"))); }
        if validated.writes.iter().any(|code|!actor.can(code)) { return Err(fail(Failure::Forbidden)); }
        import_write::lock_targets(database,&validated.bundle,session).await?;
        let state = import_state::load(database,&validated.bundle,&validated.checksum,session).await?;
        if import_state::fingerprint(&state).map_err(|_|fail(Failure::NotReady))? != record.baseline_hash { return Err(fail(Failure::Conflict("目标配置已改变，请重新预览"))); }
        let plan = preview::plan(actor,&validated,&state,record.strategy).map_err(|_|fail(Failure::Conflict("导入计划已无效，请重新预览")))?;
        if !plan.can_apply { return Err(fail(Failure::Conflict("导入计划存在冲突，不能应用"))); }
        import_write::apply(database,&validated.bundle,&state,session).await?;
        let reused = plan.version_action == "reuse";
        let version_id = if reused { state.version.as_ref().ok_or_else(||fail(Failure::NotReady))?.id } else {
            let id = ObjectId::new();
            let resources = &validated.bundle.resources;
            let counts = Counts { menus: resources.menus.len() as i64, ancestors: 0, dictionaries: resources.dictionaries.len() as i64, dictionary_items: resources.dictionaries.iter().map(|row|row.items.len() as i64).sum(), parameters: resources.parameters.len() as i64, apis: validated.bundle.required_apis.len() as i64 };
            store::insert(database,actor,store::Package {id,bundle:&validated.bundle,json:&validated.json,checksum:&validated.checksum,counts:&counts,reads:&validated.reads,origin:"import"},session).await?;
            id
        };
        let now = DateTime::now();
        let result = ApplyView { preview_id:id.to_hex(),version_id:version_id.to_hex(),checksum:validated.checksum,applied_at:time(now).map_err(|_|fail(Failure::NotReady))?,counts:plan.counts,version_reused:reused };
        let json = canonical::json(&result).map_err(|_|fail(Failure::NotReady))?;
        database.collection::<Document>("version_previews").update_one(doc!{"_id":id,"status":"pending"},doc!{"$set":{"status":"applied","resultJson":json,"appliedAt":now},"$unset":{"cleanupAt":"","bundleJson":""}}).session(&mut *session).await?;
        let counts = mongodb::bson::serialize_to_bson(&result.counts).map_err(|_|fail(Failure::NotReady))?;
        database.collection::<Document>("audit_events").insert_one(doc!{"category":"administration","action":"versions.import.apply","actorId":actor.id,"targetId":version_id,"previewId":id,"checksum":&result.checksum,"counts":counts,"outcome":"success","createdAt":now}).session(session).await?;
        Ok(result)
    }.boxed()).await
}
