use super::{
    dto::{CategoryCreate, CategoryPatch},
    models::{Category, CategoryView},
    validate,
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared::{self, Actor},
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

pub async fn list(database: &Database, input: PageInput) -> Result<Page<CategoryView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "code", "name"])?;
    let mut filter = doc! {
    "deletedAt":null};
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        filter.insert(
            "code",
            doc! {
            "$regex":validate::prefix(search)},
        );
    }
    let total = database
        .collection::<Document>("file_categories")
        .count_documents(filter.clone())
        .await?;
    let data = database
        .collection::<Category>("file_categories")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(Category::view)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(data, &input, total))
}

pub async fn get(database: &Database, id: ObjectId) -> Result<CategoryView, AppError> {
    database
        .collection::<Category>("file_categories")
        .find_one(doc! {
                "_id":id,
        "deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}

pub async fn create(
    database: &Database,
    actor: Actor,
    input: CategoryCreate,
) -> Result<CategoryView, AppError> {
    validate::category(&input.name, &input.code, &input.description)?;
    let id = ObjectId::new();
    transaction::run(
        database,
        (database.clone(), actor, input),
        |session, (database, actor, input)| {
            async move {
                shared::lock(database, "files", session).await?;
                let now = DateTime::now();
                database
                    .collection::<Document>("file_categories")
                    .insert_one(doc! {
                    "_id":id,
                    "name":&input.name,
                    "code":&input.code,
                    "description":&input.description,
                    "version":1i64,
                    "createdAt":now,
                    "updatedAt":now,
                    "deletedAt":null})
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "fileCategories.create", id, session).await?;
                database
                    .collection::<Category>("file_categories")
                    .find_one(doc! {
                    "_id":id})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))
            }
            .boxed()
        },
    )
    .await?
    .view()
}

pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: CategoryPatch,
) -> Result<CategoryView, AppError> {
    validate::version(input.version)?;
    transaction::run(
        database,
        (database.clone(), actor, input),
        |session, (database, actor, input)| {
            async move {
                shared::lock(database, "files", session).await?;
                let mut row = database
                    .collection::<Category>("file_categories")
                    .find_one(doc! {
                                        "_id":id,
                    "deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("分类已被修改，请刷新后重试")));
                }
                if let Some(value) = &input.name {
                    row.name = value.clone()
                }
                if let Some(value) = &input.description {
                    row.description = value.clone()
                }
                validate::category(&row.name, &row.code, &row.description)
                    .map_err(|_| fail(Failure::Validation("分类字段无效")))?;
                row.version += 1;
                row.updated_at = DateTime::now();
                database
                    .collection::<Document>("file_categories")
                    .update_one(
                        doc! {
                        "_id":id},
                        doc! {
                        "$set":{
                        "name":&row.name,
                        "description":&row.description,
                        "updatedAt":row.updated_at}
                        ,
                        "$inc":{
                        "version":1i64}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "fileCategories.update", id, session).await?;
                Ok(row)
            }
            .boxed()
        },
    )
    .await?
    .view()
}

pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    validate::version(version)?;
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<Category>("file_categories")
                    .find_one(doc! {
                                        "_id":id,
                    "deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if row.version != version {
                    return Err(fail(Failure::Conflict("分类已被修改，请刷新后重试")));
                }
                if database
                    .collection::<Document>("files")
                    .find_one(doc! {
                                        "categoryId":id,
                    "deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .is_some()
                    || database
                        .collection::<Document>("file_uploads")
                        .find_one(doc! {
                        "categoryId":id,
                        "state":{
                        "$nin":["cancelled",
                        "failed",
                        "completed"]}
                        })
                        .session(&mut *session)
                        .await?
                        .is_some()
                {
                    return Err(fail(Failure::Conflict("分类仍被文件或上传任务引用")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("file_categories")
                    .update_one(
                        doc! {
                        "_id":id},
                        doc! {
                        "$set":{
                        "deletedAt":now,
                        "updatedAt":now}
                        ,
                        "$inc":{
                        "version":1i64}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "fileCategories.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
