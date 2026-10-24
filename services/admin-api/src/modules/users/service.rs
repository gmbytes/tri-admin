use super::dto::{CreateUser, PasswordInput, PatchUser};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        password,
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::{
        models::{RoleRecord, UserRecord, UserView},
        shared::{self, Actor},
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

fn validate_status(value: &str) -> Result<(), AppError> {
    if matches!(value, "active" | "disabled") {
        Ok(())
    } else {
        Err(AppError::Validation("用户状态无效".into()))
    }
}
fn prefix(value: &str) -> String {
    let mut escaped = "^".to_owned();
    for character in value.chars() {
        if ".+*?^${}()|[]\\".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub async fn list(
    database: &Database,
    actor: &Actor,
    input: PageInput,
) -> Result<Page<UserView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "username", "displayName"])?;
    let mut filter = doc! {"deletedAt":null};
    if actor.scope == DataScope::Own {
        filter.insert("_id", actor.id);
    }
    if let Some(search) = &input.search
        && !search.trim().is_empty()
    {
        filter.insert(
            "usernameKey",
            doc! {"$regex":prefix(&search.trim().to_lowercase())},
        );
    }
    let total = database
        .collection::<Document>("users")
        .count_documents(filter.clone())
        .await?;
    let data = if skip >= total {
        vec![]
    } else {
        database
            .collection::<UserRecord>("users")
            .find(filter)
            .sort(sort)
            .skip(skip)
            .limit(input.page_size as i64)
            .await?
            .try_collect::<Vec<_>>()
            .await?
            .into_iter()
            .map(UserRecord::view)
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(Page::new(data, &input, total))
}

pub async fn get(database: &Database, actor: &Actor, id: ObjectId) -> Result<UserView, AppError> {
    actor.require_scope(id)?;
    database
        .collection::<UserRecord>("users")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}

async fn check_owner_change(
    database: &Database,
    actor: &Actor,
    before: &UserRecord,
    roles: &[ObjectId],
    status: &str,
    deleting: bool,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let owner = database
        .collection::<RoleRecord>("roles")
        .find_one(doc! {"slug":"owner","deletedAt":null})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotReady))?;
    if before.role_ids.contains(&owner.id) {
        if !actor.can("*") {
            return Err(fail(Failure::Forbidden));
        }
        if before.status == "active"
            && (deleting || status != "active" || !roles.contains(&owner.id))
        {
            let count = database
                .collection::<Document>("users")
                .count_documents(doc! {"roleIds":owner.id,"status":"active","deletedAt":null})
                .session(&mut *session)
                .await?;
            if count <= 1 {
                return Err(fail(Failure::Conflict("至少保留一名启用的 Owner 管理员")));
            }
        }
    }
    Ok(())
}

