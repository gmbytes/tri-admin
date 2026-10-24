use super::dto::{CreateRole, PatchRole};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::{
        models::{RoleRecord, RoleView},
        permissions,
        shared::{self, Actor},
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use std::collections::HashSet;

fn validate(
    actor: &Actor,
    name: &str,
    slug: &str,
    codes: &[String],
    scope: DataScope,
) -> Result<(), AppError> {
    validation::text(name, "角色名", 80)?;
    if !(3..=64).contains(&slug.len())
        || !slug.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        || !slug
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err(AppError::Validation(
            "角色标识需为 3–64 位小写字母、数字或短横线".into(),
        ));
    }
    if codes.len() > 128
        || codes.iter().collect::<HashSet<_>>().len() != codes.len()
        || codes.iter().any(|code| !permissions::known(code))
    {
        return Err(AppError::Validation("权限代码无效或重复".into()));
    }
    if codes.iter().any(|code| !actor.can(code))
        || scope == DataScope::All && actor.scope != DataScope::All
    {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

pub async fn list(database: &Database, input: PageInput) -> Result<Page<RoleView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "name", "slug"])?;
    let filter = doc! {"deletedAt":null};
    let total = database
        .collection::<Document>("roles")
        .count_documents(filter.clone())
        .await?;
    let data = if skip >= total {
        vec![]
    } else {
        database
            .collection::<RoleRecord>("roles")
            .find(filter)
            .sort(sort)
            .skip(skip)
            .limit(input.page_size as i64)
            .await?
            .try_collect::<Vec<_>>()
            .await?
            .into_iter()
            .map(RoleRecord::view)
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(Page::new(data, &input, total))
}

pub async fn create(
    database: &Database,
    actor: Actor,
    input: CreateRole,
) -> Result<RoleView, AppError> {
    validate(
        &actor,
        &input.name,
        &input.slug,
        &input.permissions,
        input.data_scope,
    )?;
    let id = ObjectId::new();
    let result=transaction::run(database,(database.clone(),actor,input),|session,(database,actor,input)|async move {
        shared::lock(database,"rbac",session).await?;let now=DateTime::now();
        database.collection::<Document>("roles").insert_one(doc!{"_id":id,"name":&input.name,"slug":&input.slug,"permissions":input.permissions.clone(),"dataScope":input.data_scope.as_str(),"protected":false,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).session(&mut *session).await?;
        shared::audit(database,actor,"roles.create",id,session).await?;
        Ok(RoleRecord{id,name:input.name.clone(),slug:input.slug.clone(),permissions:input.permissions.clone(),data_scope:input.data_scope,protected:false,version:1,created_at:now,updated_at:now})
    }.boxed()).await?;
    result.view()
}

pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: PatchRole,
) -> Result<RoleView, AppError> {
    if input.version < 1 {
        return Err(AppError::Validation("版本号无效".into()));
    }
    let result=transaction::run(database,(database.clone(),actor,input),|session,(database,actor,input)|async move {
        shared::lock(database,"rbac",session).await?;
        let mut role=database.collection::<RoleRecord>("roles").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if role.protected{return Err(fail(Failure::Conflict("系统 Owner 角色不可修改")));}
        if role.version!=input.version{return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));}
        if let Some(name)=&input.name {role.name=name.clone();}
if let Some(slug)=&input.slug{role.slug=slug.clone();}
if let Some(permissions)=&input.permissions{role.permissions=permissions.clone();}
if let Some(scope)=input.data_scope{role.data_scope=scope;}
        validate(actor,&role.name,&role.slug,&role.permissions,role.data_scope).map_err(|error|match error{AppError::Forbidden=>fail(Failure::Forbidden),_=>fail(Failure::Validation("角色参数或权限代码无效"))})?;
        role.version=role.version.checked_add(1).ok_or_else(||fail(Failure::Conflict("版本号已达上限")))?;role.updated_at=DateTime::now();
        database.collection::<Document>("roles").update_one(doc!{"_id":id},doc!{"$set":{"name":&role.name,"slug":&role.slug,"permissions":role.permissions.clone(),"dataScope":role.data_scope.as_str(),"version":role.version,"updatedAt":role.updated_at}}).session(&mut *session).await?;
        shared::audit(database,actor,"roles.update",id,session).await?;Ok(role)
    }.boxed()).await?;
    result.view()
}

pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    if version < 1 {
        return Err(AppError::Validation("版本号无效".into()));
    }
    transaction::run(database,(database.clone(),actor),|session,(database,actor)|async move {
        shared::lock(database,"rbac",session).await?;
        let role=database.collection::<RoleRecord>("roles").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if role.protected{return Err(fail(Failure::Conflict("系统 Owner 角色不可删除")));}
if role.version!=version{return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));}
        if database.collection::<Document>("users").count_documents(doc!{"roleIds":id,"deletedAt":null}).session(&mut *session).await?>0{return Err(fail(Failure::Conflict("角色仍被用户使用")));}
        database.collection::<Document>("roles").update_one(doc!{"_id":id},doc!{"$set":{"deletedAt":DateTime::now(),"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;
        shared::audit(database,actor,"roles.delete",id,session).await?;Ok(())
    }.boxed()).await
}
