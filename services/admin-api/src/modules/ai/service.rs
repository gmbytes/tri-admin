use super::{
    binding,
    dto::{self, ProviderInput},
    models::{ApprovedModel, Provider, ProviderView},
};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::{
        auth::AuthContext,
        rbac::{
            owned::{self, Resource},
            shared,
        },
    },
    state::AppState,
};
use futures_util::FutureExt;
use mongodb::{
    ClientSession, Database,
    bson::{self, DateTime, Document, doc, oid::ObjectId},
};
pub fn policy(permission: &'static str) -> Resource {
    Resource {
        schema: "ai",
        version: crate::storage::ai_schema::VERSION,
        read: "ai.read",
        write: permission,
    }
}
async fn rows(db: &Database, session: &mut ClientSession) -> mongodb::error::Result<Vec<Provider>> {
    let mut cursor = db
        .collection::<Provider>("ai_providers")
        .find(doc! {"deletedAt":null})
        .sort(doc! {"name":1,"_id":1})
        .limit(101)
        .session(&mut *session)
        .await?;
    let mut result = Vec::new();
    while let Some(row) = cursor.next(&mut *session).await.transpose()? {
        result.push(row);
    }
    if result.len() > 100 {
        return Err(fail(Failure::NotReady));
    }
    Ok(result)
}
pub async fn list(
    state: &AppState,
    context: AuthContext,
    input: PageInput,
) -> Result<Page<ProviderView>, AppError> {
    let skip = input.validate()?;
    input.sort_document(&["createdAt", "name"])?;
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context, input),
        |session, (state, context, input)| {
            async move {
                let db = state
                    .require_database()
                    .map_err(|_| fail(Failure::NotReady))?;
                owned::fresh(db, session, context, policy("ai.read"), false).await?;
                let mut values = rows(db, session).await?;
                if let Some(search) = input.search.as_deref() {
                    let search = search.trim().to_lowercase();
                    values.retain(|row| row.name.to_lowercase().starts_with(&search));
                }
                let sort = input.sort.as_deref().unwrap_or("-createdAt");
                values.sort_by(|a, b| {
                    let order = if sort.trim_start_matches('-') == "name" {
                        a.name.cmp(&b.name).then(a.id.cmp(&b.id))
                    } else {
                        a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id))
                    };
                    if sort.starts_with('-') {
                        order.reverse()
                    } else {
                        order
                    }
                });
                let total = values.len() as u64;
                let mut data = Vec::new();
                for row in values
                    .into_iter()
                    .skip(skip as usize)
                    .take(input.page_size as usize)
                {
                    let current = binding::current(db, session, &state.config, &row).await?;
                    data.push(row.view(current).map_err(|_| fail(Failure::NotReady))?);
                }
                Ok(Page::new(data, input, total))
            }
            .boxed()
        },
    )
    .await
}
pub async fn models(
    state: &AppState,
    context: AuthContext,
    input: PageInput,
) -> Result<Page<ApprovedModel>, AppError> {
    let skip = input.validate()?;
    if input.sort.is_some() {
        return Err(AppError::Validation("模型目录使用固定排序".into()));
    }
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context, input),
        |session, (state, context, input)| {
            async move {
                let db = state
                    .require_database()
                    .map_err(|_| fail(Failure::NotReady))?;
                owned::fresh(db, session, context, policy("ai.read"), false).await?;
                let mut values = Vec::new();
                for provider in rows(db, session).await? {
                    if !provider.enabled
                        || !binding::current(db, session, &state.config, &provider).await?
                    {
                        continue;
                    }
                    for model in &provider.models {
                        values.push(ApprovedModel {
                            provider_id: provider.id.to_hex(),
                            provider_name: provider.name.clone(),
                            provider_version: provider.version,
                            id: model.id.clone(),
                            label: model.label.clone(),
                            max_output_tokens: model.max_output_tokens,
                        });
                    }
                }
                if let Some(search) = input.search.as_deref() {
                    let search = search.trim().to_lowercase();
                    values.retain(|row| {
                        row.id.to_lowercase().starts_with(&search)
                            || row.label.to_lowercase().starts_with(&search)
                            || row.provider_name.to_lowercase().starts_with(&search)
                    });
                }
                values.sort_by(|a, b| {
                    a.provider_name
                        .cmp(&b.provider_name)
                        .then(a.provider_id.cmp(&b.provider_id))
                        .then(a.id.cmp(&b.id))
                });
                let total = values.len() as u64;
                Ok(Page::new(
                    values
                        .into_iter()
                        .skip(skip as usize)
                        .take(input.page_size as usize)
                        .collect(),
                    input,
                    total,
                ))
            }
            .boxed()
        },
    )
    .await
}
pub async fn get(
    state: &AppState,
    context: AuthContext,
    id: ObjectId,
) -> Result<ProviderView, AppError> {
    transaction::run_snapshot(
        state.require_database()?,
        (state.clone(), context),
        |session, (state, context)| {
            async move {
                let db = state
                    .require_database()
                    .map_err(|_| fail(Failure::NotReady))?;
                owned::fresh(db, session, context, policy("ai.read"), false).await?;
                let row = db
                    .collection::<Provider>("ai_providers")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                let current = binding::current(db, session, &state.config, &row).await?;
                row.view(current).map_err(|_| fail(Failure::NotReady))
            }
            .boxed()
        },
    )
    .await
}
pub async fn save(
    state: &AppState,
    context: AuthContext,
    target: Option<(ObjectId, i64)>,
    input: ProviderInput,
) -> Result<ProviderView, AppError> {
    input.validate()?;
    if let Some((_, version)) = target {
        dto::version(version)?;
    }
    let id = target.map(|value| value.0).unwrap_or_default();
    transaction::run(
        state.require_database()?,
        (state.clone(), context, input),
        |session, (state, context, input)| {
            async move {
                let db = state
                    .require_database()
                    .map_err(|_| fail(Failure::NotReady))?;
                let actor = owned::fresh(db, session, context, policy("ai.write"), true).await?;
                if !actor.can("settings.write") || actor.scope != DataScope::All {
                    return Err(fail(Failure::Forbidden));
                }
                shared::lock(db, "settings", session).await?;
                let now = DateTime::now();
                let profile_id = validation::object_id(&input.profile_id)
                    .map_err(|_| fail(Failure::Validation("配置标识无效")))?;
                let binding_fingerprint = binding::bind(
                    db,
                    session,
                    &state.config,
                    profile_id,
                    input.profile_version,
                )
                .await?;
                let row = if let Some((id, version)) = target {
                    let mut row = db
                        .collection::<Provider>("ai_providers")
                        .find_one(doc! {"_id":id,"deletedAt":null})
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| fail(Failure::NotFound))?;
                    if row.version != version {
                        return Err(fail(Failure::Conflict("AI 上游已修改，请刷新")));
                    }
                    if row.version >= dto::MAX_VERSION {
                        return Err(fail(Failure::Conflict("版本已达上限")));
                    }
                    row.name = input.name.clone();
                    row.profile_id = profile_id;
                    row.profile_version = input.profile_version;
                    row.binding_fingerprint = binding_fingerprint;
                    row.models = input.models.clone();
                    row.enabled = input.enabled;
                    row.version += 1;
                    row.updated_at = now;
                    db.collection::<Provider>("ai_providers")
                        .replace_one(doc! {"_id":id}, &row)
                        .session(&mut *session)
                        .await?;
                    row
                } else {
                    if db
                        .collection::<Document>("ai_providers")
                        .count_documents(doc! {"deletedAt":null})
                        .session(&mut *session)
                        .await?
                        >= 100
                    {
                        return Err(fail(Failure::Conflict("AI 上游数量已达上限")));
                    }
                    let row = Provider {
                        id,
                        name: input.name.clone(),
                        profile_id,
                        profile_version: input.profile_version,
                        binding_fingerprint,
                        models: input.models.clone(),
                        enabled: input.enabled,
                        version: 1,
                        created_by: actor.id,
                        created_at: now,
                        updated_at: now,
                        deleted_at: None,
                    };
                    db.collection::<Provider>("ai_providers")
                        .insert_one(&row)
                        .session(&mut *session)
                        .await?;
                    row
                };
                shared::audit(
                    db,
                    &actor,
                    if target.is_some() {
                        "ai.providers.update"
                    } else {
                        "ai.providers.create"
                    },
                    id,
                    session,
                )
                .await?;
                row.view(true).map_err(|_| fail(Failure::NotReady))
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
        let db=state.require_database().map_err(|_|fail(Failure::NotReady))?;let actor=owned::fresh(db,session,context,policy("ai.write"),true).await?;if !actor.can("settings.write")||actor.scope!=DataScope::All{return Err(fail(Failure::Forbidden));}shared::lock(db,"settings",session).await?;
        let result=db.collection::<Document>("ai_providers").update_one(doc!{"_id":id,"deletedAt":null,"version":version},doc!{"$set":{"deletedAt":DateTime::now(),"enabled":false},"$inc":{"version":1i64}}).session(&mut *session).await?;if result.matched_count!=1{if db.collection::<Document>("ai_providers").find_one(doc!{"_id":id,"deletedAt":bson::Bson::Null}).session(&mut *session).await?.is_some(){return Err(fail(Failure::Conflict("AI 上游已修改，请刷新")));}return Err(fail(Failure::NotFound));}
        shared::audit(db,&actor,"ai.providers.delete",id,session).await
    }.boxed()).await
}
