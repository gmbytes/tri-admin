use crate::{
    Error,
    schema::{Kind, Schema},
};
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
use std::sync::Arc;
#[derive(Clone)]
pub struct Config {
    pub(crate) key: String,
    pub(crate) origin: String,
}
impl Config {
    pub fn new(key: String, origin: String) -> Result<Self, Error> {
        let url = url::Url::parse(&origin).map_err(|_| Error::Validation)?;
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if key.len() < 32
            || key.len() > 4096
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https" || url.scheme() == "http" && local)
        {
            return Err(Error::Validation);
        }
        Ok(Self {
            key,
            origin: url.origin().ascii_serialization(),
        })
    }
}
#[derive(Clone)]
pub struct State {
    pub database: Database,
    pub schema: Arc<Schema>,
    pub(crate) config: Arc<Config>,
}
impl State {
    pub fn new(database: Database, schema: Schema, config: Config) -> Result<Self, Error> {
        schema.validate()?;
        Ok(Self {
            database,
            schema: Arc::new(schema),
            config: Arc::new(config),
        })
    }
    pub async fn ready(&self) -> Result<(), Error> {
        let checksum = self.schema.checksum()?;
        if self
            .database
            .collection::<Document>("_resource_schema")
            .find_one(doc! {"_id":&self.schema.resource,"version":1,"checksum":checksum})
            .await?
            .is_none()
        {
            return Err(Error::NotReady);
        }
        Ok(())
    }
    pub async fn migrate(&self) -> Result<(), Error> {
        let hello = self.database.run_command(doc! {"hello":1}).await?;
        if !hello.contains_key("setName") && hello.get_str("msg").ok() != Some("isdbgrid") {
            return Err(Error::NotReady);
        }
        let metadata = self.database.collection::<Document>("_resource_schema");
        let checksum = self.schema.checksum()?;
        let previous = metadata
            .find_one(doc! {"_id":&self.schema.resource})
            .await?;
        if let Some(old) = &previous
            && (old.get_i32("version").ok() != Some(1)
                || old.get_str("checksum").ok() != Some(checksum.as_str()))
        {
            return Err(Error::NotReady);
        }
        if previous.is_none() {
            if self
                .database
                .collection::<Document>(&self.schema.collection())
                .count_documents(doc! {})
                .await?
                != 0
            {
                return Err(Error::NotReady);
            }
            for name in ["_resource_admins", "_resource_sessions", "_resource_audit"] {
                if self
                    .database
                    .collection::<Document>(name)
                    .count_documents(doc! {"application":&self.schema.resource})
                    .await?
                    != 0
                {
                    return Err(Error::NotReady);
                }
            }
        }
        for name in [
            "_resource_schema",
            "_resource_admins",
            "_resource_sessions",
            "_resource_attempts",
            "_resource_audit",
            &self.schema.collection(),
        ] {
            if let Err(error) = self.database.create_collection(name).await
                && !matches!(&*error.kind,ErrorKind::Command(error) if error.code==48)
            {
                return Err(error.into());
            }
        }
        index(
            &self.database,
            "_resource_admins",
            doc! {"application":1,"usernameKey":1},
            true,
            None,
        )
        .await?;
        for (collection, field) in [
            ("_resource_sessions", "expiresAt"),
            ("_resource_attempts", "resetAt"),
        ] {
            let mut keys = Document::new();
            keys.insert(field, 1);
            self.database
                .collection::<Document>(collection)
                .create_index(
                    IndexModel::builder()
                        .keys(keys)
                        .options(
                            IndexOptions::builder()
                                .expire_after(std::time::Duration::ZERO)
                                .build(),
                        )
                        .build(),
                )
                .await?;
        }
        index(
            &self.database,
            "_resource_sessions",
            doc! {"application":1,"adminId":1,"revoked":1,"expiresAt":1},
            false,
            None,
        )
        .await?;
        index(
            &self.database,
            "_resource_audit",
            doc! {"application":1,"createdAt":-1,"_id":-1},
            false,
            None,
        )
        .await?;
        for keys in [
            doc! {"ownerId":1,"deletedAt":1,"createdAt":-1,"_id":-1},
            doc! {"deletedAt":1,"createdAt":-1,"_id":-1},
        ] {
            index(&self.database, &self.schema.collection(), keys, false, None).await?;
        }
        for field in self
            .schema
            .fields
            .iter()
            .filter(|f| f.searchable || f.unique)
        {
            let mut keys = doc! {"ownerId":1};
            keys.insert(format!("values.{}", field.name), 1);
            index(
                &self.database,
                &self.schema.collection(),
                keys,
                field.unique,
                Some(doc! {"deletedAt":null}),
            )
            .await?;
            if field.searchable && field.kind == Kind::Text {
                let mut keys = Document::new();
                keys.insert(format!("values.{}", field.name), 1);
                index(
                    &self.database,
                    &self.schema.collection(),
                    keys,
                    false,
                    Some(doc! {"deletedAt":null}),
                )
                .await?;
            }
        }
        metadata
            .update_one(
                doc! {"_id":&self.schema.resource},
                doc! {"$setOnInsert":{"version":1,"checksum":checksum,"createdAt":DateTime::now()}},
            )
            .upsert(true)
            .await?;
        self.ready().await
    }
    pub async fn provision_admin(
        &self,
        username: &str,
        password: &str,
        can_write: bool,
        scope_all: bool,
    ) -> Result<crate::auth::AdminView, Error> {
        crate::auth::provision(self, username, password, can_write, scope_all).await
    }
}
async fn index(
    database: &Database,
    name: &str,
    keys: Document,
    unique: bool,
    partial: Option<Document>,
) -> Result<(), Error> {
    database
        .collection::<Document>(name)
        .create_index(
            IndexModel::builder()
                .keys(keys)
                .options(
                    IndexOptions::builder()
                        .unique(unique)
                        .partial_filter_expression(partial)
                        .build(),
                )
                .build(),
        )
        .await?;
    Ok(())
}
