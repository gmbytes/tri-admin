use super::{
    credential,
    usage::{self, Descriptor, Ticket},
};
use crate::common::{
    error::AppError,
    transaction::{self, Failure, fail},
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub allowed_ips: Vec<String>,
    pub minute_limit: u32,
    pub daily_quota: u32,
}
pub fn canonical_peer(peer: std::net::IpAddr) -> std::net::IpAddr {
    match peer {
        std::net::IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(std::net::IpAddr::V4)
            .unwrap_or(peer),
        _ => peer,
    }
}
impl Policy {
    pub fn normalize(mut self) -> Result<Self, AppError> {
        if self.allowed_ips.len() > 32
            || !(1..=10000).contains(&self.minute_limit)
            || !(1..=10000000).contains(&self.daily_quota)
        {
            return Err(AppError::Validation("IP 或调用额度无效".into()));
        }
        self.allowed_ips = self
            .allowed_ips
            .iter()
            .map(|v| {
                v.parse::<ipnet::IpNet>()
                    .map(|net| net.trunc().to_string())
                    .map_err(|_| AppError::Validation("来源必须是 IPv4/IPv6 CIDR".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.allowed_ips.sort();
        self.allowed_ips.dedup();
        Ok(self)
    }
    fn allows(&self, peer: Option<std::net::IpAddr>) -> bool {
        self.allowed_ips.is_empty()
            || peer.is_some_and(|peer| {
                self.allowed_ips.iter().any(|value| {
                    value
                        .parse::<ipnet::IpNet>()
                        .is_ok_and(|net| net.contains(&peer))
                })
            })
    }
}
enum Denial {
    Ip,
    Limit(u64),
}
pub struct Reservation {
    pub ticket: Ticket,
    denial: Option<Denial>,
}
impl Reservation {
    pub fn require(self) -> Result<(), AppError> {
        match self.denial {
            None => Ok(()),
            Some(Denial::Ip) => Err(AppError::Forbidden),
            Some(Denial::Limit(seconds)) => Err(AppError::RateLimitedFor(seconds)),
        }
    }
}
pub async fn reserve(
    database: &Database,
    owner: ObjectId,
    token: ObjectId,
    policy: Policy,
    peer: Option<std::net::IpAddr>,
    descriptor: Descriptor,
) -> Result<Reservation, AppError> {
    let now = DateTime::now();
    let stamp = now.timestamp_millis();
    let call_id = ObjectId::new();
    let windows = [
        ("minute", 60000i64, policy.minute_limit),
        ("day", 86400000i64, policy.daily_quota),
    ]
    .map(|(kind, span, limit)| {
        let start = stamp.div_euclid(span) * span;
        let end = start + span;
        let id = credential::hash(&format!("admin-token:{token}:{kind}:{start}"));
        (id, end, limit)
    });
    for (id, end, _) in &windows {
        let inserted=database.collection::<Document>("api_usage_buckets").update_one(doc!{"_id":id},doc!{"$setOnInsert":{"tokenId":token,"ownerId":owner,"count":0i64,"resetAt":DateTime::from_millis(*end),"expireAt":DateTime::from_millis(end+86400000)}}).upsert(true).await;
        if let Err(error) = inserted
            && !matches!(&*error.kind,ErrorKind::Write(mongodb::error::WriteFailure::WriteError(write)) if write.code==11000)
        {
            return Err(error.into());
        }
    }
    let denial = transaction::run(
        database,
        (database.clone(), windows, policy, descriptor),
        |session, (database, windows, policy, descriptor)| {
            async move {
                if database
                    .collection::<Document>("api_tokens")
                    .update_one(
                        doc! {"_id":token,"ownerId":owner,"revokedAt":null,"expiresAt":{"$gt":now}},
                        doc! {"$inc":{"usageFence":1i64}},
                    )
                    .session(&mut *session)
                    .await?
                    .matched_count
                    != 1
                {
                    return Err(fail(Failure::Unauthorized));
                }
                let mut denial = if policy.allows(peer) {
                    None
                } else {
                    Some(Denial::Ip)
                };
                if denial.is_none() {
                    for (id, end, limit) in windows.iter() {
                        let bucket = database
                            .collection::<Document>("api_usage_buckets")
                            .find_one(doc! {"_id":id})
                            .session(&mut *session)
                            .await?
                            .ok_or_else(|| fail(Failure::NotReady))?;
                        if bucket
                            .get_i64("count")
                            .map_err(|_| fail(Failure::NotReady))?
                            >= i64::from(*limit)
                        {
                            denial =
                                Some(Denial::Limit(((*end - stamp).max(1) as u64).div_ceil(1000)));
                            break;
                        }
                    }
                }
                if denial.is_none() {
                    for (id, _, _) in windows.iter() {
                        database
                            .collection::<Document>("api_usage_buckets")
                            .update_one(doc! {"_id":id}, doc! {"$inc":{"count":1i64}})
                            .session(&mut *session)
                            .await?;
                    }
                    database
                        .collection::<Document>("api_tokens")
                        .update_one(doc! {"_id":token}, doc! {"$set":{"lastUsedAt":now}})
                        .session(&mut *session)
                        .await?;
                }
                usage::begin(
                    database,
                    session,
                    call_id,
                    usage::CallStart {
                        owner,
                        token,
                        peer,
                        descriptor,
                        now,
                    },
                )
                .await?;
                Ok(denial)
            }
            .boxed()
        },
    )
    .await?;
    Ok(Reservation {
        ticket: Ticket { id: call_id },
        denial,
    })
}
