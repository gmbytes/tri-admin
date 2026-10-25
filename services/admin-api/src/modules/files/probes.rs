use super::storage::{BlobStore, ProbeObservation, StorageError};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared,
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
#[derive(Clone)]
pub(crate) struct ProbeJournal {
    database: Database,
    profile: ObjectId,
}
impl ProbeJournal {
    pub fn new(database: Database, profile: ObjectId) -> Self {
        Self { database, profile }
    }
    pub async fn begin(&self, id: uuid::Uuid) -> Result<(), StorageError> {
        transaction::run(&self.database,(self.database.clone(),self.profile),|session,(database,profile)|{async move{
            shared::lock(database,"settings",session).await?;shared::lock(database,"files",session).await?;
            if database.collection::<Document>("_schema_migrations").find_one(doc!{"_id":"files","version":crate::storage::files_schema::VERSION}).session(&mut *session).await?.is_none(){return Err(fail(Failure::NotReady))}
            if database.collection::<Document>("integration_profiles").find_one(doc!{"_id":*profile,"kind":"s3","enabled":true,"deletedAt":null}).session(&mut *session).await?.is_none(){return Err(fail(Failure::NotFound))}
            let now=DateTime::now();database.collection::<Document>("file_storage_probes").insert_one(doc!{"_id":id.to_string(),"storageProfileId":*profile,"state":"writing","phase":0i64,"leaseUntil":DateTime::from_millis(now.timestamp_millis()+30000),"nextRunAt":now,"createdAt":now,"updatedAt":now}).session(session).await?;Ok(())
        }.boxed()}).await.map_err(|_|StorageError::ConnectionFailed)
    }
    pub async fn advance(&self, id: uuid::Uuid, from: i64, to: i64) -> Result<(), StorageError> {
        let now = DateTime::now();
        let result=self.database.collection::<Document>("file_storage_probes").update_one(doc!{"_id":id.to_string(),"state":"writing","phase":from,"leaseUntil":{"$gt":now}},doc!{"$set":{"phase":to,"leaseUntil":DateTime::from_millis(now.timestamp_millis()+30000),"updatedAt":now}}).await.map_err(|_|StorageError::ConnectionFailed)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(StorageError::Cancelled)
        }
    }
    pub async fn settle(
        &self,
        id: uuid::Uuid,
        uncertain: bool,
        no_object: bool,
    ) -> Result<bool, StorageError> {
        let now = DateTime::now();
        let state = if no_object {
            "done"
        } else if uncertain {
            "unknown"
        } else {
            "cleanup"
        };
        let filter = doc! {"_id":id.to_string(),"state":{"$in":["writing","unknown"]}};
        let mut fields = doc! {"state":state,"leaseUntil":now,"nextRunAt":DateTime::from_millis(now.timestamp_millis()+if uncertain{30000}else{0}),"updatedAt":now};
        if !uncertain {
            fields.insert("phase", 3i64);
        }
        self.database
            .collection::<Document>("file_storage_probes")
            .update_one(filter, doc! {"$set":fields})
            .await
            .map_err(|_| StorageError::ConnectionFailed)?;
        Ok(self
            .database
            .collection::<Document>("file_storage_probes")
            .find_one(doc! {"_id":id.to_string(),"state":"cleanup"})
            .await
            .map_err(|_| StorageError::ConnectionFailed)?
            .is_some())
    }
    pub async fn done(&self, id: uuid::Uuid) -> Result<(), StorageError> {
        self.database
            .collection::<Document>("file_storage_probes")
            .update_one(
                doc! {"_id":id.to_string(),"state":"cleanup"},
                doc! {"$set":{"state":"done","updatedAt":DateTime::now()}},
            )
            .await
            .map_err(|_| StorageError::ConnectionFailed)?;
        Ok(())
    }
}
pub(crate) async fn background(state: &AppState) -> Result<u64, AppError> {
    let database = state.require_database()?;
    let now = DateTime::now();
    let rows=database.collection::<Document>("file_storage_probes").find(doc!{"$or":[{"state":"writing","leaseUntil":{"$lte":now}},{"state":{"$in":["unknown","cleanup"]},"nextRunAt":{"$lte":now}}]}).sort(doc!{"nextRunAt":1,"createdAt":1,"_id":1}).limit(4).await?.try_collect::<Vec<_>>().await?;
    let mut cleaned = 0;
    for row in rows {
        let id = row
            .get_str("_id")
            .ok()
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .ok_or(AppError::NotReady)?;
        let profile = row
            .get_object_id("storageProfileId")
            .map_err(|_| AppError::NotReady)?;
        database
            .collection::<Document>("file_storage_probes")
            .update_one(
                doc! {"_id":id.to_string(),"state":"writing","leaseUntil":{"$lte":now}},
                doc! {"$set":{"state":"unknown","leaseUntil":now,"updatedAt":now}},
            )
            .await?;
        let Some(current) = database
            .collection::<Document>("file_storage_probes")
            .find_one(doc! {"_id":id.to_string()})
            .await?
        else {
            continue;
        };
        let phase = current.get_i64("phase").map_err(|_| AppError::NotReady)?;
        let status = current.get_str("state").map_err(|_| AppError::NotReady)?;
        if status == "done" || status == "writing" {
            continue;
        }
        if phase == 0 {
            database
                .collection::<Document>("file_storage_probes")
                .update_one(
                    doc! {"_id":id.to_string(),"state":"unknown","phase":0i64},
                    doc! {"$set":{"state":"done","updatedAt":now}},
                )
                .await?;
            cleaned += 1;
            continue;
        }
        let store = match BlobStore::from_profile(database, state.config.clone(), profile).await {
            Ok(store) => store,
            Err(_) => {
                defer(database, id).await?;
                continue;
            }
        };
        let safe = status == "cleanup"
            || phase >= 3
            || match store.observe_probe(id).await {
                Ok(ProbeObservation::Value(value)) => phase == 1 || phase == 2 && value == 1,
                Ok(ProbeObservation::Other) => phase == 1,
                _ => false,
            };
        if !safe {
            defer(database, id).await?;
            continue;
        }
        database
            .collection::<Document>("file_storage_probes")
            .update_one(
                doc! {"_id":id.to_string(),"state":{"$in":["unknown","cleanup"]}},
                doc! {"$set":{"state":"cleanup","updatedAt":now}},
            )
            .await?;
        if store.delete_probe(id).await.is_ok() {
            database
                .collection::<Document>("file_storage_probes")
                .update_one(
                    doc! {"_id":id.to_string(),"state":"cleanup"},
                    doc! {"$set":{"state":"done","updatedAt":DateTime::now()}},
                )
                .await?;
            cleaned += 1;
        } else {
            defer(database, id).await?;
        }
    }
    Ok(cleaned)
}
async fn defer(database: &Database, id: uuid::Uuid) -> Result<(), AppError> {
    database.collection::<Document>("file_storage_probes").update_one(doc!{"_id":id.to_string(),"state":{"$ne":"done"}},doc!{"$set":{"nextRunAt":DateTime::from_millis(DateTime::now().timestamp_millis()+30000)}}).await?;
    Ok(())
}
