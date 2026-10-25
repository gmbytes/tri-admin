use crate::{
    common::error::AppError,
    modules::rbac::models::{RoleBrief, UserView},
};
use futures_util::TryStreamExt;
use mongodb::{
    Database,
    bson::{doc, oid::ObjectId},
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Deserialize)]
struct Label {
    #[serde(rename = "_id")]
    id: ObjectId,
    name: String,
}

pub async fn enrich_roles(database: &Database, users: &mut [UserView]) -> Result<(), AppError> {
    let ids = users
        .iter()
        .flat_map(|user| &user.role_ids)
        .map(|id| ObjectId::parse_str(id).map_err(|_| AppError::Internal))
        .collect::<Result<HashSet<_>, _>>()?;
    if ids.is_empty() {
        return Ok(());
    }
    let labels = database
        .collection::<Label>("roles")
        .find(doc! {"_id":{"$in":ids.into_iter().collect::<Vec<_>>()},"deletedAt":null})
        .projection(doc! {"_id":1,"name":1})
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let labels = labels
        .into_iter()
        .map(|label| (label.id.to_hex(), label.name))
        .collect::<HashMap<_, _>>();
    for user in users {
        user.roles = user
            .role_ids
            .iter()
            .filter_map(|id| {
                labels.get(id).map(|name| RoleBrief {
                    id: id.clone(),
                    name: name.clone(),
                })
            })
            .collect();
    }
    Ok(())
}
