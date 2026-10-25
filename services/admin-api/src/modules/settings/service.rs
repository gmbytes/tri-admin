use super::{dto::*, models::*, validate};
use crate::{
    common::{
        canonical,
        error::AppError,
        pagination::Page,
        transaction::{self, Failure, fail},
        validation,
        vault::{self, SealedBox},
    },
    config::Config,
    modules::rbac::shared::{self, Actor},
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{Bson, DateTime, Document, doc, oid::ObjectId},
};
use zeroize::Zeroizing;
fn tx(error: AppError) -> mongodb::error::Error {
    match error {
        AppError::SecretStoreUnavailable => fail(Failure::SecretStoreUnavailable),
        AppError::Validation(_) => fail(Failure::Validation("配置字段或凭据无效")),
        _ => fail(Failure::NotReady),
    }
}
fn aad(id: ObjectId, kind: Kind, configuration_json: &str) -> String {
    format!(
        "tri-admin/settings/profile/{}/{}/{}",
        id.to_hex(),
        kind.as_str(),
        canonical::checksum(configuration_json.as_bytes())
    )
}
fn seal(
    config: &Config,
    id: ObjectId,
    kind: Kind,
    configuration_json: &str,
    secret: &Credentials,
) -> Result<SealedBox, AppError> {
    let json = Zeroizing::new(canonical::json(secret)?);
    vault::seal(
        config,
        aad(id, kind, configuration_json).as_bytes(),
        json.as_bytes(),
    )
}
pub(super) fn decrypt(config: &Config, row: &Profile) -> Result<Option<Credentials>, AppError> {
    let Some(sealed) = &row.secret_box else {
        return Ok(None);
    };
    let bytes = vault::open(
        config,
        aad(row.id, row.kind, &row.configuration_json).as_bytes(),
        sealed,
    )?;
    let value: Credentials =
        serde_json::from_slice(&bytes).map_err(|_| AppError::SecretStoreUnavailable)?;
    if value.kind() != row.kind {
        return Err(AppError::SecretStoreUnavailable);
    }
    Ok(Some(value))
}
pub(crate) async fn storage_definition(
    database: &Database,
    config: &Config,
    id: ObjectId,
) -> Result<(Integration, Option<Credentials>), AppError> {
    let row = database
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":id,"deletedAt":null,"enabled":true})
        .await?
        .ok_or(AppError::NotFound)?;
    if !row.kind.storage() {
        return Err(AppError::Validation("所选配置不是文件存储".into()));
    }
    let public = row.configuration()?;
    let secret = decrypt(config, &row)?;
    validate::credentials(&public, secret.as_ref(), true)?;
    Ok((public, secret))
}
fn boxed(value: &Option<SealedBox>) -> mongodb::error::Result<Bson> {
    mongodb::bson::serialize_to_bson(value).map_err(|_| fail(Failure::NotReady))
}
fn description(value: &str) -> Result<(), AppError> {
    if value.chars().count() > 1000 {
        Err(AppError::Validation("描述最多1000字符".into()))
    } else {
        Ok(())
    }
}
pub async fn get_service(database: &Database) -> Result<ServiceSettings, AppError> {
    database
        .collection::<ServiceSettings>("service_settings")
        .find_one(doc! {"_id":stable_id("service")})
        .await?
        .ok_or(AppError::NotReady)
}
pub async fn login_limit(database: &Database, config: &Config) -> Result<i64, AppError> {
    let value = get_service(database)
        .await?
        .login_limit_per_ip_override
        .unwrap_or(config.login_limit_per_ip);
    if !(1..=10000).contains(&value) {
        return Err(AppError::NotReady);
    }
    Ok(value)
}
pub async fn service_view(database: &Database, config: &Config) -> Result<ServiceView, AppError> {
    get_service(database).await?.view(config)
}
pub(crate) async fn binding(
    database: &Database,
    id: Option<ObjectId>,
    storage: bool,
    config: &Config,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let Some(id) = id else {
        return Ok(());
    };
    let row = database
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":id,"deletedAt":null,"enabled":true})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Validation("默认绑定配置不存在或未启用")))?;
    if if storage {
        !row.kind.storage()
    } else {
        row.kind != Kind::Smtp
    } {
        return Err(fail(Failure::Validation("默认绑定的配置类型不匹配")));
    }
    let public = row.configuration().map_err(tx)?;
    let secret = decrypt(config, &row).map_err(tx)?;
    validate::credentials(&public, secret.as_ref(), true).map_err(tx)
}
pub async fn patch_service(
    database: &Database,
    config: &Config,
    actor: Actor,
    input: ServicePatch,
) -> Result<ServiceView, AppError> {
    validate::version(input.version)?;
    if let Some(name) = &input.display_name {
        validation::text(name, "应用名称", 80)?;
    }
    if input
        .login_limit_per_ip_override
        .flatten()
        .is_some_and(|value| !(1..=10000).contains(&value))
    {
        return Err(AppError::Validation("每IP登录限制需为1–10000".into()));
    }
    let storage = input
        .storage_profile_id
        .as_ref()
        .map(|value| value.as_deref().map(validation::object_id).transpose())
        .transpose()?;
    let mail = input
        .mail_profile_id
        .as_ref()
        .map(|value| value.as_deref().map(validation::object_id).transpose())
        .transpose()?;
    let row=transaction::run(database,(database.clone(),config.clone(),actor,input),|session,(database,config,actor,input)|async move{
  shared::lock(database,"settings",session).await?;let mut row=database.collection::<ServiceSettings>("service_settings").find_one(doc!{"_id":stable_id("service")}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotReady))?;
  if row.version!=input.version{return Err(fail(Failure::Conflict("配置已被修改，请刷新后重试")));}
  if let Some(value)=&input.display_name{row.display_name=value.clone();}
  if let Some(value)=input.login_limit_per_ip_override{row.login_limit_per_ip_override=value;}
  if let Some(value)=storage{binding(database,value,true,config,session).await?;row.storage_profile_id=value;}
  if let Some(value)=mail{binding(database,value,false,config,session).await?;row.mail_profile_id=value;}
  row.version=row.version.checked_add(1).ok_or_else(||fail(Failure::Conflict("版本已达上限")))?;row.updated_at=DateTime::now();
  database.collection::<Document>("service_settings").update_one(doc!{"_id":row.id},doc!{"$set":{"displayName":&row.display_name,"loginLimitPerIpOverride":row.login_limit_per_ip_override,"storageProfileId":row.storage_profile_id,"mailProfileId":row.mail_profile_id,"version":row.version,"updatedAt":row.updated_at}}).session(&mut *session).await?;
  shared::audit(database,actor,"settings.service.update",row.id,session).await?;Ok(row)
 }.boxed()).await?;
    row.view(config)
}
pub async fn list(
    database: &Database,
    config: &Config,
    input: ListInput,
) -> Result<Page<ProfileView>, AppError> {
    let page = input.page();
    let skip = page.validate()?;
    let sort = page.sort_document(&["createdAt", "name", "code"])?;
    let mut filter = doc! {"deletedAt":null};
    if let Some(kind) = input.kind {
        filter.insert("kind", kind.as_str());
    }
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut regex = "^".to_owned();
        for c in search.chars() {
            if ".+*?^${}()|[]\\".contains(c) {
                regex.push('\\');
            }
            regex.push(c);
        }
        filter.insert("code", doc! {"$regex":regex});
    }
    let total = database
        .collection::<Document>("integration_profiles")
        .count_documents(filter.clone())
        .await?;
    let data = database
        .collection::<Profile>("integration_profiles")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(page.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(|row| row.view(config))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(data, &page, total))
}
pub async fn get(
    database: &Database,
    config: &Config,
    id: ObjectId,
) -> Result<ProfileView, AppError> {
    database
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?
        .view(config)
}
pub async fn create(
    database: &Database,
    config: &Config,
    actor: Actor,
    input: Create,
) -> Result<ProfileView, AppError> {
    validation::text(&input.name, "配置名称", 80)?;
    validate::code(&input.code)?;
    description(&input.description)?;
    validate::public(&input.configuration)?;
    validate::credentials(&input.configuration, input.secret.as_ref(), input.enabled)?;
    let id = ObjectId::new();
    let json = canonical::json(&input.configuration)?;
    let secret = input
        .secret
        .as_ref()
        .map(|value| seal(config, id, input.configuration.kind(), &json, value))
        .transpose()?;
    let row=transaction::run(database,(database.clone(),actor,input,secret,json),|session,(database,actor,input,secret,json)|async move{
  shared::lock(database,"settings",session).await?;if database.collection::<Document>("integration_profiles").count_documents(doc!{"deletedAt":null}).session(&mut *session).await?>=1000{return Err(fail(Failure::Conflict("集成配置数量已达上限")));}
  let now=DateTime::now();database.collection::<Document>("integration_profiles").insert_one(doc!{"_id":id,"name":&input.name,"code":&input.code,"description":&input.description,"kind":input.configuration.kind().as_str(),"configurationJson":&*json,"secretBox":boxed(secret)?,"enabled":input.enabled,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).session(&mut *session).await?;shared::audit(database,actor,"settings.profiles.create",id,session).await?;
  database.collection::<Profile>("integration_profiles").find_one(doc!{"_id":id}).session(session).await?.ok_or_else(||fail(Failure::NotReady))
 }.boxed()).await?;
    row.view(config)
}
async fn bound(
    database: &Database,
    id: ObjectId,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    if database
        .collection::<Document>("service_settings")
        .find_one(doc! {"$or":[{"storageProfileId":id},{"mailProfileId":id}]})
        .session(&mut *session)
        .await?
        .is_some()
    {
        return Ok(true);
    }
    shared::lock(database, "files", session).await?;
    for (collection, filter) in [
        ("files", doc! {"storageProfileId":id,"deletedAt":null}),
        (
            "file_uploads",
            doc! {"storageProfileId":id,"state":{"$nin":["cancelled","completed","failed"]}},
        ),
        (
            "file_uploads",
            doc! {"storageProfileId":id,"cleanupPending":true},
        ),
        (
            "file_attempts",
            doc! {"storageProfileId":id,"state":{"$ne":"cleaned"}},
        ),
        (
            "file_cleanup_tasks",
            doc! {"storageProfileId":id,"state":"pending"},
        ),
    ] {
        if database
            .collection::<Document>(collection)
            .find_one(filter)
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub async fn patch(
    database: &Database,
    config: &Config,
    actor: Actor,
    id: ObjectId,
    input: Patch,
) -> Result<ProfileView, AppError> {
    validate::version(input.version)?;
    if let Some(value) = &input.name {
        validation::text(value, "配置名称", 80)?;
    }
    if let Some(value) = &input.description {
        description(value)?;
    }
    if let Some(value) = &input.configuration {
        validate::public(value)?;
    }
    let row=transaction::run(database,(database.clone(),config.clone(),actor,input),|session,(database,config,actor,input)|async move{
  shared::lock(database,"settings",session).await?;let mut row=database.collection::<Profile>("integration_profiles").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
  if row.version!=input.version{return Err(fail(Failure::Conflict("配置已被修改，请刷新后重试")));}
  let old=row.configuration().map_err(tx)?;let public=input.configuration.as_ref().unwrap_or(&old);
  if public.kind()!=row.kind{return Err(fail(Failure::Validation("配置类型不可修改")));}
  if row.kind.storage()&&public!=&old{return Err(fail(Failure::Conflict("存储位置创建后不可修改，请新建配置并切换绑定")));}
  if public!=&old&&row.secret_box.is_some()&&input.secret.is_none(){return Err(fail(Failure::Conflict("变更连接配置必须重新提交或清除凭据")));}
  let public_json=canonical::json(public).map_err(tx)?;
  if input.enabled==Some(false)&&bound(database,id,session).await?{return Err(fail(Failure::Conflict("配置仍被默认设置、文件或上传任务引用，不能禁用")));}
  if let Some(value)=&input.name{row.name=value.clone();}
  if let Some(value)=&input.description{row.description=value.clone();}
  if let Some(value)=input.enabled{row.enabled=value;}
  let secret=match &input.secret{Some(value)=>value.clone(),None if row.enabled=>decrypt(config,&row).map_err(tx)?,None=>None};
  if input.secret.is_some()||row.enabled{validate::credentials(public,secret.as_ref(),row.enabled).map_err(tx)?;}
  if let Some(change)=&input.secret{row.secret_box=change.as_ref().map(|value|seal(config,id,row.kind,&public_json,value)).transpose().map_err(tx)?;}
  row.configuration_json=public_json;row.version=row.version.checked_add(1).ok_or_else(||fail(Failure::Conflict("版本已达上限")))?;row.updated_at=DateTime::now();
  database.collection::<Document>("integration_profiles").update_one(doc!{"_id":id},doc!{"$set":{"name":&row.name,"description":&row.description,"configurationJson":&row.configuration_json,"secretBox":boxed(&row.secret_box)?,"enabled":row.enabled,"version":row.version,"updatedAt":row.updated_at}}).session(&mut *session).await?;shared::audit(database,actor,"settings.profiles.update",id,session).await?;Ok(row)
 }.boxed()).await?;
    row.view(config)
}
pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    validate::version(version)?;
    transaction::run(database,(database.clone(),actor),|session,(database,actor)|async move{
 shared::lock(database,"settings",session).await?;let row=database.collection::<Profile>("integration_profiles").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
 if row.version!=version{return Err(fail(Failure::Conflict("配置已被修改，请刷新后重试")));}
 if bound(database,id,session).await?{return Err(fail(Failure::Conflict("配置仍被默认设置、文件或上传任务引用，不能删除")));}
 let now=DateTime::now();database.collection::<Document>("integration_profiles").update_one(doc!{"_id":id},doc!{"$set":{"deletedAt":now,"enabled":false,"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;shared::audit(database,actor,"settings.profiles.delete",id,session).await
}.boxed()).await
}
pub async fn rewrap(
    database: &Database,
    config: &Config,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<ProfileView, AppError> {
    validate::version(version)?;
    let row=transaction::run(database,(database.clone(),config.clone(),actor),|session,(database,config,actor)|async move{
 shared::lock(database,"settings",session).await?;let mut row=database.collection::<Profile>("integration_profiles").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;if row.version!=version{return Err(fail(Failure::Conflict("配置已被修改，请刷新后重试")));}let secret=decrypt(config,&row).map_err(tx)?.ok_or_else(||fail(Failure::Validation("此配置没有待轮换凭据")))?;row.secret_box=Some(seal(config,id,row.kind,&row.configuration_json,&secret).map_err(tx)?);row.version=row.version.checked_add(1).ok_or_else(||fail(Failure::Conflict("版本已达上限")))?;row.updated_at=DateTime::now();database.collection::<Document>("integration_profiles").update_one(doc!{"_id":id},doc!{"$set":{"secretBox":boxed(&row.secret_box)?,"version":row.version,"updatedAt":row.updated_at}}).session(&mut *session).await?;shared::audit(database,actor,"settings.profiles.rewrap",id,session).await?;Ok(row)
}.boxed()).await?;
    row.view(config)
}
