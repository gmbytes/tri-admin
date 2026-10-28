use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::{
        menus::service::validate_graph,
        rbac::{permissions, shared},
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{Bson, DateTime, Document, doc, oid::ObjectId},
};
use std::collections::{HashMap, HashSet};
pub const VERSION: i32 = 2;
const VIEWS: [&str; 14] = [
    "dictionaries",
    "parameters",
    "settings",
    "exports",
    "tokens",
    "apps",
    "codegen",
    "skills",
    "mcp",
    "ai",
    "workflows",
    "versions",
    "customers",
    "clients",
];
const PATHS: [&str; 14] = [
    "/dictionaries",
    "/parameters",
    "/settings",
    "/data-tools",
    "/api-tokens",
    "/applications",
    "/codegen",
    "/skills",
    "/mcp",
    "/ai",
    "/workflows",
    "/versions",
    "/customers",
    "/clients",
];
#[derive(Clone)]
struct Node {
    id: ObjectId,
    parent: Option<ObjectId>,
    version: i64,
    active: bool,
    retired: bool,
}
fn node(row: Document) -> mongodb::error::Result<Node> {
    let parent = match row.get("parentId") {
        None | Some(Bson::Null) => None,
        Some(Bson::ObjectId(value)) => Some(*value),
        _ => return Err(fail(Failure::NotReady)),
    };
    Ok(Node {
        id: row
            .get_object_id("_id")
            .map_err(|_| fail(Failure::NotReady))?,
        parent,
        version: row
            .get_i64("version")
            .or_else(|_| row.get_i32("version").map(i64::from))
            .unwrap_or(0),
        active: row
            .get("deletedAt")
            .is_none_or(|value| matches!(value, Bson::Null)),
        retired: row
            .get_str("viewKey")
            .is_ok_and(|value| VIEWS.contains(&value))
            || row
                .get_str("path")
                .is_ok_and(|value| PATHS.contains(&value)),
    })
}
async fn repair_retained_parents(
    db: &Database,
    session: &mut ClientSession,
    now: DateTime,
) -> mongodb::error::Result<()> {
    let collection = db.collection::<Document>("menus");
    let projection = doc! {"_id":1,"parentId":1,"version":1,"deletedAt":1,"viewKey":1,"path":1};
    let mut cursor = collection
        .find(doc! {"deletedAt":null,"$nor":[{"viewKey":{"$in":VIEWS}},{"path":{"$in":PATHS}}]})
        .projection(projection.clone())
        .limit(1001)
        .session(&mut *session)
        .await?;
    let rows = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
    if rows.len() > 1000 {
        return Err(fail(Failure::Conflict("菜单数量已达上限")));
    }
    let mut current = HashMap::new();
    for row in rows {
        let value = node(row)?;
        current.insert(value.id, value);
    }
    let mut cache = current.clone();
    let mut asked = HashSet::new();
    // Bounded batched ancestor reads include soft-deleted v1 parents, not all menu history.
    for _ in 0..16 {
        let missing = cache
            .values()
            .filter_map(|value| value.parent)
            .filter(|id| !cache.contains_key(id) && !asked.contains(id))
            .collect::<HashSet<_>>();
        if missing.is_empty() {
            break;
        }
        asked.extend(missing.iter().copied());
        let mut cursor = collection
            .find(doc! {"_id":{"$in":missing.into_iter().collect::<Vec<_>>()}})
            .projection(projection.clone())
            .session(&mut *session)
            .await?;
        let rows = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        for row in rows {
            let value = node(row)?;
            cache.insert(value.id, value);
        }
    }
    let mut parents = HashMap::new();
    for value in current.values() {
        let mut parent = value.parent;
        let mut seen = HashSet::from([value.id]);
        while let Some(id) = parent {
            if !seen.insert(id) {
                return Err(fail(Failure::Conflict("菜单不能形成循环")));
            }
            let Some(ancestor) = cache.get(&id) else {
                if !asked.contains(&id) {
                    return Err(fail(Failure::Conflict("菜单层级不能超过16层")));
                }
                parent = None;
                break;
            };
            if ancestor.active && !ancestor.retired {
                break;
            }
            parent = ancestor.parent;
        }
        parents.insert(value.id, parent);
    }
    validate_graph(&parents)?;
    for value in current.values() {
        let parent = parents[&value.id];
        if parent == value.parent {
            continue;
        }
        if value.version < 1 || value.version == i64::MAX {
            return Err(fail(Failure::Conflict("菜单版本号无效")));
        }
        let result = collection
            .update_one(
                doc! {"_id":value.id,"deletedAt":null,"version":value.version},
                doc! {"$set":{"parentId":parent,"updatedAt":now},"$inc":{"version":1i64}},
            )
            .session(&mut *session)
            .await?;
        if result.matched_count != 1 {
            return Err(fail(Failure::Conflict("菜单已变化，请重试")));
        }
    }
    Ok(())
}
pub async fn migrate(db: &Database) -> Result<(), AppError> {
    if let Some(row) = db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"game-scope"})
        .await?
        && !row
            .get_i32("version")
            .is_ok_and(|version| (1..=VERSION).contains(&version))
    {
        return Err(AppError::SchemaMismatch);
    }
    transaction::run(db,db.clone(),|session,db|async move{
        shared::lock(db,"rbac",session).await?;shared::lock(db,"menus",session).await?;
        let marker=db.collection::<Document>("_schema_migrations").find_one(doc!{"_id":"game-scope"}).session(&mut *session).await?;
        let previous=marker.as_ref().map(|row|row.get_i32("version")).transpose().map_err(|_|fail(Failure::NotReady))?;
        if previous==Some(VERSION){return Ok(())}
        if previous.is_some_and(|version|version!=1){return Err(fail(Failure::NotReady))}
        let now=DateTime::now();
        repair_retained_parents(db,session,now).await?;
        db.collection::<Document>("menus").update_many(doc!{"deletedAt":null,"$or":[{"viewKey":{"$in":VIEWS}},{"path":{"$in":PATHS}}]},doc!{"$set":{"enabled":false,"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;
        let mut allowed=vec!["*"];allowed.extend(permissions::CATALOG.iter().map(|permission|permission.code));
        db.collection::<Document>("roles").update_many(doc!{"permissions":{"$elemMatch":{"$nin":&allowed}}},doc!{"$pull":{"permissions":{"$nin":&allowed}},"$set":{"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;
        db.collection::<Document>("_schema_migrations").update_one(doc!{"_id":"game-scope"},doc!{"$set":{"version":VERSION,"updatedAt":now},"$setOnInsert":{"createdAt":now}}).upsert(true).session(session).await?;
        Ok(())
    }.boxed()).await
}
