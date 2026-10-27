use super::{
    dto,
    models::*,
    runs,
    service::{self, policy, tx},
};
use crate::{
    common::{
        canonical,
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
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
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use zeroize::Zeroizing;
fn validate(name: &str, data: &SessionData) -> Result<(), AppError> {
    validation::text(name, "会话名称", 80)?;
    if name.chars().any(char::is_control) {
        return Err(AppError::Validation("会话名称无效".into()));
    }
    data.selection.validate()?;
    if let Some(draft) = &data.draft {
        draft
            .validate()
            .map_err(|_| AppError::Validation("会话草稿无效或超出限额".into()))?;
    }
    Ok(())
}
fn aad(row: &Session) -> String {
    let fixed = serde_json::json!([
        row.id.to_hex(),
        row.owner_id.to_hex(),
        row.name,
        row.version,
        row.payload_sha256,
        row.current_message_id.map(|id| id.to_hex()),
        row.expires_at.timestamp_millis()
    ]);
    format!(
        "workflow/session/{}",
        canonical::checksum(fixed.to_string().as_bytes())
    )
}
async fn record(
    db: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    id: ObjectId,
    active: bool,
    private: bool,
) -> mongodb::error::Result<Session> {
    let mut filter = doc! {"_id":id};
    if active {
        filter.insert("deletedAt", mongodb::bson::Bson::Null);
    }
    let row = db
        .collection::<Session>("workflow_sessions")
        .find_one(filter)
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    owned::owns(db, session, actor, row.owner_id).await?;
    if private && row.owner_id != actor.id {
        return Err(fail(Failure::NotFound));
    }
    Ok(row)
}
async fn open(
    state: &AppState,
    session: &mut ClientSession,
    row: &Session,
) -> mongodb::error::Result<SessionData> {
    if row.expires_at <= DateTime::now() {
        return Err(fail(Failure::Conflict("会话私有内容已过期")));
    }
    let payload = state
        .require_database()
        .map_err(tx)?
        .collection::<RunPayload>("workflow_session_payloads")
        .find_one(doc! {"_id":row.id,"expiresAt":{"$gt":DateTime::now()}})
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::Conflict("会话私有内容已过期")))?;
    let bytes =
        payload::open(&state.config, &aad(row), &payload.blob, payload::MAX_INPUT).map_err(tx)?;
    if canonical::checksum(&bytes) != row.payload_sha256 {
        return Err(fail(Failure::SecretStoreUnavailable));
    }
    let value: SessionData =
        serde_json::from_slice(&bytes).map_err(|_| fail(Failure::SecretStoreUnavailable))?;
    validate(&row.name, &value).map_err(|_| fail(Failure::SecretStoreUnavailable))?;
    Ok(value)
}
async fn save(
    state: &AppState,
    session: &mut ClientSession,
    row: &mut Session,
    data: &SessionData,
    insert: bool,
) -> mongodb::error::Result<()> {
    let json = Zeroizing::new(canonical::json(data).map_err(tx)?);
    row.payload_sha256 = canonical::checksum(json.as_bytes());
    let blob = payload::seal(
        &state.config,
        &aad(row),
        json.as_bytes(),
        payload::MAX_INPUT,
    )
    .map_err(tx)?;
    let db = state.require_database().map_err(tx)?;
    let payload = RunPayload {
        id: row.id,
        blob,
        expires_at: row.expires_at,
    };
    if insert {
        db.collection::<Session>("workflow_sessions")
            .insert_one(&*row)
            .session(&mut *session)
            .await?;
        db.collection::<RunPayload>("workflow_session_payloads")
            .insert_one(payload)
            .session(session)
            .await?;
    } else {
        db.collection::<Session>("workflow_sessions")
            .replace_one(doc! {"_id":row.id}, &*row)
            .session(&mut *session)
            .await?;
        db.collection::<RunPayload>("workflow_session_payloads")
            .replace_one(doc! {"_id":row.id}, payload)
            .upsert(true)
            .session(session)
            .await?;
    }
    Ok(())
}
pub async fn create(
    state: &AppState,
    context: AuthContext,
    input: dto::CreateSession,
) -> Result<SessionView, AppError> {
    dto::request_id(&input.request_id)?;
    let data = SessionData {
        selection: input.selection.clone(),
        draft: input.draft.clone(),
        current_node: None,
    };
    validate(&input.name, &data)?;
    let fingerprint = canonical::checksum(canonical::json(&input)?.as_bytes());
    let id = ObjectId::new();
    transaction::run(
        state.require_database()?,
        (state.clone(), context, input, data, fingerprint),
        |session, (state, context, input, data, fingerprint)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.write"), true).await?;
                if let Some(row) = db
                    .collection::<Session>("workflow_sessions")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&input.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if row.creation_fingerprint != *fingerprint {
                        return Err(fail(Failure::Conflict("请求 UUID 已用于不同会话")));
                    }
                    return row.view().map_err(tx);
                }
                if db
                    .collection::<Document>("workflow_sessions")
                    .count_documents(doc! {"ownerId":actor.id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    >= 100
                    || db
                        .collection::<Document>("workflow_sessions")
                        .count_documents(doc! {})
                        .session(&mut *session)
                        .await?
                        >= 100000
                {
                    return Err(fail(Failure::RateLimited));
                }
                let now = DateTime::now();
                let mut row = Session {
                    id,
                    owner_id: actor.id,
                    request_id: input.request_id.clone(),
                    creation_fingerprint: fingerprint.clone(),
                    name: input.name.clone(),
                    version: 1,
                    payload_sha256: String::new(),
                    current_message_id: None,
                    message_count: 0,
                    expires_at: DateTime::from_millis(
                        now.timestamp_millis() + 7 * 24 * 60 * 60 * 1000,
                    ),
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                save(state, session, &mut row, data, true).await?;
                shared::audit(db, &actor, "workflows.sessions.create", id, session).await?;
                row.view().map_err(tx)
            }
            .boxed()
        },
    )
    .await
}
pub async fn list(
    state: &AppState,
    context: AuthContext,
    input: PageInput,
) -> Result<Page<SessionView>, AppError> {
    let copy = PageInput {
        page: input.page,
        page_size: input.page_size,
        search: input.search.clone(),
        sort: input.sort.clone(),
    };
    let mut filter = doc! {"deletedAt":null};
    if let Some(search) = input.search.as_deref() {
        filter.insert("name", doc! {"$regex":service::prefix(search.trim())});
    }
    let (rows, total) =
        service::page::<Session>(state, context, "workflow_sessions", input, filter).await?;
    Ok(Page::new(
        rows.into_iter()
            .map(Session::view)
            .collect::<Result<_, _>>()?,
        &copy,
        total,
    ))
}
pub async fn get(
    state: &AppState,
    context: AuthContext,
    id: Option<ObjectId>,
    request: Option<String>,
) -> Result<SessionView, AppError> {
    if let Some(request) = &request {
        dto::request_id(request)?;
    }
    transaction::run_snapshot(state.require_database()?,(state.clone(),context,request),|session,(state,context,request)|async move{let db=state.require_database().map_err(tx)?;let actor=owned::fresh(db,session,context,policy("workflows.read"),false).await?;let row=if let Some(id)=id{record(db,session,&actor,id,false,false).await?}else{db.collection::<Session>("workflow_sessions").find_one(doc!{"ownerId":actor.id,"requestId":request.as_deref().ok_or_else(||fail(Failure::Validation("请求标识无效")))?}).session(session).await?.ok_or_else(||fail(Failure::NotFound))?};row.view().map_err(tx)}.boxed()).await
}
pub async fn content(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
) -> Result<SessionContent, AppError> {
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context),
        |session, (state, context)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.read"), false).await?;
                let row = record(db, session, &actor, id, true, true).await?;
                let data = open(state, session, &row).await?;
                Ok(SessionContent {
                    version: row.version,
                    selection: data.selection,
                    draft: data.draft,
                    current_message_id: row.current_message_id.map(|id| id.to_hex()),
                    current_node: data.current_node,
                })
            }
            .boxed()
        },
    )
    .await
}
async fn pointer(
    db: &Database,
    session: &mut ClientSession,
    owner: ObjectId,
    id: ObjectId,
    message: Option<ObjectId>,
    node: Option<&str>,
) -> mongodb::error::Result<()> {
    if let Some(message) = message {
        let message = db
            .collection::<Message>("workflow_messages")
            .find_one(doc! {"_id":message,"sessionId":id,"ownerId":owner})
            .session(&mut *session)
            .await?
            .ok_or_else(|| fail(Failure::NotFound))?;
        if let Some(node) = node {
            let run = db
                .collection::<Run>("workflow_runs")
                .find_one(doc! {"_id":message.run_id,"ownerId":owner})
                .session(&mut *session)
                .await?
                .ok_or_else(|| fail(Failure::NotFound))?;
            if !run.nodes.iter().any(|step| step.id == node) {
                return Err(fail(Failure::Validation("当前步骤不属于该运行")));
            }
        }
    } else if node.is_some() {
        return Err(fail(Failure::Validation("请先选择历史消息")));
    }
    Ok(())
}
pub async fn patch(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
    input: dto::PatchSession,
) -> Result<SessionView, AppError> {
    dto::version(input.version)?;
    let data = SessionData {
        selection: input.selection.clone(),
        draft: input.draft.clone(),
        current_node: input.current_node.clone(),
    };
    validate(&input.name, &data)?;
    let current = input
        .current_message_id
        .as_deref()
        .map(validation::object_id)
        .transpose()?;
    transaction::run(
        state.require_database()?,
        (state.clone(), context, input, data),
        |session, (state, context, input, data)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.write"), true).await?;
                let mut row = record(db, session, &actor, id, true, true).await?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("会话已修改，请刷新")));
                }
                if row.version >= dto::MAX_VERSION {
                    return Err(fail(Failure::Conflict("版本已达上限")));
                }
                pointer(
                    db,
                    session,
                    actor.id,
                    id,
                    current,
                    input.current_node.as_deref(),
                )
                .await?;
                row.version += 1;
                row.name = input.name.clone();
                row.current_message_id = current;
                row.updated_at = DateTime::now();
                row.expires_at = DateTime::from_millis(
                    row.updated_at.timestamp_millis() + 7 * 24 * 60 * 60 * 1000,
                );
                save(state, session, &mut row, data, false).await?;
                shared::audit(db, &actor, "workflows.sessions.update", id, session).await?;
                row.view().map_err(tx)
            }
            .boxed()
        },
    )
    .await
}
pub async fn delete(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    dto::version(version)?;
    transaction::run(
        state.require_database()?,
        (state.clone(), context),
        |session, (state, context)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.write"), true).await?;
                let row = record(db, session, &actor, id, true, true).await?;
                if row.version != version {
                    return Err(fail(Failure::Conflict("会话已修改，请刷新")));
                }
                let now = DateTime::now();
                db.collection::<Document>("workflow_sessions")
                    .update_one(
                        doc! {"_id":id},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(db, &actor, "workflows.sessions.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
pub async fn add(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
    input: dto::AddMessage,
) -> Result<MessageView, AppError> {
    dto::request_id(&input.request_id)?;
    dto::version(input.version)?;
    let run_id = validation::object_id(&input.run_id)?;
    let parent = input
        .parent_message_id
        .as_deref()
        .map(validation::object_id)
        .transpose()?;
    let hash = canonical::checksum(canonical::json(&(id.to_hex(), &input))?.as_bytes());
    let message_id = ObjectId::new();
    transaction::run(
        state.require_database()?,
        (state.clone(), context, input, hash),
        |session, (state, context, input, hash)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.write"), true).await?;
                let mut row = record(db, session, &actor, id, true, true).await?;
                if let Some(existing) = db
                    .collection::<Message>("workflow_messages")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&input.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if existing.fingerprint != *hash {
                        return Err(fail(Failure::Conflict("请求 UUID 已用于不同消息")));
                    }
                    return existing.view().map_err(tx);
                }
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("会话已修改，请刷新")));
                }
                if row.message_count >= 500 || row.version >= dto::MAX_VERSION {
                    return Err(fail(Failure::Conflict("会话消息或版本已达上限")));
                }
                pointer(db, session, actor.id, id, parent, None).await?;
                let run = db
                    .collection::<Run>("workflow_runs")
                    .find_one(doc! {"_id":run_id,"ownerId":actor.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if run.expires_at <= DateTime::now() {
                    return Err(fail(Failure::Conflict("运行私有内容已过期")));
                }
                let mut data = open(state, session, &row).await?;
                data.current_node = None;
                row.version += 1;
                row.message_count += 1;
                row.current_message_id = Some(message_id);
                row.updated_at = DateTime::now();
                row.expires_at = DateTime::from_millis(
                    row.updated_at.timestamp_millis() + 7 * 24 * 60 * 60 * 1000,
                );
                let message = Message {
                    id: message_id,
                    owner_id: actor.id,
                    session_id: id,
                    request_id: input.request_id.clone(),
                    fingerprint: hash.clone(),
                    parent_message_id: parent,
                    run_id,
                    session_version: row.version,
                    created_at: row.updated_at,
                };
                db.collection::<Message>("workflow_messages")
                    .insert_one(&message)
                    .session(&mut *session)
                    .await?;
                save(state, session, &mut row, &data, false).await?;
                shared::audit(db, &actor, "workflows.messages.create", message_id, session).await?;
                message.view().map_err(tx)
            }
            .boxed()
        },
    )
    .await
}
pub async fn messages(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
    input: PageInput,
) -> Result<Page<MessageView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt"])?;
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context, input, sort),
        |session, (state, context, input, sort)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.read"), false).await?;
                let parent = record(db, session, &actor, id, true, false).await?;
                let filter = doc! {"sessionId":id,"ownerId":parent.owner_id};
                let count = db
                    .collection::<Document>("workflow_messages")
                    .count_documents(filter.clone())
                    .session(&mut *session)
                    .await?;
                let mut cursor = db
                    .collection::<Message>("workflow_messages")
                    .find(filter)
                    .sort(sort.clone())
                    .skip(skip)
                    .limit(input.page_size as i64)
                    .session(&mut *session)
                    .await?;
                let rows = cursor
                    .stream(session)
                    .try_collect::<Vec<_>>()
                    .await?
                    .into_iter()
                    .map(Message::view)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(tx)?;
                Ok(Page::new(rows, input, count))
            }
            .boxed()
        },
    )
    .await
}
pub async fn message_request(
    state: &AppState,
    context: AuthContext,
    request: String,
) -> Result<MessageView, AppError> {
    dto::request_id(&request)?;
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context, request),
        |session, (state, context, request)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.read"), false).await?;
                let message = db
                    .collection::<Message>("workflow_messages")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&*request})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                record(db, session, &actor, message.session_id, true, true).await?;
                message.view().map_err(tx)
            }
            .boxed()
        },
    )
    .await
}
pub async fn message_content(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
) -> Result<MessageContent, AppError> {
    let message = transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context.clone()),
        |session, (state, context)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.read"), false).await?;
                let message = db
                    .collection::<Message>("workflow_messages")
                    .find_one(doc! {"_id":id,"ownerId":actor.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                record(db, session, &actor, message.session_id, true, true).await?;
                Ok(message)
            }
            .boxed()
        },
    )
    .await?;
    let run = runs::content(state, context.clone(), message.run_id).await?;
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context),
        |session, (state, context)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.read"), false).await?;
                record(db, session, &actor, message.session_id, true, true).await?;
                Ok(())
            }
            .boxed()
        },
    )
    .await?;
    Ok(MessageContent {
        message: message.view()?,
        run,
    })
}
