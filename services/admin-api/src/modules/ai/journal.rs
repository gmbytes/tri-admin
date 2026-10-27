use super::{
    binding,
    dto::{self, RunInput},
    models::*,
    payload, runs,
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::{rbac::shared, settings::models::Profile},
    state::AppState,
};
use futures_util::FutureExt;
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc},
};
use std::collections::HashSet;
use tri_ai_runtime::{ResponseState, Status};
use zeroize::Zeroizing;
#[derive(Clone)]
pub struct Claim {
    pub run: Run,
    pub token: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Generate,
    Retrieve,
    Resume,
    Cancel,
}
pub struct Prepared {
    pub row: Run,
    pub input: RunInput,
    pub response: ResponseState,
    pub endpoint: String,
    pub headers: reqwest::header::HeaderMap,
    pub operation: Operation,
}
pub fn lease(claim: &Claim) -> Document {
    doc! {"_id":claim.run.id,"generation":claim.run.generation,"leaseToken":&claim.token,"leaseExpiresAt":{"$gt":DateTime::now()},"state":"running"}
}
async fn locks(db: &Database, session: &mut ClientSession) -> mongodb::error::Result<()> {
    for key in ["rbac", "ai", "settings"] {
        shared::lock(db, key, session).await?;
    }
    Ok(())
}
async fn owner(
    db: &Database,
    session: &mut ClientSession,
    row: &Run,
) -> mongodb::error::Result<()> {
    // An accepted AI-only intent survives browser logout. Current user/roles may
    // revoke it; this grants no browser session, API token or business/MCP powers.
    let user = db
        .collection::<Document>("users")
        .find_one(doc! {"_id":row.owner_id,"status":"active","deletedAt":null})
        .projection(doc! {"roleIds":1})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Forbidden))?;
    let roles = user
        .get_array("roleIds")
        .map_err(|_| fail(Failure::NotReady))?;
    let mut cursor = db
        .collection::<Document>("roles")
        .find(doc! {"_id":{"$in":roles},"deletedAt":null})
        .projection(doc! {"permissions":1})
        .session(&mut *session)
        .await?;
    let mut permissions = HashSet::new();
    while let Some(role) = cursor.next(&mut *session).await.transpose()? {
        for permission in role
            .get_array("permissions")
            .map_err(|_| fail(Failure::NotReady))?
        {
            permissions.insert(
                permission
                    .as_str()
                    .ok_or_else(|| fail(Failure::NotReady))?
                    .to_owned(),
            );
        }
    }
    if !permissions.contains("*")
        && (!permissions.contains("ai.read") || !permissions.contains("ai.invoke"))
    {
        return Err(fail(Failure::Forbidden));
    }
    Ok(())
}
async fn current(
    state: &AppState,
    session: &mut ClientSession,
    claim: &Claim,
) -> mongodb::error::Result<Run> {
    let db = state.require_database().map_err(runs::tx)?;
    if db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"ai","version":crate::storage::ai_schema::VERSION})
        .projection(doc! {"_id":1})
        .session(&mut *session)
        .await?
        .is_none()
    {
        return Err(fail(Failure::NotReady));
    }
    let row = db
        .collection::<Run>("ai_runs")
        .find_one(lease(claim))
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Conflict("LEASE_LOST")))?;
    if runs::aad(&row, "state") != runs::aad(&claim.run, "state") {
        return Err(fail(Failure::SecretStoreUnavailable));
    }
    if row.execution_deadline <= DateTime::now() || row.expires_at <= DateTime::now() {
        return Err(fail(Failure::Conflict("RUN_EXPIRED")));
    }
    owner(db, session, &row).await?;
    let provider = db
        .collection::<Provider>("ai_providers")
        .find_one(doc! {"_id":row.provider_id,"deletedAt":null,"enabled":true})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Conflict("BINDING_CHANGED")))?;
    if provider.version != row.provider_version
        || provider.profile_id != row.profile_id
        || provider.profile_version != row.profile_version
        || provider.binding_fingerprint != row.binding_fingerprint
        || !provider
            .models
            .iter()
            .any(|model| model.id == row.model && model.max_output_tokens >= row.max_output_tokens)
        || !binding::current(db, session, &state.config, &provider).await?
    {
        return Err(fail(Failure::Conflict("BINDING_CHANGED")));
    }
    Ok(row)
}
async fn audit(
    db: &Database,
    session: &mut ClientSession,
    row: &Run,
    action: &str,
    outcome: &str,
) -> mongodb::error::Result<()> {
    db.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"administration","action":action,"actorId":row.owner_id,"targetId":row.id,"outcome":outcome,"createdAt":DateTime::now()})).session(session).await?;
    Ok(())
}
pub async fn claim(state: &AppState) -> Result<Option<Claim>, AppError> {
    let token = uuid::Uuid::new_v4().to_string();
    transaction::run(state.require_database()?,(state.clone(),token),|session,(state,token)|async move{
        let db=state.require_database().map_err(runs::tx)?;shared::lock(db,"ai",session).await?;let now=DateTime::now();
        if db.collection::<Document>("ai_runs").count_documents(doc!{"leaseExpiresAt":{"$gt":now}}).session(&mut *session).await?>=2{return Ok(None);}
        let filter=doc!{"state":{"$in":["pending","running","unknown"]},"nextRunAt":{"$lte":now},"executionDeadline":{"$gt":now},"expiresAt":{"$gt":now},"$and":[{"$or":[{"leaseExpiresAt":null},{"leaseExpiresAt":{"$lte":now}}]},{"$or":[{"state":{"$in":["pending","running"]}},{"hasResponse":true}]}]};
        let Some(mut row)=db.collection::<Run>("ai_runs").find_one(filter).sort(doc!{"createdAt":1,"_id":1}).session(&mut *session).await? else{return Ok(None);};
        if row.version>=dto::MAX_VERSION||row.generation>=dto::MAX_VERSION{return Err(fail(Failure::NotReady));}
        row.state=RunState::Running;row.generation+=1;row.version+=1;row.updated_at=now;row.lease_token=Some(token.clone());row.lease_expires_at=Some(DateTime::from_millis(now.timestamp_millis()+15000));
        db.collection::<Run>("ai_runs").replace_one(doc!{"_id":row.id},&row).session(session).await?;Ok(Some(Claim{run:row,token:token.clone()}))
    }.boxed()).await
}
pub async fn expire(state: &AppState) -> Result<u32, AppError> {
    let db = state.require_database()?;
    let now = DateTime::now();
    let filter = doc! {"state":{"$in":["pending","running","unknown"]},"finishedAt":null,"$and":[{"$or":[{"executionDeadline":{"$lte":now}},{"expiresAt":{"$lte":now}}]},{"$or":[{"leaseExpiresAt":null},{"leaseExpiresAt":{"$lte":now}}]}]};
    if db
        .collection::<Document>("ai_runs")
        .find_one(filter.clone())
        .projection(doc! {"_id":1})
        .await?
        .is_none()
    {
        return Ok(0);
    }
    transaction::run(db, (state.clone(), filter), |session, (state, filter)| {
        async move {
            let db = state.require_database().map_err(runs::tx)?;
            shared::lock(db, "ai", session).await?;
            let mut cursor = db
                .collection::<Run>("ai_runs")
                .find(filter.clone())
                .limit(20)
                .session(&mut *session)
                .await?;
            let mut rows = Vec::new();
            while let Some(row) = cursor.next(&mut *session).await.transpose()? {
                rows.push(row);
            }
            let count = rows.len() as u32;
            for mut row in rows {
                if row.version >= dto::MAX_VERSION {
                    return Err(fail(Failure::NotReady));
                }
                row.state = if row.submission_started {
                    RunState::Unknown
                } else {
                    RunState::Failed
                };
                row.error_code = Some("RUN_EXPIRED".into());
                row.finished_at = Some(DateTime::now());
                row.updated_at = DateTime::now();
                row.version += 1;
                row.lease_token = None;
                row.lease_expires_at = None;
                db.collection::<Run>("ai_runs")
                    .replace_one(doc! {"_id":row.id}, &row)
                    .session(&mut *session)
                    .await?;
                audit(db, session, &row, "ai.runs.expire", "failed").await?;
            }
            Ok(count)
        }
        .boxed()
    })
    .await
}
pub async fn prepare(state: &AppState, claim: &Claim) -> Result<Prepared, AppError> {
    transaction::run(
        state.require_database()?,
        (state.clone(), claim.clone()),
        |session, (state, claim)| {
            async move {
                let db = state.require_database().map_err(runs::tx)?;
                locks(db, session).await?;
                let row = current(state, session, claim).await?;
                let input = runs::read_input(db, session, &state.config, &row).await?;
                let response = runs::read_state(db, session, &state.config, &row).await?;
                let operation = if !row.submission_started {
                    Operation::Generate
                } else if response.response_id().is_none() {
                    return Err(fail(Failure::Conflict("UNKNOWN_SUBMISSION")));
                } else if row.cancel_requested && !row.cancel_sent {
                    Operation::Cancel
                } else if row.stream && !row.cancel_sent {
                    Operation::Resume
                } else {
                    Operation::Retrieve
                };
                let profile = db
                    .collection::<Profile>("integration_profiles")
                    .find_one(doc! {"_id":row.profile_id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::Conflict("BINDING_CHANGED")))?;
                let (endpoint, headers) = binding::definition(&state.config, &profile)
                    .map_err(binding::transaction_error)?;
                Ok(Prepared {
                    row,
                    input,
                    response,
                    endpoint,
                    headers,
                    operation,
                })
            }
            .boxed()
        },
    )
    .await
}
pub async fn mark(state: &AppState, claim: &Claim, operation: Operation) -> Result<(), AppError> {
    transaction::run(state.require_database()?,(state.clone(),claim.clone()),|session,(state,claim)|async move{
        let db=state.require_database().map_err(runs::tx)?;locks(db,session).await?;let row=current(state,session,claim).await?;
        if operation!=Operation::Cancel&&row.cancel_requested&&!row.cancel_sent{return Err(fail(Failure::Conflict("CANCEL_REQUESTED")));}
        if operation==Operation::Generate{if row.submission_started{return Err(fail(Failure::Conflict("UNKNOWN_SUBMISSION")));}db.collection::<Document>("ai_runs").update_one(lease(claim),doc!{"$set":{"submissionStarted":true,"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;audit(db,session,&row,"ai.runs.submit","started").await?;}
        if operation==Operation::Cancel{if row.cancel_sent{return Err(fail(Failure::Conflict("CANCEL_ALREADY_SENT")));}db.collection::<Document>("ai_runs").update_one(lease(claim),doc!{"$set":{"cancelSent":true,"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;audit(db,session,&row,"ai.runs.cancel.submit","started").await?;}
        Ok(())
    }.boxed()).await
}
pub async fn renew(state: &AppState, claim: &Claim) -> Result<(), AppError> {
    transaction::run(state.require_database()?,(state.clone(),claim.clone()),|session,(state,claim)|async move{
        let db=state.require_database().map_err(runs::tx)?;locks(db,session).await?;let row=current(state,session,claim).await?;if row.cancel_requested&&!row.cancel_sent{return Err(fail(Failure::Conflict("CANCEL_REQUESTED")));}
        db.collection::<Document>("ai_runs").update_one(lease(claim),doc!{"$set":{"leaseExpiresAt":DateTime::from_millis(DateTime::now().timestamp_millis()+15000)}}).session(session).await?;Ok(())
    }.boxed()).await
}
pub async fn persist(
    state: &AppState,
    claim: &Claim,
    response: &ResponseState,
) -> Result<(), AppError> {
    let checkpoint = Zeroizing::new(response.checkpoint().map_err(|_| AppError::Internal)?);
    let blob = payload::seal(
        &state.config,
        &runs::aad(&claim.run, "state"),
        &checkpoint,
        payload::MAX_STATE,
    )?;
    let status = response.status();
    let refused = response.refused();
    let empty = response.text().is_empty();
    let has_response = response.response_id().is_some();
    transaction::run(
        state.require_database()?,
        (state.clone(), claim.clone(), blob),
        |session, (state, claim, blob)| {
            async move {
                let db = state.require_database().map_err(runs::tx)?;
                locks(db, session).await?;
                let mut row = current(state, session, claim).await?;
                if row.version >= dto::MAX_VERSION {
                    return Err(fail(Failure::Conflict("版本已达上限")));
                }
                row.has_response = has_response;
                row.response_status = status;
                row.updated_at = DateTime::now();
                row.version += 1;
                if let Some(status) = status {
                    let (state, error) = match status {
                        Status::Completed if refused => (RunState::Failed, Some("MODEL_REFUSED")),
                        Status::Completed if empty => (RunState::Failed, Some("EMPTY_OUTPUT")),
                        Status::Completed => (RunState::Succeeded, None),
                        Status::Failed => (RunState::Failed, Some("UPSTREAM_FAILED")),
                        Status::Incomplete => (RunState::Failed, Some("OUTPUT_INCOMPLETE")),
                        Status::Cancelled => (RunState::Cancelled, None),
                    };
                    row.state = state;
                    row.error_code = error.map(String::from);
                    row.finished_at = Some(row.updated_at);
                    row.lease_token = None;
                    row.lease_expires_at = None;
                    audit(
                        db,
                        session,
                        &row,
                        "ai.runs.finish",
                        if state == RunState::Succeeded {
                            "success"
                        } else {
                            "failed"
                        },
                    )
                    .await?;
                }
                let result = db
                    .collection::<PrivateRecord>("ai_states")
                    .replace_one(
                        doc! {"_id":row.id},
                        PrivateRecord {
                            id: row.id,
                            blob: blob.clone(),
                            expires_at: row.expires_at,
                        },
                    )
                    .session(&mut *session)
                    .await?;
                if result.matched_count != 1 {
                    return Err(fail(Failure::SecretStoreUnavailable));
                }
                db.collection::<Run>("ai_runs")
                    .replace_one(doc! {"_id":row.id}, &row)
                    .session(session)
                    .await?;
                Ok(())
            }
            .boxed()
        },
    )
    .await
}
pub async fn release(
    state: &AppState,
    claim: &Claim,
    error: Option<&str>,
    definitely_unsent: bool,
) -> Result<(), AppError> {
    transaction::run(
        state.require_database()?,
        (state.clone(), claim.clone(), error.map(String::from)),
        |session, (state, claim, error)| {
            async move {
                let db = state.require_database().map_err(runs::tx)?;
                shared::lock(db, "ai", session).await?;
                let Some(mut row) = db
                    .collection::<Run>("ai_runs")
                    .find_one(lease(claim))
                    .session(&mut *session)
                    .await?
                else {
                    return Ok(());
                };
                let now = DateTime::now();
                if definitely_unsent && !claim.run.submission_started {
                    row.submission_started = false;
                }
                if let Some(error) = error {
                    row.error_code = Some(error.clone());
                    row.state = if row.submission_started {
                        RunState::Unknown
                    } else if row.cancel_requested || error == "CANCELLED" {
                        RunState::Cancelled
                    } else {
                        RunState::Failed
                    };
                    if matches!(row.state, RunState::Failed | RunState::Cancelled)
                        || !row.has_response
                    {
                        row.finished_at = Some(now);
                    }
                    audit(db, session, &row, "ai.runs.interrupted", "failed").await?;
                }
                row.lease_token = None;
                row.lease_expires_at = None;
                row.updated_at = now;
                row.version += 1;
                row.next_run_at = DateTime::from_millis(
                    now.timestamp_millis() + if error.is_some() { 1000 } else { 1500 },
                );
                db.collection::<Run>("ai_runs")
                    .replace_one(doc! {"_id":row.id}, row)
                    .session(session)
                    .await?;
                Ok(())
            }
            .boxed()
        },
    )
    .await
}
