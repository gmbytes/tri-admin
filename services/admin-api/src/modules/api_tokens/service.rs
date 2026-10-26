use super::{
    dto::CreateToken,
    models::{self, IssueView, TokenMeta, TokenRecord, TokenView},
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
    },
    modules::{
        auth::{self, AdminCredential, AuthContext},
        open_access::{credential, limits, usage},
        rbac::shared::{self, Actor},
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
async fn permissions(
    database: &Database,
    session: &mut ClientSession,
    owner: ObjectId,
    session_id: ObjectId,
) -> mongodb::error::Result<std::collections::HashSet<String>> {
    if database.collection::<Document>("sessions").find_one(doc!{"_id":session_id,"userId":owner,"status":"active","revokedAt":null,"expiresAt":{"$gt":DateTime::now()}}).session(&mut *session).await?.is_none(){return Err(fail(Failure::Unauthorized));}
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {"_id":owner,"status":"active","deletedAt":null})
        .projection(doc! {"roleIds":1})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Unauthorized))?;
    let ids = user
        .get_array("roleIds")
        .map_err(|_| fail(Failure::NotReady))?;
    let mut cursor = database
        .collection::<Document>("roles")
        .find(doc! {"_id":{"$in":ids},"deletedAt":null})
        .projection(doc! {"permissions":1})
        .session(&mut *session)
        .await?;
    let roles = cursor.stream(session).try_collect::<Vec<_>>().await?;
    let mut permissions = std::collections::HashSet::new();
    for role in roles {
        for value in role
            .get_array("permissions")
            .map_err(|_| fail(Failure::NotReady))?
        {
            permissions.insert(
                value
                    .as_str()
                    .ok_or_else(|| fail(Failure::NotReady))?
                    .to_owned(),
            );
        }
    }
    if !permissions.contains("*") && !permissions.contains("apiTokens.write") {
        return Err(fail(Failure::Forbidden));
    }
    Ok(permissions)
}
pub async fn create(
    database: &Database,
    context: &AuthContext,
    input: CreateToken,
) -> Result<(IssueView, bool), AppError> {
    let session_id = context.require_session()?;
    let input = input.validate()?;
    let actor = Actor::new(context)?;
    let owner = actor.id;
    let fingerprint =
        crate::common::canonical::checksum(crate::common::canonical::json(&input)?.as_bytes());
    let id = ObjectId::new();
    let (secret, secret_hash) = credential::issue("tri_at1", id)?;
    let now = DateTime::now();
    let record = TokenRecord {
        meta: TokenMeta {
            id,
            owner_id: owner,
            request_id: input.request_id,
            name: input.name,
            scopes: input.scopes,
            allowed_ips: input.allowed_ips,
            minute_limit: input.minute_limit,
            daily_quota: input.daily_quota,
            created_at: now,
            expires_at: DateTime::from_millis(
                now.timestamp_millis() + i64::from(input.expires_in_days) * 86400000,
            ),
            revoked_at: None,
            last_used_at: None,
        },
        secret_hash,
        fingerprint,
        usage_fence: 0,
    };
    let (meta, created) = transaction::run(
        database,
        (database.clone(), record, actor),
        |session, (database, record, actor)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "api-tokens", session).await?;
                let permissions = permissions(database, session, owner, session_id).await?;
                if let Some(existing) = database
                    .collection::<TokenRecord>("api_tokens")
                    .find_one(doc! {"ownerId":owner,"requestId":&record.meta.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if existing.fingerprint != record.fingerprint {
                        return Err(fail(Failure::Conflict("请求标识已用于不同签发输入")));
                    }
                    return Ok((existing.meta, false));
                }
                if !permissions.contains("*")
                    && record
                        .meta
                        .scopes
                        .iter()
                        .any(|scope| !permissions.contains(scope))
                {
                    return Err(fail(Failure::Forbidden));
                }
                if database
                    .collection::<Document>("api_tokens")
                    .count_documents(
                        doc! {"ownerId":owner,"revokedAt":null,"expiresAt":{"$gt":DateTime::now()}},
                    )
                    .session(&mut *session)
                    .await?
                    >= 32
                {
                    return Err(fail(Failure::RateLimited));
                }
                database
                    .collection::<TokenRecord>("api_tokens")
                    .insert_one(&*record)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "apiTokens.create", id, session).await?;
                Ok((record.meta.clone(), true))
            }
            .boxed()
        },
    )
    .await?;
    Ok((
        IssueView {
            token: meta.view()?,
            secret: created.then_some(secret),
        },
        created,
    ))
}
pub async fn get(
    database: &Database,
    owner: ObjectId,
    id: ObjectId,
) -> Result<TokenView, AppError> {
    database
        .collection::<TokenMeta>("api_tokens")
        .find_one(doc! {"_id":id,"ownerId":owner})
        .projection(models::projection())
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
pub async fn by_request(
    database: &Database,
    owner: ObjectId,
    request: &str,
) -> Result<TokenView, AppError> {
    let parsed =
        uuid::Uuid::parse_str(request).map_err(|_| AppError::Validation("请求标识无效".into()))?;
    if parsed.to_string() != request {
        return Err(AppError::Validation("请求标识无效".into()));
    }
    database
        .collection::<TokenMeta>("api_tokens")
        .find_one(doc! {"ownerId":owner,"requestId":request})
        .projection(models::projection())
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
pub async fn list(
    database: &Database,
    owner: ObjectId,
    input: PageInput,
) -> Result<Page<TokenView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "name"])?;
    let mut filter = doc! {"ownerId":owner};
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let mut prefix = "^".to_owned();
        for c in search.chars() {
            if ".+*?^${}()|[]\\".contains(c) {
                prefix.push('\\');
            }
            prefix.push(c);
        }
        filter.insert("name", doc! {"$regex":prefix});
    }
    let (rows, total) = transaction::run_snapshot(
        database,
        (database.clone(), filter, sort),
        |session, (database, filter, sort)| {
            async move {
                let total = database
                    .collection::<Document>("api_tokens")
                    .count_documents(filter.clone())
                    .session(&mut *session)
                    .await?;
                let mut cursor = database
                    .collection::<TokenMeta>("api_tokens")
                    .find(filter.clone())
                    .projection(models::projection())
                    .sort(sort.clone())
                    .skip(skip)
                    .limit(input.page_size as i64)
                    .session(&mut *session)
                    .await?;
                Ok((cursor.stream(session).try_collect::<Vec<_>>().await?, total))
            }
            .boxed()
        },
    )
    .await?;
    Ok(Page::new(
        rows.into_iter()
            .map(TokenMeta::view)
            .collect::<Result<Vec<_>, _>>()?,
        &input,
        total,
    ))
}
pub async fn revoke(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
) -> Result<TokenView, AppError> {
    let session_id = context.require_session()?;
    let actor = Actor::new(context)?;
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "api-tokens", session).await?;
                permissions(database, session, actor.id, session_id).await?;
                let mut record = database
                    .collection::<TokenMeta>("api_tokens")
                    .find_one(doc! {"_id":id,"ownerId":actor.id})
                    .projection(models::projection())
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if record.revoked_at.is_none() {
                    let now = DateTime::now();
                    database
                        .collection::<Document>("api_tokens")
                        .update_one(
                            doc! {"_id":id,"ownerId":actor.id},
                            doc! {"$set":{"revokedAt":now}},
                        )
                        .session(&mut *session)
                        .await?;
                    shared::audit(database, actor, "apiTokens.revoke", id, session).await?;
                    record.revoked_at = Some(now);
                }
                Ok(record)
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn authenticate(
    state: &AppState,
    raw: &str,
    peer: Option<std::net::IpAddr>,
) -> Result<AuthContext, AppError> {
    let peer = peer.map(limits::canonical_peer);
    let database = state.require_database()?;
    if !crate::storage::schema::is_initialized(database).await? {
        return Err(AppError::NotReady);
    }
    usage::check_auth_ip(database, peer).await?;
    let Some(id) = credential::parse("tri_at1", raw) else {
        return Err(usage::failed_auth(database, peer).await?);
    };
    let Some(record) = database
        .collection::<TokenRecord>("api_tokens")
        .find_one(doc! {"_id":id,"revokedAt":null,"expiresAt":{"$gt":DateTime::now()}})
        .await?
    else {
        return Err(usage::failed_auth(database, peer).await?);
    };
    if !credential::verify(raw, &record.secret_hash) {
        return Err(usage::failed_auth(database, peer).await?);
    }
    let user = auth::repository::active_user(database, record.meta.owner_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let mut summary = auth::repository::summary(database, &user).await?;
    let original = summary.permissions;
    summary.permissions = record
        .meta
        .scopes
        .iter()
        .filter(|scope| original.iter().any(|p| p == "*" || p == *scope))
        .cloned()
        .collect();
    crate::common::observability::actor(record.meta.owner_id);
    let reservation = limits::reserve(
        database,
        record.meta.owner_id,
        id,
        record.meta.policy(),
        peer,
        crate::common::observability::call_descriptor().ok_or(AppError::Internal)?,
    )
    .await?;
    crate::common::observability::call_ticket(reservation.ticket.clone());
    reservation.require()?;
    Ok(AuthContext {
        user: summary,
        credential: AdminCredential::ApiToken { token_id: id },
    })
}
