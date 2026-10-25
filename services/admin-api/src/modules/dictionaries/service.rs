use super::{
    dto::{CreateDictionary, CreateItem, PatchDictionary, PatchItem},
    models::{DictionaryRecord, DictionaryView, ItemRecord, ItemView},
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::shared::{self, Actor},
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

pub(crate) fn validate_dictionary(
    name: &str,
    key: &str,
    description: &str,
) -> Result<(), AppError> {
    validation::text(name, "字典名称", 80)?;
    if key.is_empty()
        || key.len() > 64
        || !key.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        || !key
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
    {
        return Err(AppError::Validation(
            "字典标识需为 1–64 位小写字母开头的字母、数字、点、下划线或短横线".into(),
        ));
    }
    if description.chars().count() > 1000 {
        return Err(AppError::Validation("描述最多 1000 个字符".into()));
    }
    Ok(())
}
pub(crate) fn validate_item(label: &str, value: &str) -> Result<(), AppError> {
    validation::text(label, "明细名称", 100)?;
    validation::text(value, "明细值", 200)
}
fn validate_version(version: i64) -> Result<(), AppError> {
    if !(1..i64::MAX).contains(&version) {
        return Err(AppError::Validation("版本号无效".into()));
    }
    Ok(())
}
fn search_filter(input: &PageInput, mut filter: Document, field: &str) -> Document {
    if let Some(value) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut prefix = "^".to_owned();
        for character in value.chars() {
            if ".+*?^${}()|[]\\".contains(character) {
                prefix.push('\\');
            }
            prefix.push(character);
        }
        filter.insert(field, doc! {"$regex":prefix});
    }
    filter
}
pub async fn list(database: &Database, input: PageInput) -> Result<Page<DictionaryView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "key", "name"])?;
    let filter = search_filter(&input, doc! {"deletedAt":null}, "key");
    let total = database
        .collection::<Document>("dictionaries")
        .count_documents(filter.clone())
        .await?;
    let data = database
        .collection::<DictionaryRecord>("dictionaries")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(DictionaryRecord::view)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(data, &input, total))
}
pub async fn get(database: &Database, id: ObjectId) -> Result<DictionaryView, AppError> {
    database
        .collection::<DictionaryRecord>("dictionaries")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
pub async fn create(
    database: &Database,
    actor: Actor,
    input: CreateDictionary,
) -> Result<DictionaryView, AppError> {
    validate_dictionary(&input.name, &input.key, &input.description)?;
    let id = ObjectId::new();
    transaction::run(database, (database.clone(), actor, input), |session, (database, actor, input)| async move {
        let now = DateTime::now();
        database.collection::<Document>("dictionaries").insert_one(doc!{"_id":id,"name":&input.name,"key":&input.key,"description":&input.description,"enabled":input.enabled,"version":1i64,"writeFence":0i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).session(&mut *session).await?;
        shared::audit(database, actor, "dictionaries.create", id, session).await?;
        database.collection::<DictionaryRecord>("dictionaries").find_one(doc!{"_id":id}).session(session).await?.ok_or_else(||fail(Failure::NotFound))
    }.boxed()).await?.view()
}
async fn lock_parent(
    database: &Database,
    id: ObjectId,
    session: &mut ClientSession,
) -> mongodb::error::Result<DictionaryRecord> {
    let result = database
        .collection::<Document>("dictionaries")
        .update_one(
            doc! {"_id":id,"deletedAt":null},
            doc! {"$inc":{"writeFence":1i64}},
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(fail(Failure::NotFound));
    }
    database
        .collection::<DictionaryRecord>("dictionaries")
        .find_one(doc! {"_id":id})
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))
}
pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: PatchDictionary,
) -> Result<DictionaryView, AppError> {
    validate_version(input.version)?;
    transaction::run(database,(database.clone(), actor, input), |session,(database,actor,input)| async move {
        let mut record = lock_parent(database, id, session).await?;
        if record.stamp.version != input.version { return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试"))); }
        if let Some(name) = &input.name { record.name = name.clone(); }
        if let Some(key) = &input.key { record.key = key.clone(); }
        if let Some(description) = &input.description { record.description = description.clone(); }
        if let Some(enabled) = input.enabled { record.enabled = enabled; }
        validate_dictionary(&record.name, &record.key, &record.description).map_err(|_| fail(Failure::Validation("字典名称、标识或描述无效")))?;
        record.stamp.version += 1;
        record.stamp.updated_at = DateTime::now();
        database.collection::<Document>("dictionaries").update_one(doc!{"_id":id}, doc!{"$set":{"name":&record.name,"key":&record.key,"description":&record.description,"enabled":record.enabled,"version":record.stamp.version,"updatedAt":record.stamp.updated_at}}).session(&mut *session).await?;
        shared::audit(database, actor, "dictionaries.update", id, session).await?;
        Ok(record)
    }.boxed()).await?.view()
}
pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    validate_version(version)?;
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                let record = lock_parent(database, id, session).await?;
                if record.stamp.version != version {
                    return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));
                }
                if database
                    .collection::<Document>("dictionary_items")
                    .count_documents(doc! {"dictionaryId":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    > 0
                {
                    return Err(fail(Failure::Conflict("请先删除字典中的活动明细")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("dictionaries")
                    .update_one(
                        doc! {"_id":id},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "dictionaries.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
pub async fn list_items(
    database: &Database,
    parent: ObjectId,
    input: PageInput,
) -> Result<Page<ItemView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "sort", "label", "value"])?;
    get(database, parent).await?;
    let filter = search_filter(
        &input,
        doc! {"dictionaryId":parent,"deletedAt":null},
        "value",
    );
    let total = database
        .collection::<Document>("dictionary_items")
        .count_documents(filter.clone())
        .await?;
    let data = database
        .collection::<ItemRecord>("dictionary_items")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(ItemRecord::view)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(data, &input, total))
}
pub async fn create_item(
    database: &Database,
    actor: Actor,
    parent: ObjectId,
    input: CreateItem,
) -> Result<ItemView, AppError> {
    validate_item(&input.label, &input.value)?;
    let id = ObjectId::new();
    transaction::run(database,(database.clone(),actor,input),|session,(database,actor,input)|async move {
        lock_parent(database,parent,session).await?;
        let now = DateTime::now();
        database.collection::<Document>("dictionary_items").insert_one(doc!{"_id":id,"dictionaryId":parent,"label":&input.label,"value":&input.value,"sort":input.sort,"enabled":input.enabled,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).session(&mut *session).await?;
        shared::audit(database,actor,"dictionaries.items.create",id,session).await?;
        database.collection::<ItemRecord>("dictionary_items").find_one(doc!{"_id":id}).session(session).await?.ok_or_else(||fail(Failure::NotFound))
    }.boxed()).await?.view()
}
pub async fn patch_item(
    database: &Database,
    actor: Actor,
    parent: ObjectId,
    id: ObjectId,
    input: PatchItem,
) -> Result<ItemView, AppError> {
    validate_version(input.version)?;
    transaction::run(database,(database.clone(),actor,input),|session,(database,actor,input)|async move {
        lock_parent(database,parent,session).await?;
        let mut record = database.collection::<ItemRecord>("dictionary_items").find_one(doc!{"_id":id,"dictionaryId":parent,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if record.stamp.version != input.version { return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试"))); }
        if let Some(label) = &input.label { record.label = label.clone(); }
        if let Some(value) = &input.value { record.value = value.clone(); }
        if let Some(sort) = input.sort { record.sort = sort; }
        if let Some(enabled) = input.enabled { record.enabled = enabled; }
        validate_item(&record.label,&record.value).map_err(|_|fail(Failure::Validation("明细名称或值无效")))?;
        record.stamp.version += 1;
        record.stamp.updated_at = DateTime::now();
        database.collection::<Document>("dictionary_items").update_one(doc!{"_id":id},doc!{"$set":{"label":&record.label,"value":&record.value,"sort":record.sort,"enabled":record.enabled,"version":record.stamp.version,"updatedAt":record.stamp.updated_at}}).session(&mut *session).await?;
        shared::audit(database,actor,"dictionaries.items.update",id,session).await?;
        Ok(record)
    }.boxed()).await?.view()
}
pub async fn delete_item(
    database: &Database,
    actor: Actor,
    parent: ObjectId,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    validate_version(version)?;
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                lock_parent(database, parent, session).await?;
                let record = database
                    .collection::<ItemRecord>("dictionary_items")
                    .find_one(doc! {"_id":id,"dictionaryId":parent,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if record.stamp.version != version {
                    return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("dictionary_items")
                    .update_one(
                        doc! {"_id":id},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "dictionaries.items.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
