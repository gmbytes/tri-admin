use super::{
    authority,
    dto::{self, CreateDefinition, Generate, HistoryQuery, UpdateDefinition},
    models::*,
};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::{
        auth::AuthContext,
        rbac::shared::{self, Actor},
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{self, Binary, DateTime, Document, doc, oid::ObjectId, spec::BinarySubtype},
};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::io::Read;
use tri_resource_generator::render::{self, TEMPLATE_VERSION};
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn prefix(raw: &str) -> String {
    let mut value = String::from("^");
    for c in raw.chars() {
        if ".+*?^${}()|[]\\".contains(c) {
            value.push('\\')
        }
        value.push(c)
    }
    value
}
fn schema_error(error: tri_resource_runtime::schema::SchemaError) -> AppError {
    AppError::Validation(error.to_string())
}
pub async fn create_definition(
    database: &Database,
    context: &AuthContext,
    input: CreateDefinition,
) -> Result<(DefinitionView, bool), AppError> {
    input.validate()?;
    let fingerprint = input.schema.checksum().map_err(schema_error)?;
    let id = ObjectId::new();
    let now = DateTime::now();
    let (meta, created) = transaction::run(
        database,
        (database.clone(), context.clone(), input, fingerprint),
        |session, (database, context, input, fingerprint)| {
            async move {
                let actor = authority::fresh(database, session, context, true).await?;
                if let Some(old) = database
                    .collection::<DefinitionRecord>("code_definitions")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&input.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if old.fingerprint != *fingerprint {
                        return Err(fail(Failure::Conflict("请求标识已用于不同定义")));
                    }
                    return Ok((old.meta, false));
                }
                let record = DefinitionRecord {
                    meta: DefinitionMeta {
                        id,
                        owner_id: actor.id,
                        request_id: input.request_id.clone(),
                        schema: input.schema.clone(),
                        version: 1,
                        created_at: now,
                        updated_at: now,
                        deleted_at: None,
                    },
                    fingerprint: fingerprint.clone(),
                };
                database
                    .collection::<DefinitionRecord>("code_definitions")
                    .insert_one(&record)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, &actor, "codegen.definition.create", id, session).await?;
                Ok((record.meta, true))
            }
            .boxed()
        },
    )
    .await?;
    Ok((meta.view()?, created))
}
async fn definition_tx(
    database: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    id: ObjectId,
) -> mongodb::error::Result<DefinitionMeta> {
    let row = database
        .collection::<DefinitionMeta>("code_definitions")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .projection(doc! {"fingerprint":0})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    authority::owns(database, session, actor, row.owner_id).await?;
    Ok(row)
}
pub async fn definition(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
) -> Result<DefinitionView, AppError> {
    transaction::run_snapshot(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                let actor = authority::fresh(database, session, context, false).await?;
                definition_tx(database, session, &actor, id).await
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn definition_by_request(
    database: &Database,
    context: &AuthContext,
    request: &str,
) -> Result<DefinitionView, AppError> {
    dto::request_id(request)?;
    transaction::run_snapshot(
        database,
        (database.clone(), context.clone(), request.to_owned()),
        |session, (database, context, request)| {
            async move {
                let actor = authority::fresh(database, session, context, false).await?;
                database
                    .collection::<DefinitionMeta>("code_definitions")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&*request})
                    .projection(doc! {"fingerprint":0})
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
pub async fn update_definition(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    input: UpdateDefinition,
) -> Result<DefinitionView, AppError> {
    dto::version(input.version)?;
    dto::schema(&input.schema)?;
    transaction::run(
        database,
        (database.clone(), context.clone(), input),
        |session, (database, context, input)| {
            async move {
                let actor = authority::fresh(database, session, context, true).await?;
                let mut row = definition_tx(database, session, &actor, id).await?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("定义版本已改变")));
                }
                let schema =
                    bson::serialize_to_bson(&input.schema).map_err(|_| fail(Failure::NotReady))?;
                let now = DateTime::now();
                database
                    .collection::<Document>("code_definitions")
                    .update_one(
                        doc! {"_id":id,"version":input.version,"deletedAt":null},
                        doc! {"$set":{"schema":schema,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, &actor, "codegen.definition.update", id, session).await?;
                row.schema = input.schema.clone();
                row.version += 1;
                row.updated_at = now;
                Ok(row)
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn delete_definition(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    dto::version(version)?;
    transaction::run(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                let actor = authority::fresh(database, session, context, true).await?;
                let row = definition_tx(database, session, &actor, id).await?;
                if row.version != version {
                    return Err(fail(Failure::Conflict("定义版本已改变")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("code_definitions")
                    .update_one(
                        doc! {"_id":id,"version":version,"deletedAt":null},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, &actor, "codegen.definition.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
fn page_filter(input: &PageInput, active: bool) -> Result<Document, AppError> {
    input.validate()?;
    let mut filter = Document::new();
    if active {
        filter.insert("deletedAt", bson::Bson::Null);
    }
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        filter.insert("schema.resource", doc! {"$regex":prefix(search)});
    }
    Ok(filter)
}
async fn scoped_page<T: DeserializeOwned + Send>(
    database: &Database,
    context: &AuthContext,
    collection: &'static str,
    input: &PageInput,
    filter: Document,
) -> Result<(Vec<T>, u64), AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt"])?;
    let size = input.page_size;
    transaction::run_snapshot(database,(database.clone(),context.clone(),filter,sort),|session,(database,context,filter,sort)|async move{let actor=authority::fresh(database,session,context,false).await?;let mut filter=filter.clone();if actor.scope==DataScope::Own{filter.insert("ownerId",actor.id);}let mut pipeline=vec![doc!{"$match":filter}];
 if actor.scope==DataScope::Roles{pipeline.push(doc!{"$lookup":{"from":"users","localField":"ownerId","foreignField":"_id","pipeline":[{"$match":{"deletedAt":null}},{"$project":{"roleIds":1}}],"as":"__owner"}});pipeline.push(doc!{"$match":{"$or":[{"ownerId":actor.id},{"__owner.roleIds":{"$in":&actor.scope_role_ids}}]}});}
 pipeline.push(doc!{"$sort":sort.clone()});pipeline.push(doc!{"$facet":{"rows":[{"$skip":skip as i64},{"$limit":size as i64},{"$project":{"fingerprint":0,"__owner":0}}],"count":[{"$count":"total"}]}});
 let mut result=database.collection::<Document>(collection).aggregate(pipeline).max_time(std::time::Duration::from_secs(3)).session(&mut *session).await?;let mut cursor=result.stream(session);let result=cursor.try_next().await?.ok_or_else(||fail(Failure::NotReady))?;
 let rows=result.get_array("rows").map_err(|_|fail(Failure::NotReady))?.iter().map(|row|bson::deserialize_from_bson::<T>(row.clone()).map_err(|_|fail(Failure::NotReady))).collect::<mongodb::error::Result<Vec<_>>>()?;
 let counts=result.get_array("count").map_err(|_|fail(Failure::NotReady))?;let total=if let Some(row)=counts.first(){let value=row.as_document().and_then(|row|row.get("total")).ok_or_else(||fail(Failure::NotReady))?;value.as_i64().or_else(||value.as_i32().map(i64::from)).filter(|n|*n>=0).ok_or_else(||fail(Failure::NotReady))? as u64}else{0};Ok((rows,total))
 }.boxed()).await
}
pub async fn definitions(
    database: &Database,
    context: &AuthContext,
    input: PageInput,
) -> Result<Page<DefinitionView>, AppError> {
    let filter = page_filter(&input, true)?;
    let (rows, total) =
        scoped_page::<DefinitionMeta>(database, context, "code_definitions", &input, filter)
            .await?;
    Ok(Page::new(
        rows.into_iter()
            .map(DefinitionMeta::view)
            .collect::<Result<_, _>>()?,
        &input,
        total,
    ))
}
pub async fn generations(
    database: &Database,
    context: &AuthContext,
    input: HistoryQuery,
) -> Result<Page<GenerationView>, AppError> {
    let page = PageInput {
        page: input.page,
        page_size: input.page_size,
        search: input.search,
        sort: None,
    };
    let mut filter = page_filter(&page, false)?;
    if let Some(id) = input.definition_id {
        filter.insert("definitionId", validation::object_id(&id)?);
    }
    let (rows, total) =
        scoped_page::<GenerationMeta>(database, context, "code_generations", &page, filter).await?;
    Ok(Page::new(
        rows.into_iter()
            .map(GenerationMeta::view)
            .collect::<Result<_, _>>()?,
        &page,
        total,
    ))
}
async fn generation_tx(
    database: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    id: ObjectId,
) -> mongodb::error::Result<GenerationMeta> {
    let row = database
        .collection::<GenerationMeta>("code_generations")
        .find_one(doc! {"_id":id})
        .projection(doc! {"fingerprint":0})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    authority::owns(database, session, actor, row.owner_id).await?;
    Ok(row)
}
pub async fn generation(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
) -> Result<GenerationView, AppError> {
    transaction::run_snapshot(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                let actor = authority::fresh(database, session, context, false).await?;
                generation_tx(database, session, &actor, id).await
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn generation_by_request(
    database: &Database,
    context: &AuthContext,
    request: &str,
) -> Result<GenerationView, AppError> {
    dto::request_id(request)?;
    transaction::run_snapshot(
        database,
        (database.clone(), context.clone(), request.to_owned()),
        |session, (database, context, request)| {
            async move {
                let actor = authority::fresh(database, session, context, false).await?;
                database
                    .collection::<GenerationMeta>("code_generations")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&*request})
                    .projection(doc! {"fingerprint":0})
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
enum Candidate {
    Replay(GenerationMeta),
    Definition(DefinitionMeta),
}
async fn replay(
    database: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    request: &str,
    fingerprint: &str,
) -> mongodb::error::Result<Option<GenerationMeta>> {
    if let Some(old) = database
        .collection::<GenerationRecord>("code_generations")
        .find_one(doc! {"ownerId":actor.id,"requestId":request})
        .session(session)
        .await?
    {
        if old.fingerprint != fingerprint {
            return Err(fail(Failure::Conflict("请求标识已用于不同生成输入")));
        }
        Ok(Some(old.meta))
    } else {
        Ok(None)
    }
}
pub async fn generate(
    database: &Database,
    context: &AuthContext,
    input: Generate,
) -> Result<(GenerationView, bool), AppError> {
    input.validate()?;
    let definition_id = validation::object_id(&input.definition_id)?;
    let fingerprint =
        hash(format!("{}:{}", definition_id.to_hex(), input.definition_version).as_bytes());
    let candidate = transaction::run_snapshot(
        database,
        (
            database.clone(),
            context.clone(),
            input.clone(),
            fingerprint.clone(),
        ),
        |session, (database, context, input, fingerprint)| {
            async move {
                let actor = authority::fresh(database, session, context, false).await?;
                if !actor.can("codegen.write") {
                    return Err(fail(Failure::Forbidden));
                }
                if let Some(old) =
                    replay(database, session, &actor, &input.request_id, fingerprint).await?
                {
                    return Ok(Candidate::Replay(old));
                }
                let definition = definition_tx(database, session, &actor, definition_id).await?;
                if definition.version != input.definition_version {
                    return Err(fail(Failure::Conflict("定义版本已改变")));
                }
                Ok(Candidate::Definition(definition))
            }
            .boxed()
        },
    )
    .await?;
    let definition = match candidate {
        Candidate::Replay(row) => return Ok((row.view()?, false)),
        Candidate::Definition(row) => row,
    };
    let schema = definition.schema.clone();
    let bundle = tokio::task::spawn_blocking(move || render::render(&schema))
        .await
        .map_err(|_| AppError::Internal)?
        .map_err(|_| AppError::Internal)?;
    let schema_checksum = definition
        .schema
        .checksum()
        .map_err(|_| AppError::NotReady)?;
    let now = DateTime::now();
    let record = GenerationRecord {
        meta: GenerationMeta {
            id: ObjectId::new(),
            owner_id: validation::object_id(&context.user.id)?,
            request_id: input.request_id.clone(),
            definition_id,
            definition_version: input.definition_version,
            template_version: TEMPLATE_VERSION,
            schema: definition.schema,
            schema_checksum,
            sha256: bundle.sha256,
            size: bundle.archive.len() as u64,
            files: bundle
                .files
                .into_iter()
                .map(|file| FileMeta {
                    path: file.path,
                    size: file.size as u64,
                    sha256: file.sha256,
                })
                .collect(),
            created_at: now,
        },
        fingerprint,
    };
    let archive = Binary {
        subtype: BinarySubtype::Generic,
        bytes: bundle.archive,
    };
    let (row, created) = transaction::run(
        database,
        (database.clone(), context.clone(), record, archive),
        |session, (database, context, record, archive)| {
            async move {
                let actor = authority::fresh(database, session, context, true).await?;
                if let Some(old) = replay(
                    database,
                    session,
                    &actor,
                    &record.meta.request_id,
                    &record.fingerprint,
                )
                .await?
                {
                    return Ok((old, false));
                }
                let current =
                    definition_tx(database, session, &actor, record.meta.definition_id).await?;
                if current.version != record.meta.definition_version
                    || current
                        .schema
                        .checksum()
                        .map_err(|_| fail(Failure::NotReady))?
                        != record.meta.schema_checksum
                {
                    return Err(fail(Failure::Conflict("生成期间定义已改变")));
                }
                database
                    .collection::<GenerationRecord>("code_generations")
                    .insert_one(&*record)
                    .session(&mut *session)
                    .await?;
                database
                    .collection::<Document>("codegen_packages")
                    .insert_one(doc! {"_id":record.meta.id,"archiveBytes":archive.clone()})
                    .session(&mut *session)
                    .await?;
                shared::audit(
                    database,
                    &actor,
                    "codegen.generate",
                    record.meta.id,
                    session,
                )
                .await?;
                Ok((record.meta.clone(), true))
            }
            .boxed()
        },
    )
    .await?;
    Ok((row.view()?, created))
}
pub async fn package(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
) -> Result<(GenerationMeta, Vec<u8>), AppError> {
    let (meta, stored) = transaction::run_snapshot(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                let actor = authority::fresh(database, session, context, false).await?;
                let meta = generation_tx(database, session, &actor, id).await?;
                let package = database
                    .collection::<StoredPackage>("codegen_packages")
                    .find_one(doc! {"_id":id})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                Ok((meta, package))
            }
            .boxed()
        },
    )
    .await?;
    let bytes = stored.archive_bytes.bytes;
    if stored.archive_bytes.subtype != BinarySubtype::Generic
        || bytes.len() > 1024 * 1024
        || meta.size != bytes.len() as u64
        || hash(&bytes) != meta.sha256
    {
        return Err(AppError::NotReady);
    }
    Ok((meta, bytes))
}
pub async fn preview(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    path: &str,
) -> Result<SourceView, AppError> {
    if path.is_empty()
        || path.len() > 256
        || path
            .split('/')
            .any(|part| part.is_empty() || part == ".." || part == ".")
        || !path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-/".contains(&c))
    {
        return Err(AppError::Validation("预览文件路径无效".into()));
    }
    let (meta, bytes) = package(database, context, id).await?;
    let file = meta
        .files
        .iter()
        .find(|file| file.path == path)
        .ok_or(AppError::NotFound)?;
    if file.size > 512 * 1024 {
        return Err(AppError::NotReady);
    }
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| AppError::NotReady)?;
    if archive.len() > 64 {
        return Err(AppError::NotReady);
    }
    let entry = archive.by_name(path).map_err(|_| AppError::NotReady)?;
    if entry.size() != file.size {
        return Err(AppError::NotReady);
    }
    let mut content = Vec::new();
    entry
        .take(512 * 1024 + 1)
        .read_to_end(&mut content)
        .map_err(|_| AppError::NotReady)?;
    if content.len() as u64 != file.size || hash(&content) != file.sha256 {
        return Err(AppError::NotReady);
    }
    Ok(SourceView {
        path: file.path.clone(),
        content: String::from_utf8(content).map_err(|_| AppError::NotReady)?,
        size: file.size,
        sha256: file.sha256.clone(),
    })
}
