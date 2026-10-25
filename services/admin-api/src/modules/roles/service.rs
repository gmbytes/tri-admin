use super::dto::{CreateRole, PatchRole, RoleChoice};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::{
        models::{RoleBrief, RoleRecord, RoleView},
        permissions,
        shared::{self, Actor},
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use std::collections::{HashMap, HashSet};

fn validate(
    actor: &Actor,
    name: &str,
    slug: &str,
    codes: &[String],
    scope: DataScope,
    ids: &[ObjectId],
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
    if scope == DataScope::Roles && ids.is_empty() || scope != DataScope::Roles && !ids.is_empty() {
        return Err(AppError::Validation(
            "指定角色范围必须选择角色，其他范围不能携带角色选择".into(),
        ));
    }
    if codes.iter().any(|code| !actor.can(code)) || !actor.can_grant_scope(scope, ids) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}
fn scope_ids(scope: DataScope, raw: &[String]) -> Result<Vec<ObjectId>, AppError> {
    if scope == DataScope::Roles {
        shared::ids(raw)
    } else if raw.is_empty() {
        Ok(vec![])
    } else {
        Err(AppError::Validation("当前数据范围不能选择角色".into()))
    }
}
async fn check_references(
    database: &Database,
    ids: &[ObjectId],
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    if !ids.is_empty()
        && database
            .collection::<Document>("roles")
            .count_documents(doc! {"_id":{"$in":ids},"deletedAt":null})
            .session(session)
            .await?
            != ids.len() as u64
    {
        return Err(fail(Failure::Validation("范围引用的角色不存在或已删除")));
    }
    Ok(())
}
async fn enrich(database: &Database, views: &mut [RoleView]) -> Result<(), AppError> {
    let ids = views
        .iter()
        .flat_map(|view| &view.data_scope_role_ids)
        .map(|id| validation::object_id(id))
        .collect::<Result<HashSet<_>, _>>()?;
    if ids.is_empty() {
        return Ok(());
    }
    let ids: Vec<_> = ids.into_iter().collect();
    let records = database
        .collection::<Document>("roles")
        .find(doc! {"_id":{"$in":ids},"deletedAt":null})
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let mut names = HashMap::new();
    for record in records {
        names.insert(
            record
                .get_object_id("_id")
                .map_err(|_| AppError::Internal)?
                .to_hex(),
            record
                .get_str("name")
                .map_err(|_| AppError::Internal)?
                .to_owned(),
        );
    }
    for view in views {
        view.data_scope_roles = view
            .data_scope_role_ids
            .iter()
            .map(|id| {
                Ok(RoleBrief {
                    id: id.clone(),
                    name: names.get(id).cloned().ok_or(AppError::Internal)?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
    }
    Ok(())
}
async fn render(database: &Database, record: RoleRecord) -> Result<RoleView, AppError> {
    let mut view = record.view()?;
    enrich(database, std::slice::from_mut(&mut view)).await?;
    Ok(view)
}
pub async fn list(database: &Database, input: PageInput) -> Result<Page<RoleView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "name", "slug"])?;
    let mut filter = doc! {"deletedAt":null};
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut prefix = "^".to_owned();
        for character in search.to_lowercase().chars() {
            if ".+*?^${}()|[]\\".contains(character) {
                prefix.push('\\');
            }
            prefix.push(character);
        }
        filter.insert("slug", doc! {"$regex":prefix});
    }
    let total = database
        .collection::<Document>("roles")
        .count_documents(filter.clone())
        .await?;
    let mut data = database
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
        .collect::<Result<Vec<_>, _>>()?;
    enrich(database, &mut data).await?;
    Ok(Page::new(data, &input, total))
}
pub async fn assignable(
    database: &Database,
    actor: &Actor,
    input: PageInput,
) -> Result<Page<RoleChoice>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "name", "slug"])?;
    let mut filter = doc! {"deletedAt":null};
    if !actor.can("*") {
        filter.insert("protected", false);
        filter.insert(
            "permissions",
            doc! {"$not":{"$elemMatch":{"$nin":&actor.permissions}}},
        );
    }
    match actor.scope {
        DataScope::All => {}
        DataScope::Own => {
            filter.insert("_id", doc! {"$in":&actor.role_ids});
            filter.insert("dataScope", "self");
        }
        DataScope::Roles => {
            let ids = actor
                .role_ids
                .iter()
                .chain(&actor.scope_role_ids)
                .copied()
                .collect::<Vec<_>>();
            filter.insert("$or",vec![doc!{"dataScope":"self","_id":{"$in":ids}},doc!{"dataScope":"roles","$expr":{"$setIsSubset":["$dataScopeRoleIds",{"$literal":&actor.scope_role_ids}]}}]);
        }
    }
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut prefix = "^".to_owned();
        for character in search.to_lowercase().chars() {
            if ".+*?^${}()|[]\\".contains(character) {
                prefix.push('\\');
            }
            prefix.push(character);
        }
        filter.insert("slug", doc! {"$regex":prefix});
    }
    let total = database
        .collection::<Document>("roles")
        .count_documents(filter.clone())
        .await?;
    let records = database
        .collection::<RoleRecord>("roles")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let data = records
        .into_iter()
        .map(|role| RoleChoice {
            id: role.id.to_hex(),
            name: role.name,
            slug: role.slug,
        })
        .collect();
    Ok(Page::new(data, &input, total))
}
pub async fn create(
    database: &Database,
    actor: Actor,
    input: CreateRole,
) -> Result<RoleView, AppError> {
    let ids = scope_ids(input.data_scope, &input.data_scope_role_ids)?;
    validate(
        &actor,
        &input.name,
        &input.slug,
        &input.permissions,
        input.data_scope,
        &ids,
    )?;
    let id = ObjectId::new();
    let result=transaction::run(database,(database.clone(),actor,input,ids),|session,(database,actor,input,ids)| {
        async move {
            shared::lock(database,"rbac",session).await?;
            check_references(database,ids,session).await?;
            let now=DateTime::now();
            let record=doc!{"_id":id,"name":&input.name,"slug":&input.slug,"permissions":input.permissions.clone(),"dataScope":input.data_scope.as_str(),"dataScopeRoleIds":ids.clone(),"protected":false,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null};
            database.collection::<Document>("roles").insert_one(record).session(&mut *session).await?;
            shared::audit(database,actor,"roles.create",id,session).await?;
            Ok(RoleRecord{id,name:input.name.clone(),slug:input.slug.clone(),permissions:input.permissions.clone(),data_scope:input.data_scope,data_scope_role_ids:ids.clone(),protected:false,version:1,created_at:now,updated_at:now})
        }.boxed()
    }).await?;
    render(database, result).await
}
pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: PatchRole,
) -> Result<RoleView, AppError> {
    if !(1..i64::MAX).contains(&input.version) {
        return Err(AppError::Validation("版本号无效".into()));
    }
    let result=transaction::run(database,(database.clone(),actor,input),|session,(database,actor,input)| {
        async move {
            shared::lock(database,"rbac",session).await?;
            let mut role=database.collection::<RoleRecord>("roles").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
            if role.protected{return Err(fail(Failure::Conflict("系统 Owner 角色不可修改")));}
            if role.version!=input.version{return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));}
            if let Some(name)=&input.name{role.name=name.clone();}
            if let Some(slug)=&input.slug{role.slug=slug.clone();}
            if let Some(codes)=&input.permissions{role.permissions=codes.clone();}
            if let Some(scope)=input.data_scope{role.data_scope=scope;}
            if let Some(ids)=&input.data_scope_role_ids{role.data_scope_role_ids=scope_ids(role.data_scope,ids).map_err(|_|fail(Failure::Validation("数据范围角色选择无效")))?;}
            validate(actor,&role.name,&role.slug,&role.permissions,role.data_scope,&role.data_scope_role_ids).map_err(|error|match error{AppError::Forbidden=>fail(Failure::Forbidden),_=>fail(Failure::Validation("角色字段、权限或数据范围无效"))})?;
            check_references(database,&role.data_scope_role_ids,session).await?;
            role.version+=1;role.updated_at=DateTime::now();
            let fields=doc!{"name":&role.name,"slug":&role.slug,"permissions":role.permissions.clone(),"dataScope":role.data_scope.as_str(),"dataScopeRoleIds":role.data_scope_role_ids.clone(),"version":role.version,"updatedAt":role.updated_at};
            database.collection::<Document>("roles").update_one(doc!{"_id":id},doc!{"$set":fields}).session(&mut *session).await?;
            shared::audit(database,actor,"roles.update",id,session).await?;Ok(role)
        }.boxed()
    }).await?;
    render(database, result).await
}
pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    if !(1..i64::MAX).contains(&version) {
        return Err(AppError::Validation("版本号无效".into()));
    }
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                let role = database
                    .collection::<RoleRecord>("roles")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if role.protected {
                    return Err(fail(Failure::Conflict("系统 Owner 角色不可删除")));
                }
                if role.version != version {
                    return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));
                }
                if database
                    .collection::<Document>("users")
                    .count_documents(doc! {"roleIds":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    > 0
                {
                    return Err(fail(Failure::Conflict("角色仍被用户使用")));
                }
                if database
                    .collection::<Document>("roles")
                    .count_documents(doc! {"dataScopeRoleIds":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    > 0
                {
                    return Err(fail(Failure::Conflict("角色仍被数据范围引用")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("roles")
                    .update_one(
                        doc! {"_id":id},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "roles.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