pub async fn create(
    database: &Database,
    actor: Actor,
    input: CreateUser,
) -> Result<UserView, AppError> {
    if actor.scope != DataScope::All {
        return Err(AppError::Forbidden);
    }
    validation::username(&input.username)?;
    validation::text(&input.display_name, "显示名", 80)?;
    validation::password(&input.password)?;
    validate_status(&input.status)?;
    if !actor.can("roles.assign") {
        return Err(AppError::Forbidden);
    }
    let role_ids = shared::ids(&input.role_ids)?;
    let hash = password::hash(input.password).await?;
    let id = ObjectId::new();
    let result=transaction::run(database,(database.clone(),actor,input.username,input.display_name,input.status,role_ids,hash),|session,(database,actor,username,display_name,status,role_ids,hash)|async move{
        shared::lock(database,"rbac",session).await?;shared::roles_for_assignment(database,role_ids,actor,session).await?;let now=DateTime::now();
        database.collection::<Document>("users").insert_one(doc!{"_id":id,"username":username.as_str(),"usernameKey":username.to_lowercase(),"displayName":display_name.as_str(),"passwordHash":hash.as_str(),"roleIds":role_ids.clone(),"status":status.as_str(),"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).session(&mut *session).await?;
        shared::audit(database,actor,"users.create",id,session).await?;
        Ok(UserRecord{id,username:username.clone(),display_name:display_name.clone(),role_ids:role_ids.clone(),status:status.clone(),version:1,created_at:now,updated_at:now})
    }.boxed()).await?;
    result.view()
}

pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: PatchUser,
) -> Result<UserView, AppError> {
    actor.require_scope(id)?;
    if input.version < 1 {
        return Err(AppError::Validation("版本号无效".into()));
    }
    if let Some(username) = &input.username {
        validation::username(username)?;
    }
    if let Some(name) = &input.display_name {
        validation::text(name, "显示名", 80)?;
    }
    if let Some(status) = &input.status {
        validate_status(status)?;
    }
    let requested_roles = input
        .role_ids
        .as_ref()
        .map(|values| shared::ids(values))
        .transpose()?;
    let result=transaction::run(database,(database.clone(),actor,input,requested_roles),|session,(database,actor,input,requested_roles)|async move{
        shared::lock(database,"rbac",session).await?;
        let mut user=database.collection::<UserRecord>("users").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if user.version!=input.version{return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));}
        let roles=requested_roles.as_ref().unwrap_or(&user.role_ids);let status=input.status.as_deref().unwrap_or(&user.status);
        check_owner_change(database,actor,&user,roles,status,false,session).await?;
        if roles!=&user.role_ids{shared::roles_for_assignment(database,roles,actor,session).await?;}
        if let Some(name)=&input.username{user.username=name.clone();}
if let Some(name)=&input.display_name{user.display_name=name.clone();}
if let Some(roles)=requested_roles{user.role_ids=roles.clone();}
if let Some(status)=&input.status{user.status=status.clone();}
        user.version=user.version.checked_add(1).ok_or_else(||fail(Failure::Conflict("版本号已达上限")))?;user.updated_at=DateTime::now();
        database.collection::<Document>("users").update_one(doc!{"_id":id},doc!{"$set":{"username":&user.username,"usernameKey":user.username.to_lowercase(),"displayName":&user.display_name,"roleIds":user.role_ids.clone(),"status":&user.status,"version":user.version,"updatedAt":user.updated_at}}).session(&mut *session).await?;
        if user.status=="disabled"{shared::revoke_sessions(database,id,session).await?;}
        shared::audit(database,actor,"users.update",id,session).await?;Ok(user)
    }.boxed()).await?;
    result.view()
}

pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    actor.require_scope(id)?;
    if version < 1 {
        return Err(AppError::Validation("版本号无效".into()));
    }
    transaction::run(database,(database.clone(),actor),|session,(database,actor)|async move{
        shared::lock(database,"rbac",session).await?;
        let user=database.collection::<UserRecord>("users").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if user.version!=version{return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));}
        check_owner_change(database,actor,&user,&user.role_ids,"disabled",true,session).await?;let now=DateTime::now();
        database.collection::<Document>("users").update_one(doc!{"_id":id},doc!{"$set":{"status":"disabled","deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;
        shared::revoke_sessions(database,id,session).await?;shared::audit(database,actor,"users.delete",id,session).await?;Ok(())
    }.boxed()).await
}

pub async fn reset_password(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: PasswordInput,
) -> Result<(), AppError> {
    actor.require_scope(id)?;
    if input.version < 1 {
        return Err(AppError::Validation("版本号无效".into()));
    }
    let hash = password::hash(input.password).await?;
    transaction::run(database,(database.clone(),actor,hash),|session,(database,actor,hash)|async move{
        shared::lock(database,"rbac",session).await?;
        let user=database.collection::<UserRecord>("users").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if user.version!=input.version{return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));}
        check_owner_change(database,actor,&user,&user.role_ids,&user.status,false,session).await?;
        database.collection::<Document>("users").update_one(doc!{"_id":id},doc!{"$set":{"passwordHash":hash.as_str(),"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;
        shared::revoke_sessions(database,id,session).await?;shared::audit(database,actor,"users.resetPassword",id,session).await?;Ok(())
    }.boxed()).await
}
