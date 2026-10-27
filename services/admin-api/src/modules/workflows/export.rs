use super::{
    dto,
    models::*,
    runs,
    service::{policy, tx},
};
use crate::{
    common::{
        canonical,
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::{
        ai::payload,
        auth::AuthContext,
        rbac::{
            owned,
            shared::{self, Actor},
        },
    },
    state::AppState,
};
use futures_util::FutureExt;
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use tri_workflow_runtime::{Entry, Transcript};
use zeroize::Zeroizing;
const MAXIMUM: usize = 4 * 1024 * 1024;
fn aad(row: &ExportRecord) -> String {
    let fields = serde_json::json!([
        row.id.to_hex(),
        row.owner_id.to_hex(),
        row.request_id,
        row.fingerprint,
        row.run_id.to_hex(),
        row.run_version,
        row.size,
        row.sha256,
        row.file_name,
        row.expires_at.timestamp_millis()
    ]);
    format!(
        "workflow/export/{}",
        canonical::checksum(fields.to_string().as_bytes())
    )
}
async fn actor(
    db: &Database,
    session: &mut ClientSession,
    context: &AuthContext,
    write: bool,
) -> mongodb::error::Result<Actor> {
    let actor = owned::fresh(db, session, context, policy("workflows.read"), write).await?;
    if !actor.can("ai.read") || !actor.can("ai.invoke") {
        return Err(fail(Failure::Forbidden));
    }
    Ok(actor)
}
async fn replay(
    state: &AppState,
    context: AuthContext,
    request: String,
    fingerprint: String,
) -> Result<Option<ExportView>, AppError> {
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context, request, fingerprint),
        |session, (state, context, request, fingerprint)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor = actor(db, session, context, false).await?;
                if let Some(row) = db
                    .collection::<ExportRecord>("workflow_exports")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&*request})
                    .session(session)
                    .await?
                {
                    if row.fingerprint != *fingerprint {
                        return Err(fail(Failure::Conflict("请求 UUID 已用于不同导出")));
                    }
                    return Ok(Some(row.view().map_err(tx)?));
                }
                Ok(None)
            }
            .boxed()
        },
    )
    .await
}
pub async fn create(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
    input: dto::ExportInput,
) -> Result<ExportView, AppError> {
    dto::request_id(&input.request_id)?;
    dto::version(input.version)?;
    let fingerprint = canonical::checksum(canonical::json(&(id.to_hex(), &input))?.as_bytes());
    if let Some(existing) = replay(
        state,
        context.clone(),
        input.request_id.clone(),
        fingerprint.clone(),
    )
    .await?
    {
        return Ok(existing);
    }
    let content = runs::content(state, context.clone(), id).await?;
    if content.version != input.version {
        return Err(AppError::Conflict("工作流已改变，请刷新后导出".into()));
    }
    let metadata = runs::get(state, context.clone(), Some(id), None).await?;
    let mut entries = Vec::new();
    for (index, step) in content.steps.iter().enumerate() {
        let prompt = if step.prompt.is_empty() {
            format!(
                "计划提示（尚未执行）：\n{}\n需求输入：\n{}",
                content.plan.steps[index].prompt,
                serde_json::to_string(&content.inputs).map_err(|_| AppError::Internal)?
            )
        } else {
            step.prompt.clone()
        };
        entries.push(Entry {
            title: step.title.clone(),
            outcome: step.state,
            prompt,
            text: step.text.clone(),
        });
    }
    let artifact = Transcript {
        title: content.title,
        outcome: metadata.state,
        entries,
    }
    .markdown()
    .map_err(|_| AppError::PayloadTooLarge)?;
    let bytes = Zeroizing::new(artifact.bytes);
    let export_id = ObjectId::new();
    transaction::run(
        state.require_database()?,
        (
            state.clone(),
            context,
            input,
            fingerprint,
            bytes,
            artifact.sha256,
        ),
        |session, (state, context, input, fingerprint, bytes, sha256)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor = actor(db, session, context, true).await?;
                if let Some(row) = db
                    .collection::<ExportRecord>("workflow_exports")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&input.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if row.fingerprint != *fingerprint {
                        return Err(fail(Failure::Conflict("请求 UUID 已用于不同导出")));
                    }
                    return row.view().map_err(tx);
                }
                let parent = runs::record(db, session, &actor, id).await?;
                if parent.owner_id != actor.id {
                    return Err(fail(Failure::NotFound));
                }
                if parent.version != input.version {
                    return Err(fail(Failure::Conflict("工作流已改变，请刷新后导出")));
                }
                if parent.expires_at <= DateTime::now() {
                    return Err(fail(Failure::Conflict("工作流私有内容已过期")));
                }
                if db
                    .collection::<Document>("workflow_exports")
                    .count_documents(doc! {})
                    .session(&mut *session)
                    .await?
                    >= 100000
                {
                    return Err(fail(Failure::RateLimited));
                }
                let row = ExportRecord {
                    id: export_id,
                    owner_id: actor.id,
                    request_id: input.request_id.clone(),
                    fingerprint: fingerprint.clone(),
                    run_id: id,
                    run_version: input.version,
                    size: bytes.len() as i64,
                    sha256: sha256.clone(),
                    file_name: format!("workflow-{}-v{}.md", id.to_hex(), input.version),
                    expires_at: parent.expires_at,
                    created_at: DateTime::now(),
                };
                let blob = payload::seal(&state.config, &aad(&row), bytes, MAXIMUM).map_err(tx)?;
                db.collection::<ExportRecord>("workflow_exports")
                    .insert_one(&row)
                    .session(&mut *session)
                    .await?;
                db.collection::<RunPayload>("workflow_export_payloads")
                    .insert_one(RunPayload {
                        id: export_id,
                        blob,
                        expires_at: row.expires_at,
                    })
                    .session(&mut *session)
                    .await?;
                shared::audit(db, &actor, "workflows.exports.create", export_id, session).await?;
                row.view().map_err(tx)
            }
            .boxed()
        },
    )
    .await
}
pub async fn get(
    state: &AppState,
    context: AuthContext,
    id: Option<ObjectId>,
    request: Option<String>,
) -> Result<ExportView, AppError> {
    if let Some(request) = &request {
        dto::request_id(request)?;
    }
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context, request),
        |session, (state, context, request)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor = actor(db, session, context, false).await?;
                let mut filter = doc! {"ownerId":actor.id};
                if let Some(id) = id {
                    filter.insert("_id", id);
                } else {
                    filter.insert(
                        "requestId",
                        request
                            .as_deref()
                            .ok_or_else(|| fail(Failure::Validation("请求标识无效")))?,
                    );
                }
                db.collection::<ExportRecord>("workflow_exports")
                    .find_one(filter)
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?
                    .view()
                    .map_err(tx)
            }
            .boxed()
        },
    )
    .await
}
pub struct Download {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub sha256: String,
}
pub async fn download(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
) -> Result<Download, AppError> {
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context),
        |session, (state, context)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor = actor(db, session, context, false).await?;
                let row = db
                    .collection::<ExportRecord>("workflow_exports")
                    .find_one(doc! {"_id":id,"ownerId":actor.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                let parent = runs::record(db, session, &actor, row.run_id).await?;
                if parent.owner_id != actor.id {
                    return Err(fail(Failure::NotFound));
                }
                if row.expires_at <= DateTime::now() || parent.expires_at <= DateTime::now() {
                    return Err(fail(Failure::Conflict("导出已过期")));
                }
                let payload = db
                    .collection::<RunPayload>("workflow_export_payloads")
                    .find_one(doc! {"_id":id,"expiresAt":{"$gt":DateTime::now()}})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::Conflict("导出已过期")))?;
                let bytes =
                    payload::open(&state.config, &aad(&row), &payload.blob, MAXIMUM).map_err(tx)?;
                if bytes.len() as i64 != row.size || canonical::checksum(&bytes) != row.sha256 {
                    return Err(fail(Failure::SecretStoreUnavailable));
                }
                Ok(Download {
                    bytes: bytes.to_vec(),
                    file_name: row.file_name,
                    sha256: row.sha256,
                })
            }
            .boxed()
        },
    )
    .await
}
