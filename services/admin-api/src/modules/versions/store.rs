use super::models::{Bundle, Counts, MAX_PACKAGE_BYTES, MetaRecord};
use crate::{
    common::transaction::{Failure, fail},
    modules::rbac::shared::Actor,
};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

pub struct Package<'a> {
    pub id: ObjectId,
    pub bundle: &'a Bundle,
    pub json: &'a str,
    pub checksum: &'a str,
    pub counts: &'a Counts,
    pub reads: &'a [String],
    pub origin: &'a str,
}

pub async fn insert(
    database: &Database,
    actor: &Actor,
    package: Package<'_>,
    session: &mut ClientSession,
) -> mongodb::error::Result<MetaRecord> {
    if package.json.len() > MAX_PACKAGE_BYTES {
        return Err(fail(Failure::PayloadTooLarge));
    }
    let creator = database
        .collection::<Document>("users")
        .find_one(doc! {"_id":actor.id})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Forbidden))?;
    let name = creator
        .get_str("displayName")
        .map_err(|_| fail(Failure::NotReady))?;
    let now = DateTime::now();
    let release = &package.bundle.release;
    let counts = package.counts;
    database.collection::<Document>("versions").insert_one(doc!{
        "_id":package.id,"name":&release.name,"code":&release.code,"codeKey":release.code.to_ascii_lowercase(),
        "description":&release.description,"checksum":package.checksum,"byteLength":package.json.len() as i64,
        "counts":{"menus":counts.menus,"ancestors":counts.ancestors,"dictionaries":counts.dictionaries,"dictionaryItems":counts.dictionary_items,"parameters":counts.parameters,"apis":counts.apis},
        "readPermissions":package.reads,"createdById":actor.id,"createdByName":name,"origin":package.origin,
        "version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null
    }).session(&mut *session).await?;
    database
        .collection::<Document>("version_packages")
        .insert_one(doc! {"_id":package.id,"packageJson":package.json})
        .session(&mut *session)
        .await?;
    database
        .collection::<MetaRecord>("versions")
        .find_one(doc! {"_id":package.id})
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))
}
