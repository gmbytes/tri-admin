use super::{
    dto::{self, CreatePlan, PatchPlan},
    models::*,
};
use crate::{
    common::{
        canonical,
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
    },
    modules::{
        ai::payload,
        auth::AuthContext,
        rbac::{
            owned::{self, Resource},
            shared::{self, Actor},
        },
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{self, DateTime, Document, doc, oid::ObjectId},
};
use serde::de::DeserializeOwned;
use tri_workflow_runtime::{Mode, Plan};
use zeroize::Zeroizing;
pub fn policy(permission: &'static str) -> Resource {
    Resource {
        schema: "workflows",
        version: crate::storage::workflow_schema::VERSION,
        read: "workflows.read",
        write: permission,
    }
}
pub fn tx(error: AppError) -> mongodb::error::Error {
    match error {
        AppError::SecretStoreUnavailable => fail(Failure::SecretStoreUnavailable),
        AppError::PayloadTooLarge => fail(Failure::PayloadTooLarge),
        _ => fail(Failure::NotReady),
    }
}
pub fn templates() -> Vec<Template> {
    [
        ("project", "新 Rust/MongoDB/Vue 工程", Mode::Project),
        ("feature", "完善功能", Mode::Feature),
        ("mcpGuide", "MCP 开发指导", Mode::McpGuide),
    ]
    .into_iter()
    .map(|(id, title, mode)| Template {
        id,
        title,
        plan: Plan::template(mode),
    })
    .collect()
}
pub fn prefix(value: &str) -> String {
    let mut result = String::from("^");
    for c in value.chars() {
        if ".+*?^${}()|[]\\".contains(c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}
pub async fn page<T: DeserializeOwned + Send>(
    state: &AppState,
    context: AuthContext,
    collection: &'static str,
    input: PageInput,
    filter: Document,
) -> Result<(Vec<T>, u64), AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt"])?;
    transaction::run_snapshot(state.require_database()?,(state.clone(),context,input,filter,sort),|session,(state,context,input,filter,sort)|async move{
        let db=state.require_database().map_err(tx)?;let actor=owned::fresh(db,session,context,policy("workflows.read"),false).await?;let mut filter=filter.clone();if actor.scope==DataScope::Own{filter.insert("ownerId",actor.id);}
        let mut pipeline=vec![doc!{"$match":filter}];if actor.scope==DataScope::Roles{pipeline.push(doc!{"$lookup":{"from":"users","localField":"ownerId","foreignField":"_id","pipeline":[{"$match":{"deletedAt":null}},{"$project":{"roleIds":1}}],"as":"__scopeOwner"}});pipeline.push(doc!{"$match":{"$or":[{"ownerId":actor.id},{"__scopeOwner.roleIds":{"$in":&actor.scope_role_ids}}]}});}
        pipeline.push(doc!{"$sort":sort.clone()});pipeline.push(doc!{"$facet":{"rows":[{"$skip":skip as i64},{"$limit":input.page_size as i64},{"$project":{"__scopeOwner":0}}],"count":[{"$count":"total"}]}});let mut cursor=db.collection::<Document>(collection).aggregate(pipeline).max_time(std::time::Duration::from_secs(3)).session(&mut *session).await?;let result=cursor.stream(session).try_next().await?.ok_or_else(||fail(Failure::NotReady))?;
        let values=result.get_array("rows").map_err(|_|fail(Failure::NotReady))?.iter().map(|value|bson::deserialize_from_bson(value.clone()).map_err(|_|fail(Failure::NotReady))).collect::<mongodb::error::Result<Vec<T>>>()?;let total=result.get_array("count").map_err(|_|fail(Failure::NotReady))?.first().and_then(|value|value.as_document()).map(|value|value.get_i64("total").or_else(|_|value.get_i32("total").map(i64::from))).transpose().map_err(|_|fail(Failure::NotReady))?.unwrap_or(0);Ok((values,u64::try_from(total).map_err(|_|fail(Failure::NotReady))?))
    }.boxed()).await
}
pub async fn plans(
    state: &AppState,
    context: AuthContext,
    input: PageInput,
) -> Result<Page<PlanView>, AppError> {
    let mut filter = doc! {"deletedAt":null};
    if let Some(search) = input
        .search
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        filter.insert("name", doc! {"$regex":prefix(search.trim())});
    }
    let page_input = PageInput {
        page: input.page,
        page_size: input.page_size,
        search: input.search.clone(),
        sort: input.sort.clone(),
    };
    let (rows, total) = page::<PlanRecord>(state, context, "workflow_plans", input, filter).await?;
    Ok(Page::new(
        rows.into_iter()
            .map(PlanRecord::view)
            .collect::<Result<_, _>>()?,
        &page_input,
        total,
    ))
}
pub async fn plan_record(
    db: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    id: ObjectId,
    active: bool,
) -> mongodb::error::Result<PlanRecord> {
    let mut filter = doc! {"_id":id};
    if active {
        filter.insert("deletedAt", bson::Bson::Null);
    }
    let row = db
        .collection::<PlanRecord>("workflow_plans")
        .find_one(filter)
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    owned::owns(db, session, actor, row.owner_id).await?;
    Ok(row)
}
fn revision_context(
    id: ObjectId,
    plan_id: ObjectId,
    owner_id: ObjectId,
    version: i64,
    name: &str,
    sha256: &str,
) -> String {
    let metadata = serde_json::json!([
        id.to_hex(),
        plan_id.to_hex(),
        owner_id.to_hex(),
        version,
        name,
        sha256
    ]);
    format!(
        "workflow/definition/{}",
        canonical::checksum(metadata.to_string().as_bytes())
    )
}
fn revision_aad(row: &Revision) -> String {
    revision_context(
        row.id,
        row.plan_id,
        row.owner_id,
        row.version,
        &row.name,
        &row.sha256,
    )
}
pub fn open_revision(config: &crate::config::Config, row: &Revision) -> Result<Plan, AppError> {
    let bytes = payload::open(config, &revision_aad(row), &row.blob, payload::MAX_INPUT)?;
    if canonical::checksum(&bytes) != row.sha256 {
        return Err(AppError::SecretStoreUnavailable);
    }
    let plan: Plan =
        serde_json::from_slice(&bytes).map_err(|_| AppError::SecretStoreUnavailable)?;
    plan.validate()
        .map_err(|_| AppError::SecretStoreUnavailable)?;
    Ok(plan)
}
fn revision(
    config: &crate::config::Config,
    id: ObjectId,
    plan_id: ObjectId,
    owner_id: ObjectId,
    version: i64,
    name: &str,
    plan: &Plan,
) -> Result<Revision, AppError> {
    let json = Zeroizing::new(canonical::json(plan)?);
    let sha256 = canonical::checksum(json.as_bytes());
    let blob = payload::seal(
        config,
        &revision_context(id, plan_id, owner_id, version, name, &sha256),
        json.as_bytes(),
        payload::MAX_INPUT,
    )?;
    let row = Revision {
        id,
        plan_id,
        owner_id,
        version,
        name: name.into(),
        sha256,
        blob,
        created_at: DateTime::now(),
        expires_at: None,
    };
    Ok(row)
}
pub async fn create(
    state: &AppState,
    context: AuthContext,
    input: CreatePlan,
) -> Result<PlanView, AppError> {
    dto::request_id(&input.request_id)?;
    dto::plan(&input.name, &input.plan)?;
    let fingerprint = canonical::checksum(canonical::json(&input)?.as_bytes());
    let id = ObjectId::new();
    let revision_id = ObjectId::new();
    transaction::run(
        state.require_database()?,
        (state.clone(), context, input, fingerprint),
        |session, (state, context, input, fingerprint)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.write"), true).await?;
                if let Some(row) = db
                    .collection::<PlanRecord>("workflow_plans")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&input.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if row.creation_fingerprint != *fingerprint {
                        return Err(fail(Failure::Conflict("请求 UUID 已用于不同工作流")));
                    }
                    return row.view().map_err(tx);
                }
                if db
                    .collection::<Document>("workflow_plans")
                    .count_documents(doc! {"ownerId":actor.id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    >= 100
                    || db
                        .collection::<Document>("workflow_plans")
                        .count_documents(doc! {"deletedAt":null})
                        .session(&mut *session)
                        .await?
                        >= 1000
                {
                    return Err(fail(Failure::Conflict("工作流定义数量已达上限")));
                }
                let now = DateTime::now();
                let row = PlanRecord {
                    id,
                    owner_id: actor.id,
                    request_id: input.request_id.clone(),
                    creation_fingerprint: fingerprint.clone(),
                    name: input.name.clone(),
                    mode: input.plan.mode,
                    step_count: input.plan.steps.len() as u32,
                    version: 1,
                    revision_id,
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                let revision = revision(
                    &state.config,
                    revision_id,
                    id,
                    actor.id,
                    1,
                    &input.name,
                    &input.plan,
                )
                .map_err(tx)?;
                db.collection::<PlanRecord>("workflow_plans")
                    .insert_one(&row)
                    .session(&mut *session)
                    .await?;
                db.collection::<Revision>("workflow_plan_revisions")
                    .insert_one(revision)
                    .session(&mut *session)
                    .await?;
                shared::audit(db, &actor, "workflows.plans.create", id, session).await?;
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
) -> Result<PlanView, AppError> {
    if let Some(request) = &request {
        dto::request_id(request)?;
    }
    transaction::run_snapshot(state.require_database()?,(state.clone(),context,request),|session,(state,context,request)|async move{let db=state.require_database().map_err(tx)?;let actor=owned::fresh(db,session,context,policy("workflows.read"),false).await?;let row=if let Some(id)=id{plan_record(db,session,&actor,id,false).await?}else{db.collection::<PlanRecord>("workflow_plans").find_one(doc!{"ownerId":actor.id,"requestId":request.as_deref().ok_or_else(||fail(Failure::Validation("请求标识无效")))?}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?};row.view().map_err(tx)}.boxed()).await
}
pub async fn definition(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
    version: Option<i64>,
) -> Result<DefinitionView, AppError> {
    if let Some(version) = version {
        dto::version(version)?;
    }
    transaction::run_snapshot(state.require_database()?,(state.clone(),context),|session,(state,context)|async move{
        let db=state.require_database().map_err(tx)?;let actor=owned::fresh(db,session,context,policy("workflows.read"),false).await?;let row=plan_record(db,session,&actor,id,true).await?;if actor.id!=row.owner_id{return Err(fail(Failure::NotFound));}
        let revision=db.collection::<Revision>("workflow_plan_revisions").find_one(doc!{"planId":id,"ownerId":actor.id,"version":version.unwrap_or(row.version)}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;let plan=open_revision(&state.config,&revision).map_err(tx)?;Ok(DefinitionView{plan_id:id.to_hex(),version:revision.version,name:revision.name,sha256:revision.sha256,plan})
    }.boxed()).await
}
pub async fn patch(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
    input: PatchPlan,
) -> Result<PlanView, AppError> {
    dto::version(input.version)?;
    dto::plan(&input.name, &input.plan)?;
    let revision_id = ObjectId::new();
    transaction::run(
        state.require_database()?,
        (state.clone(), context, input),
        |session, (state, context, input)| {
            async move {
                let db = state.require_database().map_err(tx)?;
                let actor =
                    owned::fresh(db, session, context, policy("workflows.write"), true).await?;
                let mut row = plan_record(db, session, &actor, id, true).await?;
                if row.owner_id != actor.id {
                    return Err(fail(Failure::NotFound));
                }
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("工作流已修改，请刷新")));
                }
                if row.version >= dto::MAX_VERSION || row.version >= 256 {
                    return Err(fail(Failure::Conflict("工作流修订数量已达上限")));
                }
                row.version += 1;
                row.name = input.name.clone();
                row.mode = input.plan.mode;
                row.step_count = input.plan.steps.len() as u32;
                row.revision_id = revision_id;
                row.updated_at = DateTime::now();
                let revision = revision(
                    &state.config,
                    revision_id,
                    id,
                    actor.id,
                    row.version,
                    &row.name,
                    &input.plan,
                )
                .map_err(tx)?;
                db.collection::<Revision>("workflow_plan_revisions")
                    .insert_one(revision)
                    .session(&mut *session)
                    .await?;
                db.collection::<PlanRecord>("workflow_plans")
                    .replace_one(doc! {"_id":id}, &row)
                    .session(&mut *session)
                    .await?;
                shared::audit(db, &actor, "workflows.plans.update", id, session).await?;
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
    transaction::run(state.require_database()?,(state.clone(),context),|session,(state,context)|async move{
        let db=state.require_database().map_err(tx)?;let actor=owned::fresh(db,session,context,policy("workflows.write"),true).await?;let row=plan_record(db,session,&actor,id,true).await?;if row.owner_id!=actor.id{return Err(fail(Failure::NotFound));}
if row.version!=version{return Err(fail(Failure::Conflict("工作流已修改，请刷新")));}
        if db.collection::<Document>("workflow_runs").count_documents(doc!{"planId":id,"state":{"$in":["pending","running","unknown"]},"finishedAt":null}).session(&mut *session).await?>0{return Err(fail(Failure::Conflict("请先停止该定义的活动工作流任务")));}
        let now=DateTime::now();db.collection::<Document>("workflow_plans").update_one(doc!{"_id":id},doc!{"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;db.collection::<Document>("workflow_plan_revisions").update_many(doc!{"planId":id},doc!{"$set":{"expiresAt":DateTime::from_millis(now.timestamp_millis()+7*24*60*60*1000)}}).session(&mut *session).await?;shared::audit(db,&actor,"workflows.plans.delete",id,session).await
    }.boxed()).await
}
