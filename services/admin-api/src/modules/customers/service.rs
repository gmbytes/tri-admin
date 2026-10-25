use super::{
    dto::{CreateCustomer, CustomerQuery, PatchCustomer},
    models::{CustomerRecord, CustomerView, OwnerRecord, OwnerView},
};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::{
        models::UserRecord,
        shared::{self, Actor},
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

fn validate(
    code: &str,
    name: &str,
    contact: &str,
    phone: &str,
    email: &str,
    note: &str,
) -> Result<(), AppError> {
    if code.is_empty()
        || code.len() > 64
        || !code
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(AppError::Validation(
            "客户编号需为 1–64 位字母、数字、点、下划线或短横线".into(),
        ));
    }
    validation::text(name, "客户名称", 80)?;
    if contact.chars().count() > 80
        || phone.chars().count() > 32
        || email.len() > 254
        || note.chars().count() > 2000
    {
        return Err(AppError::Validation(
            "联系人、电话、邮箱或备注长度超限".into(),
        ));
    }
    if !email.is_empty()
        && (email.chars().any(char::is_whitespace)
            || !email.split_once('@').is_some_and(|(local, host)| {
                !local.is_empty() && !host.is_empty() && !host.contains('@')
            }))
    {
        return Err(AppError::Validation("邮箱格式无效".into()));
    }
    Ok(())
}
fn version(value: i64) -> Result<(), AppError> {
    if !(1..i64::MAX).contains(&value) {
        return Err(AppError::Validation("版本号无效".into()));
    }
    Ok(())
}
fn prefix(value: &str) -> String {
    let mut result = "^".to_owned();
    for character in value.chars() {
        if ".+*?^${}()|[]\\".contains(character) {
            result.push('\\');
        }
        result.push(character);
    }
    result
}
async fn enrich(database: &Database, views: &mut [CustomerView]) -> Result<(), AppError> {
    let ids = views
        .iter()
        .map(|view| validation::object_id(&view.owner_id))
        .collect::<Result<HashSet<_>, _>>()?;
    if ids.is_empty() {
        return Ok(());
    }
    let owners = database
        .collection::<OwnerRecord>("users")
        .find(doc! {"_id":{"$in":ids.into_iter().collect::<Vec<_>>()}})
        .projection(doc! {"_id":1,"username":1,"displayName":1})
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let owners = owners
        .into_iter()
        .map(|record| {
            let view = record.view();
            (view.id.clone(), view)
        })
        .collect::<HashMap<_, _>>();
    for view in views {
        view.owner = owners.get(&view.owner_id).cloned();
    }
    Ok(())
}
async fn render(database: &Database, record: CustomerRecord) -> Result<CustomerView, AppError> {
    let mut view = record.view()?;
    enrich(database, std::slice::from_mut(&mut view)).await?;
    Ok(view)
}
pub async fn owners(
    database: &Database,
    actor: &Actor,
    input: PageInput,
) -> Result<Page<OwnerView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "username", "displayName"])?;
    let sort: Document = sort
        .into_iter()
        .map(|(field, value)| {
            (
                if field == "username" {
                    "usernameKey".into()
                } else {
                    field
                },
                value,
            )
        })
        .collect();
    let mut filter = doc! {"deletedAt":null,"status":"active"};
    filter.extend(actor.user_filter());
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        filter.insert(
            "usernameKey",
            doc! {"$regex":prefix(&search.to_lowercase())},
        );
    }
    let total = database
        .collection::<Document>("users")
        .count_documents(filter.clone())
        .await?;
    let data = database
        .collection::<OwnerRecord>("users")
        .find(filter)
        .projection(doc! {"_id":1,"username":1,"displayName":1})
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(OwnerRecord::view)
        .collect();
    Ok(Page::new(data, &input, total))
}
#[derive(Default, Deserialize)]
struct Facet {
    data: Vec<CustomerRecord>,
    meta: Vec<Count>,
}
#[derive(Deserialize)]
struct Count {
    total: u64,
}

