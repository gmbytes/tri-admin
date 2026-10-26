use super::{
    dto::{self, CreateUser, PasswordInput, PatchUser},
    models::{self, UserMeta, UserRecord, UserView},
    service,
    user_authority::{Action, Authority},
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        password,
        transaction::{self, Failure, fail},
        validation,
    },
    modules::open_access::scopes,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
async fn current(
    database: &Database,
    session: &mut ClientSession,
    application: ObjectId,
    id: ObjectId,
) -> mongodb::error::Result<UserMeta> {
    database
        .collection::<UserMeta>("application_users")
        .find_one(doc! {"_id":id,"applicationId":application,"deletedAt":null})
        .projection(models::user_projection())
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))
}
pub async fn create(
    database: &Database,
    authority: Authority,
    application: ObjectId,
    input: CreateUser,
) -> Result<UserView, AppError> {
    let username = input.username.trim().to_owned();
    let display = input.display_name.trim().to_owned();
    validation::username(&username)?;
    dto::name(&display, "显示名称", 80)?;
    let scopes = scopes::normalize(input.scopes, true)?;
    let hash = password::hash(input.password).await?;
    let now = DateTime::now();
    let id = ObjectId::new();
    let record = UserRecord {
        meta: UserMeta {
            id,
            application_id: application,
            username_key: username.to_lowercase(),
            username,
            display_name: display,
            scopes,
            status: input.status,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        },
        password_hash: hash,
        auth_version: 1,
    };
    transaction::run(
        database,
        (database.clone(), authority, record),
        |session, (database, authority, record)| {
            async move {
                let authorized = authority.authorize(database, session, application).await?;
                authorized.check_grant(&record.meta.scopes)?;
                database
                    .collection::<UserRecord>("application_users")
                    .insert_one(&*record)
                    .session(&mut *session)
                    .await?;
                authorized
                    .audit(database, session, Action::Create, id)
                    .await?;
                Ok(record.meta.clone())
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn get(
    database: &Database,
    application: ObjectId,
    id: ObjectId,
) -> Result<UserView, AppError> {
    transaction::run_snapshot(database, database.clone(), |session, database| {
        async move {
            service::app_in_session(database, session, application).await?;
            current(database, session, application, id).await
        }
        .boxed()
    })
    .await?
    .view()
}
pub async fn list(
    database: &Database,
    application: ObjectId,
    input: PageInput,
) -> Result<Page<UserView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "username", "displayName"])?;
    let mut filter = doc! {"applicationId":application,"deletedAt":null};
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        filter.insert(
            "usernameKey",
            doc! {"$regex":service::prefix(&search.to_lowercase())},
        );
    }
    let (rows, total) = transaction::run_snapshot(
        database,
        (database.clone(), filter, sort),
        |session, (database, filter, sort)| {
            async move {
                service::app_in_session(database, session, application).await?;
                let total = database
                    .collection::<Document>("application_users")
                    .count_documents(filter.clone())
                    .session(&mut *session)
                    .await?;
                let mut cursor = database
                    .collection::<UserMeta>("application_users")
                    .find(filter.clone())
                    .projection(models::user_projection())
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
            .map(UserMeta::view)
            .collect::<Result<Vec<_>, _>>()?,
        &input,
        total,
    ))
}
pub async fn patch(
    database: &Database,
    authority: Authority,
    application: ObjectId,
    id: ObjectId,
    input: PatchUser,
) -> Result<UserView, AppError> {
    dto::version(input.version)?;
    if input.username.is_none()
        && input.display_name.is_none()
        && input.scopes.is_none()
        && input.status.is_none()
    {
        return Err(AppError::Validation("请提供需要修改的用户字段".into()));
    }
    let username = input.username.as_deref().map(str::trim).map(String::from);
    if let Some(name) = &username {
        validation::username(name)?;
    }
    let display = input
        .display_name
        .as_deref()
        .map(str::trim)
        .map(String::from);
    if let Some(value) = &display {
        dto::name(value, "显示名称", 80)?;
    }
    let scopes = input
        .scopes
        .map(|v| scopes::normalize(v, true))
        .transpose()?;
    transaction::run(
        database,
        (database.clone(), authority, username, display, scopes),
        |session, (database, authority, username, display, scopes)| {
            async move {
                let authorized = authority.authorize(database, session, application).await?;
                let mut user = current(database, session, application, id).await?;
                if user.version != input.version {
                    return Err(fail(Failure::Conflict("应用用户版本已改变")));
                }
                if let Some(scopes) = scopes {
                    authorized.check_grant(scopes)?;
                    user.scopes = scopes.clone();
                }
                if let Some(username) = username {
                    user.username = username.clone();
                    user.username_key = username.to_lowercase();
                }
                if let Some(display) = display {
                    user.display_name = display.clone();
                }
                let status_changed = input.status.is_some_and(|status| status != user.status);
                if input.status == Some(dto::UserStatus::Active) && status_changed {
                    authorized.check_grant(&user.scopes)?;
                }
                if let Some(status) = input.status {
                    user.status = status;
                }
                user.version += 1;
                user.updated_at = DateTime::now();
                let mut data = mongodb::bson::serialize_to_document(&user)
                    .map_err(|_| fail(Failure::NotReady))?;
                data.remove("_id");
                let mut update = doc! {"$set":data};
                if status_changed {
                    update.insert("$inc", doc! {"authVersion":1i64});
                    service::revoke_sessions(
                        database,
                        session,
                        application,
                        Some("user"),
                        Some(id),
                    )
                    .await?;
                }
                database
                    .collection::<Document>("application_users")
                    .update_one(doc! {"_id":id,"applicationId":application}, update)
                    .session(&mut *session)
                    .await?;
                authorized
                    .audit(database, session, Action::Update, id)
                    .await?;
                Ok(user)
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn delete(
    database: &Database,
    authority: Authority,
    application: ObjectId,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    dto::version(version)?;
    transaction::run(database,(database.clone(),authority),|session,(database,authority)|async move{
        let authorized=authority.authorize(database,session,application).await?;let user=current(database,session,application,id).await?;if user.version!=version{return Err(fail(Failure::Conflict("应用用户版本已改变")));}
        let now=DateTime::now();database.collection::<Document>("application_users").update_one(doc!{"_id":id,"applicationId":application},doc!{"$set":{"deletedAt":now,"updatedAt":now,"status":"disabled"},"$inc":{"version":1i64,"authVersion":1i64}}).session(&mut *session).await?;
        service::revoke_sessions(database,session,application,Some("user"),Some(id)).await?;authorized.audit(database,session,Action::Delete,id).await?;Ok(())
    }.boxed()).await
}
pub async fn reset_password(
    database: &Database,
    authority: Authority,
    application: ObjectId,
    id: ObjectId,
    input: PasswordInput,
) -> Result<(), AppError> {
    dto::version(input.version)?;
    let hash = password::hash(input.password).await?;
    transaction::run(database,(database.clone(),authority,hash),|session,(database,authority,hash)|async move{
        let authorized=authority.authorize(database,session,application).await?;let user=current(database,session,application,id).await?;if user.version!=input.version{return Err(fail(Failure::Conflict("应用用户版本已改变")));}authorized.check_grant(&user.scopes)?;
        database.collection::<Document>("application_users").update_one(doc!{"_id":id,"applicationId":application},doc!{"$set":{"passwordHash":hash.as_str(),"updatedAt":DateTime::now()},"$inc":{"version":1i64,"authVersion":1i64}}).session(&mut *session).await?;
        service::revoke_sessions(database,session,application,Some("user"),Some(id)).await?;authorized.audit(database,session,Action::ResetPassword,id).await?;Ok(())
    }.boxed()).await
}
