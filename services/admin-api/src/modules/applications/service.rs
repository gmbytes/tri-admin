use super::{
    dto::{self, CreateApplication, PatchApplication, RotateSecret},
    models::{
        self, ActionRecord, ActionView, ApplicationMeta, ApplicationRecord, ApplicationView,
        IssuedApplication, RotatedApplication,
    },
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
    },
    modules::{
        auth::{AuthContext, repository},
        open_access::{credential, limits::Policy, scopes::Scope},
        rbac::shared::{self, Actor},
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
    options::ReturnDocument,
};
pub(crate) fn prefix(value: &str) -> String {
    let mut result = "^".to_owned();
    for c in value.chars() {
        if ".+*?^${}()|[]\\".contains(c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}
pub(crate) fn grant(
    permissions: &std::collections::HashSet<String>,
    scopes: &[Scope],
) -> mongodb::error::Result<()> {
    if permissions.contains("*")
        || scopes
            .iter()
            .all(|scope| permissions.contains(scope.admin_permission()))
    {
        Ok(())
    } else {
        Err(fail(Failure::Forbidden))
    }
}
pub(crate) async fn lock_app(
    database: &Database,
    session: &mut ClientSession,
    id: ObjectId,
) -> mongodb::error::Result<ApplicationMeta> {
    database
        .collection::<ApplicationMeta>("applications")
        .find_one_and_update(
            doc! {"_id":id,"deletedAt":null},
            doc! {"$inc":{"writeFence":1i64}},
        )
        .projection(models::application_projection())
        .return_document(ReturnDocument::Before)
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))
}
pub(crate) async fn app_in_session(
    database: &Database,
    session: &mut ClientSession,
    id: ObjectId,
) -> mongodb::error::Result<ApplicationMeta> {
    database
        .collection::<ApplicationMeta>("applications")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .projection(models::application_projection())
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))
}
pub(crate) async fn revoke_sessions(
    database: &Database,
    session: &mut ClientSession,
    id: ObjectId,
    kind: Option<&str>,
    user: Option<ObjectId>,
) -> mongodb::error::Result<()> {
    let mut filter = doc! {"applicationId":id,"status":"active"};
    if let Some(kind) = kind {
        filter.insert("principalKind", kind);
    }
    if let Some(user) = user {
        filter.insert("userId", user);
    }
    database
        .collection::<Document>("open_sessions")
        .update_many(
            filter,
            doc! {"$set":{"status":"revoked","revokedAt":DateTime::now()}},
        )
        .session(session)
        .await?;
    Ok(())
}
pub async fn create(
    database: &Database,
    context: &AuthContext,
    input: CreateApplication,
) -> Result<(IssuedApplication, bool), AppError> {
    let session_id = context.require_session()?;
    let actor = Actor::new(context)?;
    let input = input.validate()?;
    let fingerprint =
        crate::common::canonical::checksum(crate::common::canonical::json(&input)?.as_bytes());
    let id = ObjectId::new();
    let (secret, hash) = credential::issue("tri_app1", id)?;
    let now = DateTime::now();
    let record = ApplicationRecord {
        meta: ApplicationMeta {
            id,
            created_by: actor.id,
            request_id: input.request_id,
            name: input.name,
            code: input.code,
            enabled: input.enabled,
            allowed_scopes: input.allowed_scopes,
            allowed_ips: input.allowed_ips,
            minute_limit: input.minute_limit,
            daily_quota: input.daily_quota,
            credential_version: 1,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        },
        secret_hash: hash,
        fingerprint,
        write_fence: 0,
    };
    let (meta, created) = transaction::run(
        database,
        (database.clone(), actor, record),
        |session, (database, actor, record)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "applications", session).await?;
                let permissions = repository::interactive_permissions(
                    database,
                    session,
                    actor.id,
                    session_id,
                    &["applications.write"],
                )
                .await?;
                if let Some(previous) = database
                    .collection::<ApplicationRecord>("applications")
                    .find_one(doc! {"createdBy":actor.id,"requestId":&record.meta.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if previous.fingerprint != record.fingerprint {
                        return Err(fail(Failure::Conflict("创建请求标识已用于不同输入")));
                    }
                    return Ok((previous.meta, false));
                }
                grant(&permissions, &record.meta.allowed_scopes)?;
                database
                    .collection::<ApplicationRecord>("applications")
                    .insert_one(&*record)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "applications.create", id, session).await?;
                Ok((record.meta.clone(), true))
            }
            .boxed()
        },
    )
    .await?;
    Ok((
        IssuedApplication {
            application: meta.view()?,
            secret: created.then_some(secret),
        },
        created,
    ))
}
pub async fn get(database: &Database, id: ObjectId) -> Result<ApplicationView, AppError> {
    database
        .collection::<ApplicationMeta>("applications")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .projection(models::application_projection())
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
pub async fn by_request(
    database: &Database,
    actor: ObjectId,
    request: &str,
) -> Result<ApplicationView, AppError> {
    dto::request_id(request)?;
    database
        .collection::<ApplicationMeta>("applications")
        .find_one(doc! {"createdBy":actor,"requestId":request})
        .projection(models::application_projection())
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
pub async fn list(
    database: &Database,
    input: PageInput,
) -> Result<Page<ApplicationView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "code", "name"])?;
    let mut filter = doc! {"deletedAt":null};
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        filter.insert("code", doc! {"$regex":prefix(&search.to_ascii_lowercase())});
    }
    let (rows, total) = transaction::run_snapshot(
        database,
        (database.clone(), filter, sort),
        |session, (database, filter, sort)| {
            async move {
                let total = database
                    .collection::<Document>("applications")
                    .count_documents(filter.clone())
                    .session(&mut *session)
                    .await?;
                let mut cursor = database
                    .collection::<ApplicationMeta>("applications")
                    .find(filter.clone())
                    .projection(models::application_projection())
                    .sort(sort.clone())
                    .skip(skip)
                    .limit(input.page_size as i64)
                    .session(&mut *session)
                    .await?;
                Ok((cursor.stream(session).try_collect::<Vec<_>>().await?, total))
            }
            .boxed()
        },
    )
    .await?;
    Ok(Page::new(
        rows.into_iter()
            .map(ApplicationMeta::view)
            .collect::<Result<Vec<_>, _>>()?,
        &input,
        total,
    ))
}
pub async fn patch(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    input: PatchApplication,
) -> Result<ApplicationView, AppError> {
    let session_id = context.require_session()?;
    dto::version(input.version)?;
    let actor = Actor::new(context)?;
    if input.name.is_none()
        && input.enabled.is_none()
        && input.allowed_scopes.is_none()
        && input.allowed_ips.is_none()
        && input.minute_limit.is_none()
        && input.daily_quota.is_none()
    {
        return Err(AppError::Validation("请提供需要修改的应用字段".into()));
    }
    let name = input.name.as_deref().map(str::trim).map(String::from);
    if let Some(name) = &name {
        dto::name(name, "应用名称", 80)?;
    }
    let scopes = input
        .allowed_scopes
        .map(|v| crate::modules::open_access::scopes::normalize(v, false))
        .transpose()?;
    let meta = transaction::run(
        database,
        (database.clone(), actor, name, scopes, input.allowed_ips),
        |session, (database, actor, name, scopes, ips)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                let permissions = repository::interactive_permissions(
                    database,
                    session,
                    actor.id,
                    session_id,
                    &["applications.write"],
                )
                .await?;
                let mut current = lock_app(database, session, id).await?;
                if current.version != input.version {
                    return Err(fail(Failure::Conflict("应用版本已改变")));
                }
                if let Some(scopes) = scopes {
                    grant(&permissions, scopes)?;
                    current.allowed_scopes = scopes.clone();
                }
                if input.enabled == Some(true)
                    || ips.is_some()
                    || input.minute_limit.is_some()
                    || input.daily_quota.is_some()
                {
                    grant(&permissions, &current.allowed_scopes)?;
                }
                let policy = Policy {
                    allowed_ips: ips.clone().unwrap_or(current.allowed_ips),
                    minute_limit: input.minute_limit.unwrap_or(current.minute_limit),
                    daily_quota: input.daily_quota.unwrap_or(current.daily_quota),
                }
                .normalize()
                .map_err(|_| fail(Failure::Validation("来源或调用额度无效")))?;
                current.allowed_ips = policy.allowed_ips;
                current.minute_limit = policy.minute_limit;
                current.daily_quota = policy.daily_quota;
                if let Some(name) = name {
                    current.name = name.clone();
                }
                if let Some(enabled) = input.enabled {
                    current.enabled = enabled;
                    if !enabled {
                        revoke_sessions(database, session, id, None, None).await?;
                    }
                }
                current.version += 1;
                current.updated_at = DateTime::now();
                let mut data = mongodb::bson::serialize_to_document(&current)
                    .map_err(|_| fail(Failure::NotReady))?;
                data.remove("_id");
                database
                    .collection::<Document>("applications")
                    .update_one(doc! {"_id":id}, doc! {"$set":data})
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "applications.update", id, session).await?;
                Ok(current)
            }
            .boxed()
        },
    )
    .await?;
    meta.view()
}
pub async fn delete(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    dto::version(version)?;
    let actor = Actor::new(context)?;
    let sid = context.require_session()?;
    transaction::run(database,(database.clone(),actor),|session,(database,actor)|async move{shared::lock(database,"rbac",session).await?;repository::interactive_permissions(database,session,actor.id,sid,&["applications.write"]).await?;let app=lock_app(database,session,id).await?;if app.version!=version{return Err(fail(Failure::Conflict("应用版本已改变")));}let now=DateTime::now();database.collection::<Document>("applications").update_one(doc!{"_id":id},doc!{"$set":{"enabled":false,"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;revoke_sessions(database,session,id,None,None).await?;shared::audit(database,actor,"applications.delete",id,session).await?;Ok(())}.boxed()).await
}
pub async fn rotate(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    input: RotateSecret,
) -> Result<RotatedApplication, AppError> {
    let sid = context.require_session()?;
    dto::request_id(&input.request_id)?;
    dto::version(input.version)?;
    let actor = Actor::new(context)?;
    let fingerprint =
        crate::common::canonical::checksum(crate::common::canonical::json(&input)?.as_bytes());
    let (secret, hash) = credential::issue("tri_app1", id)?;
    let(meta,receipt,created)=transaction::run(database,(database.clone(),actor,input,fingerprint,hash),|session,(database,actor,input,fingerprint,hash)|async move{
        shared::lock(database,"rbac",session).await?;let permissions=repository::interactive_permissions(database,session,actor.id,sid,&["applications.write"]).await?;
        if let Some(receipt)=database.collection::<ActionRecord>("application_actions").find_one(doc!{"applicationId":id,"actorId":actor.id,"requestId":&input.request_id}).session(&mut *session).await?{if receipt.fingerprint!=*fingerprint{return Err(fail(Failure::Conflict("轮换请求标识已用于不同输入")));}let meta=database.collection::<ApplicationMeta>("applications").find_one(doc!{"_id":id}).projection(models::application_projection()).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;return Ok((meta,receipt,false));}
        let mut current=lock_app(database,session,id).await?;if current.version!=input.version{return Err(fail(Failure::Conflict("应用版本已改变")));}grant(&permissions,&current.allowed_scopes)?;
        if current.credential_version>=dto::MAX_VERSION{return Err(fail(Failure::Conflict("凭据版本已达到上限")));}
        let now=DateTime::now();current.version+=1;current.credential_version+=1;current.updated_at=now;
        database.collection::<Document>("applications").update_one(doc!{"_id":id},doc!{"$set":{"secretHash":hash.as_str(),"version":current.version,"credentialVersion":current.credential_version,"updatedAt":now}}).session(&mut *session).await?;revoke_sessions(database,session,id,Some("application"),None).await?;
        let receipt=ActionRecord{id:ObjectId::new(),application_id:id,actor_id:actor.id,request_id:input.request_id.clone(),fingerprint:fingerprint.clone(),version:current.version,credential_version:current.credential_version,created_at:now};
        database.collection::<ActionRecord>("application_actions").insert_one(&receipt).session(&mut *session).await?;shared::audit(database,actor,"applications.rotateSecret",id,session).await?;Ok((current,receipt,true))
    }.boxed()).await?;
    Ok(RotatedApplication {
        application: meta.view()?,
        secret: created.then_some(secret),
        receipt: receipt.view()?,
    })
}
pub async fn action(
    database: &Database,
    id: ObjectId,
    actor: ObjectId,
    request: &str,
) -> Result<ActionView, AppError> {
    dto::request_id(request)?;
    database
        .collection::<ActionRecord>("application_actions")
        .find_one(doc! {"applicationId":id,"actorId":actor,"requestId":request})
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