pub async fn list(
    database: &Database,
    actor: &Actor,
    query: CustomerQuery,
) -> Result<Page<CustomerView>, AppError> {
    let (input, status) = query.split();
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "updatedAt", "code", "name", "status"])?;
    let sort: Document = sort
        .into_iter()
        .map(|(field, value)| {
            (
                match field.as_str() {
                    "code" => "codeKey".to_owned(),
                    "name" => "nameKey".to_owned(),
                    _ => field,
                },
                value,
            )
        })
        .collect();
    let mut filter = doc! {"deletedAt":null};
    if let Some(status) = status {
        filter.insert("status", status.as_str());
    }
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        filter.insert(
            "$or",
            vec![
                doc! {"nameKey":{"$regex":prefix(&search.to_lowercase())}},
                doc! {"codeKey":{"$regex":prefix(&search.to_uppercase())}},
            ],
        );
    }
    let (records, total) = if actor.scope == DataScope::Roles {
        let pipeline = vec![
            doc! {"$match":filter},
            doc! {"$sort":sort},
            doc! {"$lookup":{"from":"users","let":{"owner":"$ownerId"},"pipeline":[{"$match":{"$expr":{"$eq":["$_id","$$owner"]},"deletedAt":null,"roleIds":{"$in":&actor.scope_role_ids}}},{"$project":{"_id":1}}],"as":"visibleOwner"}},
            doc! {"$match":{"$or":[{"ownerId":actor.id},{"visibleOwner.0":{"$exists":true}}]}},
            doc! {"$facet":{"meta":[{"$count":"total"}],"data":[{"$skip":skip as i64},{"$limit":input.page_size as i64},{"$unset":"visibleOwner"}]}},
        ];
        let raw = database
            .collection::<Document>("customers")
            .aggregate(pipeline)
            .await?
            .try_next()
            .await?;
        let result: Facet = raw
            .map(mongodb::bson::deserialize_from_document)
            .transpose()
            .map_err(|_| AppError::Internal)?
            .unwrap_or_default();
        (
            result.data,
            result.meta.first().map(|count| count.total).unwrap_or(0),
        )
    } else {
        if actor.scope == DataScope::Own {
            filter.insert("ownerId", actor.id);
        }
        let total = database
            .collection::<Document>("customers")
            .count_documents(filter.clone())
            .await?;
        let records = if skip >= total {
            vec![]
        } else {
            database
                .collection::<CustomerRecord>("customers")
                .find(filter)
                .sort(sort)
                .skip(skip)
                .limit(input.page_size as i64)
                .await?
                .try_collect::<Vec<_>>()
                .await?
        };
        (records, total)
    };
    let mut views = records
        .into_iter()
        .map(CustomerRecord::view)
        .collect::<Result<Vec<_>, _>>()?;
    enrich(database, &mut views).await?;
    Ok(Page::new(views, &input, total))
}
pub async fn get(
    database: &Database,
    actor: &Actor,
    id: ObjectId,
) -> Result<CustomerView, AppError> {
    let record = database
        .collection::<CustomerRecord>("customers")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?;
    let allowed = match actor.scope {
        DataScope::All => true,
        DataScope::Own => record.owner_id == actor.id,
        DataScope::Roles => record.owner_id == actor.id || database.collection::<Document>("users")
            .count_documents(doc!{"_id":record.owner_id,"deletedAt":null,"roleIds":{"$in":&actor.scope_role_ids}}).await? > 0,
    };
    if !allowed {
        return Err(AppError::NotFound);
    }
    render(database, record).await
}
async fn new_owner(
    database: &Database,
    actor: &Actor,
    id: ObjectId,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    if actor.scope == DataScope::Own && actor.id != id {
        return Err(fail(Failure::Forbidden));
    }
    let owner = database
        .collection::<UserRecord>("users")
        .find_one(doc! {"_id":id,"deletedAt":null,"status":"active"})
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::Validation("归属人不存在或已停用")))?;
    actor
        .require_scope(id, &owner.role_ids)
        .map_err(|_| fail(Failure::Forbidden))
}
async fn visible_locked(
    database: &Database,
    actor: &Actor,
    record: &CustomerRecord,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    if actor.scope == DataScope::All || record.owner_id == actor.id {
        return Ok(());
    }
    if actor.scope == DataScope::Own {
        return Err(fail(Failure::NotFound));
    }
    if database
        .collection::<Document>("users")
        .count_documents(
            doc! {"_id":record.owner_id,"deletedAt":null,"roleIds":{"$in":&actor.scope_role_ids}},
        )
        .session(session)
        .await?
        == 0
    {
        return Err(fail(Failure::NotFound));
    }
    Ok(())
}
pub async fn create(
    database: &Database,
    actor: Actor,
    input: CreateCustomer,
) -> Result<CustomerView, AppError> {
    validate(
        &input.code,
        &input.name,
        &input.contact_name,
        &input.phone,
        &input.email,
        &input.note,
    )?;
    let owner = input
        .owner_id
        .as_deref()
        .map(validation::object_id)
        .transpose()?
        .unwrap_or(actor.id);
    let id = ObjectId::new();
    let record = transaction::run(
        database,
        (database.clone(), actor, input),
        |session, (database, actor, input)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                new_owner(database, actor, owner, session).await?;
                let now = DateTime::now();
                let fields = doc! {
                    "_id":id, "code":&input.code, "codeKey":input.code.to_uppercase(),
                    "name":&input.name, "nameKey":input.name.trim().to_lowercase(),
                    "contactName":&input.contact_name, "phone":&input.phone, "email":&input.email,
                    "note":&input.note, "status":input.status.as_str(), "ownerId":owner,
                    "version":1i64, "createdAt":now, "updatedAt":now, "deletedAt":null
                };
                database
                    .collection::<Document>("customers")
                    .insert_one(fields)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "customers.create", id, session).await?;
                database
                    .collection::<CustomerRecord>("customers")
                    .find_one(doc! {"_id":id})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))
            }
            .boxed()
        },
    )
    .await?;
    render(database, record).await
}
pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: PatchCustomer,
) -> Result<CustomerView, AppError> {
    version(input.version)?;
    let owner = input
        .owner_id
        .as_deref()
        .map(validation::object_id)
        .transpose()?;
    let record = transaction::run(database, (database.clone(), actor, input), |session, (database, actor, input)| {
        async move {
            shared::lock(database, "rbac", session).await?;
            let mut record = database.collection::<CustomerRecord>("customers")
                .find_one(doc! {"_id":id,"deletedAt":null}).session(&mut *session).await?
                .ok_or_else(|| fail(Failure::NotFound))?;
            visible_locked(database, actor, &record, session).await?;
            if record.version != input.version {
                return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));
            }
            if let Some(owner) = owner && owner != record.owner_id {
                new_owner(database, actor, owner, session).await?;
                record.owner_id = owner;
            }
            if let Some(code) = &input.code { record.code = code.clone(); }
            if let Some(name) = &input.name { record.name = name.clone(); }
            if let Some(contact) = &input.contact_name { record.contact_name = contact.clone(); }
            if let Some(phone) = &input.phone { record.phone = phone.clone(); }
            if let Some(email) = &input.email { record.email = email.clone(); }
            if let Some(note) = &input.note { record.note = note.clone(); }
            if let Some(status) = input.status { record.status = status; }
            validate(&record.code, &record.name, &record.contact_name, &record.phone, &record.email, &record.note)
                .map_err(|_| fail(Failure::Validation("客户编号、名称或联系资料无效")))?;
            record.version += 1;
            record.updated_at = DateTime::now();
            let fields = doc! {
                "code":&record.code, "codeKey":record.code.to_uppercase(),
                "name":&record.name, "nameKey":record.name.trim().to_lowercase(),
                "contactName":&record.contact_name, "phone":&record.phone, "email":&record.email,
                "note":&record.note, "status":record.status.as_str(), "ownerId":record.owner_id,
                "version":record.version, "updatedAt":record.updated_at
            };
            database.collection::<Document>("customers").update_one(doc! {"_id":id}, doc! {"$set":fields})
                .session(&mut *session).await?;
            shared::audit(database, actor, "customers.update", id, session).await?;
            Ok(record)
        }
        .boxed()
    })
    .await?;
    render(database, record).await
}
pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    expected: i64,
) -> Result<(), AppError> {
    version(expected)?;
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                let record = database
                    .collection::<CustomerRecord>("customers")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                visible_locked(database, actor, &record, session).await?;
                if record.version != expected {
                    return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("customers")
                    .update_one(
                        doc! {"_id":id},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "customers.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
