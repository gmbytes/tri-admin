use super::{
    binding,
    dto::{self, Cancel, RunInput},
    models::*,
    payload,
    service::policy,
};
use crate::{
    common::{
        canonical,
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::{
        auth::AuthContext,
        rbac::{owned, shared},
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{self, DateTime, Document, doc, oid::ObjectId},
};
use zeroize::Zeroizing;
pub fn aad(row: &Run, kind: &str) -> String {
    let mut fixed = serde_json::json!({"input":row.fingerprint,"provider":row.provider_id.to_hex(),"providerVersion":row.provider_version,"profile":row.profile_id.to_hex(),"profileVersion":row.profile_version,"binding":row.binding_fingerprint,"model":row.model,"tokens":row.max_output_tokens,"stream":row.stream,"deadline":row.execution_deadline.timestamp_millis(),"expires":row.expires_at.timestamp_millis()});
    if let Some(origin) = &row.workflow_origin {
        fixed["workflowOrigin"] = serde_json::json!(origin);
    }
    format!(
        "{}/{}/{}/{kind}",
        row.id.to_hex(),
        row.owner_id.to_hex(),
        canonical::checksum(fixed.to_string().as_bytes())
    )
}
pub fn tx(error: AppError) -> mongodb::error::Error {
    match error {
        AppError::SecretStoreUnavailable => fail(Failure::SecretStoreUnavailable),
        AppError::PayloadTooLarge => fail(Failure::PayloadTooLarge),
        _ => fail(Failure::NotReady),
    }
}
pub async fn record(
    db: &Database,
    session: &mut ClientSession,
    actor: &shared::Actor,
    id: ObjectId,
) -> mongodb::error::Result<Run> {
    let row = db
        .collection::<Run>("ai_runs")
        .find_one(doc! {"_id":id})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    owned::owns(db, session, actor, row.owner_id).await?;
    Ok(row)
}
pub async fn create(
    state: &AppState,
    context: AuthContext,
    input: RunInput,
) -> Result<RunView, AppError> {
    input.validate()?;
    let id = ObjectId::new();
    transaction::run(
        state.require_database()?,
        (state.clone(), context, input),
        |session, (state, context, input)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor = owned::fresh(db, session, context, policy("ai.invoke"), true).await?;
                accept_in(state, session, &actor, input, id, None).await
            }
            .boxed()
        },
    )
    .await
}
// This helper is private to the AI module. Workflow callers provide an opaque,
// verified parent/step grant, never an AuthContext or arbitrary Actor.
async fn accept_in(
    state: &AppState,
    session: &mut ClientSession,
    actor: &shared::Actor,
    input: &RunInput,
    id: ObjectId,
    origin: Option<WorkflowOrigin>,
) -> mongodb::error::Result<RunView> {
    input
        .validate()
        .map_err(|_| fail(Failure::Validation("AI 运行输入无效")))?;
    let json = Zeroizing::new(canonical::json(input).map_err(tx)?);
    let fingerprint = canonical::checksum(json.as_bytes());
    let db = state.require_database().map_err(tx)?;
    if let Some(row) = db
        .collection::<Run>("ai_runs")
        .find_one(doc! {"ownerId":actor.id,"requestId":&input.request_id})
        .session(&mut *session)
        .await?
    {
        if row.fingerprint != fingerprint || row.workflow_origin != origin {
            return Err(fail(Failure::Conflict("请求 UUID 已用于不同 AI 意图")));
        }
        return row.view().map_err(tx);
    }
    shared::lock(db, "settings", session).await?;
    let provider=db.collection::<Provider>("ai_providers").find_one(doc!{"_id":validation::object_id(&input.provider_id).map_err(tx)?,"deletedAt":null,"enabled":true}).session(&mut *session).await?.ok_or_else(||fail(Failure::Validation("批准的 AI 上游不可用")))?;
    if provider.version != input.provider_version
        || !binding::current(db, session, &state.config, &provider).await?
    {
        return Err(fail(Failure::Conflict("AI 上游或 HTTP 配置版本已改变")));
    }
    if !provider
        .models
        .iter()
        .any(|model| model.id == input.model && input.max_output_tokens <= model.max_output_tokens)
    {
        return Err(fail(Failure::Validation("模型或 Token 上限未获批准")));
    }
    if db
        .collection::<Document>("ai_runs")
        .count_documents(doc! {"ownerId":actor.id,"state":{"$in":["pending","running"]}})
        .session(&mut *session)
        .await?
        >= 20
        || db
            .collection::<Document>("ai_runs")
            .count_documents(doc! {"state":{"$in":["pending","running"]}})
            .session(&mut *session)
            .await?
            >= 100
        || db
            .collection::<Document>("ai_runs")
            .count_documents(doc! {})
            .session(&mut *session)
            .await?
            >= 100000
    {
        return Err(fail(Failure::RateLimited));
    }
    let now = DateTime::now();
    let expires = DateTime::from_millis(now.timestamp_millis() + 7 * 24 * 60 * 60 * 1000);
    let row = Run {
        id,
        owner_id: actor.id,
        request_id: input.request_id.clone(),
        fingerprint,
        provider_id: provider.id,
        provider_version: provider.version,
        profile_id: provider.profile_id,
        profile_version: provider.profile_version,
        binding_fingerprint: provider.binding_fingerprint,
        model: input.model.clone(),
        max_output_tokens: input.max_output_tokens,
        stream: input.stream,
        workflow_origin: origin,
        state: RunState::Pending,
        submission_started: false,
        has_response: false,
        cancel_requested: false,
        cancel_request_id: None,
        cancel_fingerprint: None,
        cancel_sent: false,
        response_status: None,
        error_code: None,
        version: 1,
        generation: 0,
        lease_token: None,
        lease_expires_at: None,
        next_run_at: now,
        execution_deadline: DateTime::from_millis(now.timestamp_millis() + 15 * 60 * 1000),
        expires_at: expires,
        created_at: now,
        updated_at: now,
        finished_at: None,
    };
    let input_blob = payload::seal(
        &state.config,
        &aad(&row, "input"),
        json.as_bytes(),
        payload::MAX_INPUT,
    )
    .map_err(tx)?;
    let checkpoint = Zeroizing::new(
        tri_ai_runtime::ResponseState::new(tri_ai_runtime::Limits::default())
            .map_err(|_| fail(Failure::NotReady))?
            .checkpoint()
            .map_err(|_| fail(Failure::NotReady))?,
    );
    let state_blob = payload::seal(
        &state.config,
        &aad(&row, "state"),
        &checkpoint,
        payload::MAX_STATE,
    )
    .map_err(tx)?;
    db.collection::<Run>("ai_runs")
        .insert_one(&row)
        .session(&mut *session)
        .await?;
    for (name, blob) in [("ai_inputs", input_blob), ("ai_states", state_blob)] {
        db.collection::<PrivateRecord>(name)
            .insert_one(PrivateRecord {
                id,
                blob,
                expires_at: expires,
            })
            .session(&mut *session)
            .await?;
    }
    shared::audit(db, actor, "ai.runs.create", id, session).await?;
    row.view().map_err(tx)
}
pub(crate) async fn from_workflow(
    state: &AppState,
    session: &mut ClientSession,
    grant: &crate::modules::workflows::runs::Grant,
) -> mongodb::error::Result<RunView> {
    let (actor, input, id, origin) = grant.input(state, session).await?;
    accept_in(state, session, &actor, &input, id, Some(origin)).await
}
pub async fn get(
    state: &AppState,
    context: AuthContext,
    id: Option<ObjectId>,
    request: Option<String>,
) -> Result<RunView, AppError> {
    if let Some(request) = &request {
        dto::request_id(request)?;
    }
    transaction::run_snapshot(state.require_database()?,(state.clone(),context,request),|session,(state,context,request)|async move{
        let db=state.require_database().map_err(|_|fail(Failure::NotReady))?;let actor=owned::fresh(db,session,context,policy("ai.read"),false).await?;
        let row=if let Some(id)=id{record(db,session,&actor,id).await?}else{db.collection::<Run>("ai_runs").find_one(doc!{"ownerId":actor.id,"requestId":request.as_deref().ok_or_else(||fail(Failure::Validation("请求标识无效")))?}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?};row.view().map_err(tx)
    }.boxed()).await
}
pub async fn content(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
) -> Result<Content, AppError> {
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context),
        |session, (state, context)| {
            async move {
                let db = state
                    .require_database()
                    .map_err(|_| fail(Failure::NotReady))?;
                let actor = owned::fresh(db, session, context, policy("ai.invoke"), false).await?;
                if !actor.can("ai.invoke") {
                    return Err(fail(Failure::Forbidden));
                }
                let row = record(db, session, &actor, id).await?;
                if row.owner_id != actor.id {
                    return Err(fail(Failure::NotFound));
                }
                if row.workflow_origin.is_some() && !actor.can("workflows.read") {
                    return Err(fail(Failure::Forbidden));
                }
                if row.expires_at <= DateTime::now() {
                    return Err(fail(Failure::Conflict("AI 私有内容已过期")));
                }
                let input = read_input(db, session, &state.config, &row).await?;
                let response = read_state(db, session, &state.config, &row).await?;
                let text = response.text();
                let sha256 = canonical::checksum(text.as_bytes());
                Ok(Content {
                    input: input.input,
                    instructions: input.instructions,
                    text,
                    sha256,
                    response_status: response.status(),
                    refused: response.refused(),
                    sequence: response.sequence().map(|value| value.to_string()),
                })
            }
            .boxed()
        },
    )
    .await
}
pub async fn read_input(
    db: &Database,
    session: &mut ClientSession,
    config: &crate::config::Config,
    row: &Run,
) -> mongodb::error::Result<RunInput> {
    let bytes = private(
        db,
        session,
        config,
        row,
        "ai_inputs",
        "input",
        payload::MAX_INPUT,
    )
    .await?;
    let input: RunInput =
        serde_json::from_slice(&bytes).map_err(|_| fail(Failure::SecretStoreUnavailable))?;
    input
        .validate()
        .map_err(|_| fail(Failure::SecretStoreUnavailable))?;
    if canonical::checksum(canonical::json(&input).map_err(tx)?.as_bytes()) != row.fingerprint
        || input.request_id != row.request_id
        || input.provider_id != row.provider_id.to_hex()
        || input.provider_version != row.provider_version
        || input.model != row.model
        || input.max_output_tokens != row.max_output_tokens
        || input.stream != row.stream
    {
        return Err(fail(Failure::SecretStoreUnavailable));
    }
    Ok(input)
}
pub async fn read_state(
    db: &Database,
    session: &mut ClientSession,
    config: &crate::config::Config,
    row: &Run,
) -> mongodb::error::Result<tri_ai_runtime::ResponseState> {
    let bytes = private(
        db,
        session,
        config,
        row,
        "ai_states",
        "state",
        payload::MAX_STATE,
    )
    .await?;
    tri_ai_runtime::ResponseState::restore(&bytes, tri_ai_runtime::Limits::default())
        .map_err(|_| fail(Failure::SecretStoreUnavailable))
}
async fn private(
    db: &Database,
    session: &mut ClientSession,
    config: &crate::config::Config,
    row: &Run,
    collection: &str,
    kind: &str,
    maximum: usize,
) -> mongodb::error::Result<Zeroizing<Vec<u8>>> {
    let record = db
        .collection::<PrivateRecord>(collection)
        .find_one(doc! {"_id":row.id,"expiresAt":{"$gt":DateTime::now()}})
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::Conflict("AI 私有内容已过期")))?;
    payload::open(config, &aad(row, kind), &record.blob, maximum).map_err(tx)
}
pub async fn cancel(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
    input: Cancel,
) -> Result<RunView, AppError> {
    dto::request_id(&input.request_id)?;
    dto::version(input.version)?;
    transaction::run(
        state.require_database()?,
        (state.clone(), context, input),
        |session, (state, context, input)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor = owned::fresh(db, session, context, policy("ai.invoke"), true).await?;
                cancel_in(state, session, &actor, id, input).await
            }
            .boxed()
        },
    )
    .await
}
async fn cancel_in(
    state: &AppState,
    session: &mut ClientSession,
    actor: &shared::Actor,
    id: ObjectId,
    input: &Cancel,
) -> mongodb::error::Result<RunView> {
    let db = state.require_database().map_err(tx)?;
    let fingerprint = canonical::checksum(
        canonical::json(&(id.to_hex(), input))
            .map_err(tx)?
            .as_bytes(),
    );
    let mut row = record(db, session, actor, id).await?;
    if row.owner_id != actor.id {
        return Err(fail(Failure::NotFound));
    }
    if row.workflow_origin.is_some()
        && (!actor.can("workflows.read") || !actor.can("workflows.invoke"))
    {
        return Err(fail(Failure::Forbidden));
    }
    if row.cancel_requested {
        if row.cancel_request_id.as_deref() != Some(&input.request_id)
            || row.cancel_fingerprint.as_deref() != Some(&fingerprint)
        {
            return Err(fail(Failure::Conflict("该运行已有其他取消请求")));
        }
        return row.view().map_err(tx);
    }
    if row.version != input.version {
        return Err(fail(Failure::Conflict("AI 运行状态已改变，请刷新")));
    }
    if row.version >= dto::MAX_VERSION {
        return Err(fail(Failure::Conflict("版本已达上限")));
    }
    row.cancel_requested = true;
    row.cancel_request_id = Some(input.request_id.clone());
    row.cancel_fingerprint = Some(fingerprint);
    row.version += 1;
    row.updated_at = DateTime::now();
    row.next_run_at = row.updated_at;
    if !row.submission_started {
        row.state = RunState::Cancelled;
        row.finished_at = Some(row.updated_at);
        row.lease_token = None;
        row.lease_expires_at = None;
    }
    db.collection::<Run>("ai_runs")
        .replace_one(doc! {"_id":id}, &row)
        .session(&mut *session)
        .await?;
    shared::audit(db, actor, "ai.runs.cancel", id, session).await?;
    row.view().map_err(tx)
}
pub(crate) async fn cancel_from_workflow(
    state: &AppState,
    session: &mut ClientSession,
    grant: &crate::modules::workflows::runs::Grant,
) -> mongodb::error::Result<()> {
    if let Some((actor, row, input)) = grant.cancellation(state, session).await?
        && !row.cancel_requested
        && matches!(
            row.state,
            RunState::Pending | RunState::Running | RunState::Unknown
        )
    {
        cancel_in(state, session, &actor, row.id, &input).await?;
    }
    Ok(())
}
pub async fn list(
    state: &AppState,
    context: AuthContext,
    input: PageInput,
) -> Result<Page<RunView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt"])?;
    transaction::run_snapshot(state.require_database()?,(state.clone(),context,input,sort),|session,(state,context,input,sort)|async move{
        let db=state.require_database().map_err(|_|fail(Failure::NotReady))?;let actor=owned::fresh(db,session,context,policy("ai.read"),false).await?;let mut filter=doc!{};
        if actor.scope==DataScope::Own{filter.insert("ownerId",actor.id);}
if let Some(search)=input.search.as_deref(){let mut escaped=String::from("^");for c in search.trim().chars(){if ".+*?^${}()|[]\\".contains(c){escaped.push('\\');}escaped.push(c);}filter.insert("model",doc!{"$regex":escaped});}
        let mut pipeline=vec![doc!{"$match":filter}];if actor.scope==DataScope::Roles{pipeline.push(doc!{"$lookup":{"from":"users","localField":"ownerId","foreignField":"_id","pipeline":[{"$match":{"deletedAt":null}},{"$project":{"roleIds":1}}],"as":"__scopeOwner"}});pipeline.push(doc!{"$match":{"$or":[{"ownerId":actor.id},{"__scopeOwner.roleIds":{"$in":&actor.scope_role_ids}}]}});}
        pipeline.push(doc!{"$sort":sort.clone()});pipeline.push(doc!{"$facet":{"rows":[{"$skip":skip as i64},{"$limit":input.page_size as i64},{"$project":{"__scopeOwner":0}}],"count":[{"$count":"total"}]}});
        let mut cursor=db.collection::<Document>("ai_runs").aggregate(pipeline).max_time(std::time::Duration::from_secs(3)).session(&mut *session).await?;let result=cursor.stream(session).try_next().await?.ok_or_else(||fail(Failure::NotReady))?;
        let data=result.get_array("rows").map_err(|_|fail(Failure::NotReady))?.iter().map(|value|bson::deserialize_from_bson::<Run>(value.clone()).map_err(|_|fail(Failure::NotReady))?.view().map_err(tx)).collect::<mongodb::error::Result<Vec<_>>>()?;
        let count=result.get_array("count").map_err(|_|fail(Failure::NotReady))?.first().and_then(|value|value.as_document()).map(|value|value.get_i64("total").or_else(|_|value.get_i32("total").map(i64::from))).transpose().map_err(|_|fail(Failure::NotReady))?.unwrap_or(0);
        Ok(Page::new(data,input,u64::try_from(count).map_err(|_|fail(Failure::NotReady))?))
    }.boxed()).await
}
