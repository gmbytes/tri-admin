use super::{import_model::TargetState, models::Bundle, preview};
use crate::common::transaction::{Failure, fail};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

pub async fn lock_targets(
    database: &Database,
    bundle: &Bundle,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    if !bundle.resources.menus.is_empty() {
        crate::modules::rbac::shared::lock(database, "menus", session).await?;
    }
    // This internal write marker serializes with normal edits without changing the
    // configuration version or the dictionary fence included in the preview hash.
    for (collection, keys) in [
        (
            "dictionaries",
            bundle
                .resources
                .dictionaries
                .iter()
                .map(|row| row.key.clone())
                .collect::<Vec<_>>(),
        ),
        (
            "parameters",
            bundle
                .resources
                .parameters
                .iter()
                .map(|row| row.key.clone())
                .collect::<Vec<_>>(),
        ),
    ] {
        if !keys.is_empty() {
            database
                .collection::<Document>(collection)
                .update_many(
                    doc! {"key":{"$in":keys},"deletedAt":null},
                    doc! {"$set":{"importLock":ObjectId::new()}},
                )
                .session(&mut *session)
                .await?;
        }
    }
    Ok(())
}

async fn row(
    database: &Database,
    collection: &str,
    id: ObjectId,
    expected: Option<i64>,
    mut fields: Document,
    now: DateTime,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let records = database.collection::<Document>(collection);
    fields.insert("updatedAt", now);
    if let Some(version) = expected {
        let next = version
            .checked_add(1)
            .ok_or_else(|| fail(Failure::Conflict("版本号已达上限")))?;
        fields.insert("version", next);
        let result = records
            .update_one(
                doc! {"_id":id,"version":version,"deletedAt":null},
                doc! {"$set":fields},
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(fail(Failure::Conflict("目标配置已改变，请重新预览")));
        }
    } else {
        fields.extend(doc! {"_id":id,"version":1i64,"createdAt":now,"deletedAt":null});
        records.insert_one(fields).session(session).await?;
    }
    Ok(())
}

pub async fn apply(
    database: &Database,
    bundle: &Bundle,
    state: &TargetState,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let now = DateTime::now();
    let ids = preview::proposed_menu_ids(bundle, state)
        .map_err(|_| fail(Failure::Validation("导入后的菜单图无效")))?;
    for desired in &bundle.resources.menus {
        let old = state
            .menus
            .iter()
            .find(|row| row.config.path == desired.path);
        if old.is_some_and(|row| row.config == *desired) {
            continue;
        }
        let parent = desired.parent_path.as_ref().map(|path| ids[path]);
        row(database,"menus",ids[&desired.path],old.map(|row|row.version),doc!{"label":&desired.label,"path":&desired.path,"parentId":parent,"viewKey":desired.view_key.clone(),"icon":desired.icon.clone(),"permission":desired.permission.clone(),"sort":desired.sort,"enabled":desired.enabled},now,session).await?;
    }
    for desired in &bundle.resources.dictionaries {
        let old = state
            .dictionaries
            .iter()
            .find(|row| row.config.key == desired.key);
        let id = old.map(|row| row.id).unwrap_or_else(ObjectId::new);
        let metadata_changed = old.is_none_or(|row| {
            row.config.name != desired.name
                || row.config.description != desired.description
                || row.config.enabled != desired.enabled
        });
        let items_changed = desired.items.iter().any(|item| {
            old.and_then(|row| row.items.iter().find(|row| row.config.value == item.value))
                .is_none_or(|row| row.config != *item)
        });
        if metadata_changed {
            let fence = old
                .map(|row| row.fence)
                .unwrap_or(0)
                .checked_add(i64::from(old.is_some() || items_changed))
                .ok_or_else(|| fail(Failure::Conflict("字典写入版本已达上限")))?;
            row(database,"dictionaries",id,old.map(|row|row.version),doc!{"name":&desired.name,"key":&desired.key,"description":&desired.description,"enabled":desired.enabled,"writeFence":fence},now,session).await?;
        } else if items_changed {
            let fence = old
                .unwrap()
                .fence
                .checked_add(1)
                .ok_or_else(|| fail(Failure::Conflict("字典写入版本已达上限")))?;
            database
                .collection::<Document>("dictionaries")
                .update_one(doc! {"_id":id}, doc! {"$set":{"writeFence":fence}})
                .session(&mut *session)
                .await?;
        }
        for item in &desired.items {
            let previous =
                old.and_then(|row| row.items.iter().find(|row| row.config.value == item.value));
            if previous.is_some_and(|row| row.config == *item) {
                continue;
            }
            row(database,"dictionary_items",previous.map(|row|row.id).unwrap_or_else(ObjectId::new),previous.map(|row|row.version),doc!{"dictionaryId":id,"label":&item.label,"value":&item.value,"sort":item.sort,"enabled":item.enabled},now,session).await?;
        }
    }
    for desired in &bundle.resources.parameters {
        let old = state
            .parameters
            .iter()
            .find(|row| row.config.key == desired.key);
        if old.is_some_and(|row| row.config == *desired) {
            continue;
        }
        let value = crate::modules::parameters::service::validate(
            &desired.name,
            &desired.key,
            &desired.group,
            &desired.description,
            desired.value_type,
            &desired.value,
        )
        .map_err(|_| fail(Failure::Validation("参数值无效")))?;
        row(database,"parameters",old.map(|row|row.id).unwrap_or_else(ObjectId::new),old.map(|row|row.version),doc!{"name":&desired.name,"key":&desired.key,"group":&desired.group,"description":&desired.description,"valueType":desired.value_type.as_str(),"valueJson":value},now,session).await?;
    }
    Ok(())
}
