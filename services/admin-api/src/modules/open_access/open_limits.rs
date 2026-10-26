use super::{
    context::OpenContext,
    credential,
    jwt::Claims,
    limits::Policy,
    sessions,
    usage::{RETENTION_MS, Ticket},
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    state::AppState,
};
use futures_util::FutureExt;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
pub async fn reserve(
    state: &AppState,
    claims: Claims,
    peer: Option<std::net::IpAddr>,
) -> Result<OpenContext, AppError> {
    let database = state.require_database()?;
    if !crate::storage::schema::is_initialized(database).await? {
        return Err(AppError::NotReady);
    }
    let application =
        ObjectId::parse_str(&claims.application_id).map_err(|_| AppError::Unauthorized)?;
    let now = DateTime::now();
    let stamp = now.timestamp_millis();
    let id = ObjectId::new();
    let descriptor = crate::common::observability::call_descriptor().ok_or(AppError::Internal)?;
    let windows = [("minute", 60000i64), ("day", 86400000i64)].map(|(kind, span)| {
        let start = stamp.div_euclid(span) * span;
        (
            credential::hash(&format!("application:{application}:{kind}:{start}")),
            start + span,
            kind,
        )
    });
    for (key, end, _) in &windows {
        let result=database.collection::<Document>("application_usage_buckets").update_one(doc!{"_id":key},doc!{"$setOnInsert":{"applicationId":application,"count":0i64,"resetAt":DateTime::from_millis(*end),"expireAt":DateTime::from_millis(end+86400000)}}).upsert(true).await;
        if let Err(error) = result
            && !matches!(&*error.kind,mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(write))if write.code==11000)
        {
            return Err(error.into());
        }
    }
    let(context,denial)=transaction::run(database,(database.clone(),claims,windows,descriptor),|transaction,(database,claims,windows,descriptor)|async move{
        let(record,app,scopes)=sessions::resolve(database,transaction,claims).await?;
        let policy=Policy{allowed_ips:app.allowed_ips,minute_limit:app.minute_limit,daily_quota:app.daily_quota};let mut denial=if policy.allows(peer){None}else{Some((403,0u64))};
        if denial.is_none(){for(key,end,kind)in windows.iter(){let limit=if *kind=="minute"{policy.minute_limit}else{policy.daily_quota};let bucket=database.collection::<Document>("application_usage_buckets").find_one(doc!{"_id":key}).session(&mut *transaction).await?.ok_or_else(||fail(Failure::NotReady))?;if bucket.get_i64("count").map_err(|_|fail(Failure::NotReady))?>=i64::from(limit){denial=Some((429,((*end-stamp).max(1)as u64).div_ceil(1000)));break;}}}
        if denial.is_none(){for(key,_,_)in windows.iter(){database.collection::<Document>("application_usage_buckets").update_one(doc!{"_id":key},doc!{"$inc":{"count":1i64}}).session(&mut *transaction).await?;}}
        database.collection::<Document>("application_call_logs").insert_one(doc!{"_id":id,"applicationId":application,"principalKind":record.principal_kind.as_str(),"userId":record.user_id,"sessionId":record.id,"requestId":&descriptor.request_id,"operationId":descriptor.operation_id,"method":descriptor.method,"route":descriptor.route,"ip":peer.map(|ip|ip.to_string()),"state":"started","status":null,"errorCode":null,"durationMs":null,"createdAt":now,"finishedAt":null,"expireAt":DateTime::from_millis(stamp+RETENTION_MS)}).session(transaction).await?;
        Ok((OpenContext{claims:claims.clone(),session:record,scopes},denial))
    }.boxed()).await?;
    crate::common::observability::call_ticket(Ticket::Open { id });
    if let Some((code, seconds)) = denial {
        return Err(if code == 403 {
            AppError::Forbidden
        } else {
            AppError::RateLimitedFor(seconds)
        });
    }
    Ok(context)
}
