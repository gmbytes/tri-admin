use super::dto::{CreateMenu, PatchMenu};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
        validation,
    },
    modules::{
        auth::AuthContext,
        rbac::{
            models::{MenuRecord, MenuView},
            permissions,
            shared::{self, Actor},
        },
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use std::collections::{HashMap, HashSet};

const VIEWS: &[&str] = &[
    "dashboard",
    "users",
    "roles",
    "menus",
    "permissions",
    "dictionaries",
    "parameters",
    "audit",
    "files",
    "exports",
    "apps",
    "tokens",
    "codegen",
    "skills",
    "mcp",
    "workflows",
    "gameServers",
    "gameJobs",
    "settings",
    "versions",
    "customers",
];

fn validate(
    label: &str,
    path: &str,
    view: &Option<String>,
    icon: &Option<String>,
    permission: &Option<String>,
) -> Result<(), AppError> {
    validation::text(label, "菜单名", 80)?;
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.chars().count() > 128
        || !path
            .chars()
            .all(|c| c.is_alphanumeric() || "/_-".contains(c))
    {
        return Err(AppError::Validation("菜单路径格式无效".into()));
    }
    if view
        .as_ref()
        .is_some_and(|view| !VIEWS.contains(&view.as_str()))
    {
        return Err(AppError::Validation("页面标识不在注册表中".into()));
    }
    if icon.as_ref().is_some_and(|icon| {
        icon.is_empty()
            || icon.len() > 40
            || !icon.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) {
        return Err(AppError::Validation("图标标识无效".into()));
    }
    if permission
        .as_ref()
        .is_some_and(|code| !permissions::known(code))
    {
        return Err(AppError::Validation("菜单权限代码无效".into()));
    }
    Ok(())
}

async fn validate_parent(
    database: &Database,
    id: ObjectId,
    parent: Option<ObjectId>,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let mut cursor = database
        .collection::<MenuRecord>("menus")
        .find(doc! {"deletedAt":null})
        .session(&mut *session)
        .await?;
    let nodes = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
    let mut parents = nodes
        .into_iter()
        .map(|node| (node.id, node.parent_id))
        .collect::<HashMap<_, _>>();
    parents.insert(id, parent);
    for start in parents.keys() {
        let mut next = Some(*start);
        let mut seen = HashSet::new();
        let mut depth = 0;
        while let Some(node) = next {
            if !seen.insert(node) {
                return Err(fail(Failure::Conflict("菜单不能形成循环")));
            }
            depth += 1;
            if depth > 16 {
                return Err(fail(Failure::Conflict("菜单层级不能超过 16 层")));
            }
            next = *parents
                .get(&node)
                .ok_or_else(|| fail(Failure::Validation("父菜单不存在")))?;
        }
    }
    Ok(())
}

pub async fn list(database: &Database) -> Result<Vec<MenuView>, AppError> {
    Ok(database
        .collection::<MenuRecord>("menus")
        .find(doc! {"deletedAt":null})
        .sort(doc! {"sort":1,"_id":1})
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(MenuRecord::view)
        .collect())
}

pub async fn navigation(
    database: &Database,
    context: &AuthContext,
) -> Result<Vec<MenuView>, AppError> {
    fn tree(
        parent: Option<&str>,
        nodes: &[MenuView],
        context: &AuthContext,
        depth: usize,
    ) -> Result<Vec<MenuView>, AppError> {
        if depth > 16 {
            return Err(AppError::Internal);
        }
        let mut result = vec![];
        for node in nodes
            .iter()
            .filter(|node| node.parent_id.as_deref() == parent)
        {
            if !node.enabled
                || node
                    .permission
                    .as_ref()
                    .is_some_and(|permission| !context.can(permission))
            {
                continue;
            }
            let mut node = node.clone();
            node.children = tree(Some(&node.id), nodes, context, depth + 1)?;
            if node.view_key.is_some() || !node.children.is_empty() {
                result.push(node);
            }
        }
        Ok(result)
    }
    tree(None, &list(database).await?, context, 0)
}

pub async fn create(
    database: &Database,
    actor: Actor,
    input: CreateMenu,
) -> Result<MenuView, AppError> {
    validate(
        &input.label,
        &input.path,
        &input.view_key,
        &input.icon,
        &input.permission,
    )?;
    let parent = input
        .parent_id
        .as_ref()
        .map(|id| validation::object_id(id))
        .transpose()?;
    let id = ObjectId::new();
    let result=transaction::run(database,(database.clone(),actor,input),|session,(database,actor,input)|async move{
        shared::lock(database,"menus",session).await?;validate_parent(database,id,parent,session).await?;
        if database.collection::<Document>("menus").count_documents(doc!{"deletedAt":null}).session(&mut *session).await?>=1000{return Err(fail(Failure::Conflict("菜单数量已达上限")));}
        let now=DateTime::now();
        database.collection::<Document>("menus").insert_one(doc!{"_id":id,"label":&input.label,"path":&input.path,"viewKey":input.view_key.clone(),"icon":input.icon.clone(),"permission":input.permission.clone(),"parentId":parent,"sort":input.sort,"enabled":input.enabled,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).session(&mut *session).await?;
        shared::audit(database,actor,"menus.create",id,session).await?;
        Ok(MenuRecord{id,label:input.label.clone(),path:input.path.clone(),view_key:input.view_key.clone(),icon:input.icon.clone(),permission:input.permission.clone(),parent_id:parent,sort:input.sort,enabled:input.enabled,version:1})
    }.boxed()).await?;
    Ok(result.view())
}

pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: PatchMenu,
) -> Result<MenuView, AppError> {
    if input.version < 1 {
        return Err(AppError::Validation("版本号无效".into()));
    }
    let parent = input
        .parent_id
        .as_ref()
        .map(|value| {
            value
                .as_ref()
                .map(|id| validation::object_id(id))
                .transpose()
        })
        .transpose()?;
    let result=transaction::run(database,(database.clone(),actor,input),|session,(database,actor,input)|async move{
        shared::lock(database,"menus",session).await?;
        let mut menu=database.collection::<MenuRecord>("menus").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if menu.version!=input.version{return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));}
        if let Some(label)=&input.label{menu.label=label.clone();}
if let Some(path)=&input.path{menu.path=path.clone();}
if let Some(view)=&input.view_key{menu.view_key=view.clone();}
if let Some(icon)=&input.icon{menu.icon=icon.clone();}
if let Some(permission)=&input.permission{menu.permission=permission.clone();}
        if let Some(parent)=parent{menu.parent_id=parent;}
if let Some(sort)=input.sort{menu.sort=sort;}
if let Some(enabled)=input.enabled{menu.enabled=enabled;}
        validate(&menu.label,&menu.path,&menu.view_key,&menu.icon,&menu.permission).map_err(|_|fail(Failure::Validation("菜单参数无效")))?;
        validate_parent(database,id,menu.parent_id,session).await?;menu.version=menu.version.checked_add(1).ok_or_else(||fail(Failure::Conflict("版本号已达上限")))?;
        database.collection::<Document>("menus").update_one(doc!{"_id":id},doc!{"$set":{"label":&menu.label,"path":&menu.path,"viewKey":menu.view_key.clone(),"icon":menu.icon.clone(),"permission":menu.permission.clone(),"parentId":menu.parent_id,"sort":menu.sort,"enabled":menu.enabled,"version":menu.version,"updatedAt":DateTime::now()}}).session(&mut *session).await?;
        shared::audit(database,actor,"menus.update",id,session).await?;Ok(menu)
    }.boxed()).await?;
    Ok(result.view())
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
    transaction::run(database,(database.clone(),actor),|session,(database,actor)|async move{
        shared::lock(database,"menus",session).await?;let menu=database.collection::<MenuRecord>("menus").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if menu.version!=version{return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));}
        if database.collection::<Document>("menus").count_documents(doc!{"parentId":id,"deletedAt":null}).session(&mut *session).await?>0{return Err(fail(Failure::Conflict("请先删除或移动子菜单")));}
        database.collection::<Document>("menus").update_one(doc!{"_id":id},doc!{"$set":{"deletedAt":DateTime::now(),"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;shared::audit(database,actor,"menus.delete",id,session).await?;Ok(())
    }.boxed()).await
}
