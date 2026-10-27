use super::dto::{Transport, TransportKind};
use crate::common::{error::AppError, vault::SealedBox};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
fn time(value: DateTime) -> Result<String, AppError> {
    crate::modules::extensions::models::time(value)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMeta {
    pub id: ObjectId,
    pub service_id: String,
    pub state: String,
    pub transport_kind: TransportKind,
    pub pid: Option<i64>,
    pub release_confirmed: bool,
    pub lease_until: DateTime,
    pub updated_at: DateTime,
    pub error_code: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub name: String,
    pub transport: Transport,
    pub capability_fingerprint: String,
    pub enabled: bool,
    pub version: i64,
    pub runtime: Option<RuntimeMeta>,
    pub control_action_id: Option<ObjectId>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeView {
    pub id: String,
    pub state: String,
    pub transport_kind: TransportKind,
    pub pid: Option<i64>,
    pub release_confirmed: bool,
    pub updated_at: String,
    pub error_code: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    pub id: String,
    pub owner_id: String,
    pub request_id: String,
    pub name: String,
    pub transport: Transport,
    pub capability_current: bool,
    pub enabled: bool,
    pub version: i64,
    pub runtime: Option<RuntimeView>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}
impl Connection {
    pub fn view(self, current: bool) -> Result<ConnectionView, AppError> {
        let runtime = self
            .runtime
            .map(|v| {
                let state = if !v.release_confirmed
                    && v.state != "retired"
                    && v.lease_until < DateTime::now()
                {
                    if v.transport_kind == TransportKind::Http {
                        "disconnected".into()
                    } else {
                        "orphaned".into()
                    }
                } else {
                    v.state
                };
                Ok::<RuntimeView, AppError>(RuntimeView {
                    id: v.id.to_hex(),
                    state,
                    transport_kind: v.transport_kind,
                    pid: v.pid,
                    release_confirmed: v.release_confirmed,
                    updated_at: time(v.updated_at)?,
                    error_code: v.error_code,
                })
            })
            .transpose()?;
        Ok(ConnectionView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            request_id: self.request_id,
            name: self.name,
            transport: self.transport,
            capability_current: current,
            enabled: self.enabled,
            version: self.version,
            runtime,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            deleted_at: self.deleted_at.map(time).transpose()?,
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub connection_id: ObjectId,
    pub owner_id: ObjectId,
    pub actor_id: ObjectId,
    pub service_id: String,
    pub state: String,
    pub transport_kind: TransportKind,
    pub pid: Option<i64>,
    pub release_confirmed: bool,
    pub lease_until: DateTime,
    pub tools_json: String,
    pub error_code: Option<String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub actor_id: ObjectId,
    pub owner_id: ObjectId,
    pub connection_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub kind: String,
    pub status: String,
    pub connection_version: i64,
    pub instance_id: Option<ObjectId>,
    pub target_action_id: Option<ObjectId>,
    pub name: Option<String>,
    pub arguments_sha256: Option<String>,
    pub error_code: Option<String>,
    pub result_kind: Option<String>,
    pub result_size: Option<i64>,
    pub result_sha256: Option<String>,
    pub created_at: DateTime,
    pub finished_at: Option<DateTime>,
    pub cancel_requested: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    pub id: String,
    pub actor_id: String,
    pub owner_id: String,
    pub connection_id: String,
    pub request_id: String,
    pub kind: String,
    pub status: String,
    pub connection_version: i64,
    pub instance_id: Option<String>,
    pub target_action_id: Option<String>,
    pub name: Option<String>,
    pub arguments_sha256: Option<String>,
    pub error_code: Option<String>,
    pub result_kind: Option<String>,
    pub result_size: Option<i64>,
    pub result_sha256: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub cancel_requested: bool,
}
impl Action {
    pub fn view(self) -> Result<ActionView, AppError> {
        Ok(ActionView {
            id: self.id.to_hex(),
            actor_id: self.actor_id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            connection_id: self.connection_id.to_hex(),
            request_id: self.request_id,
            kind: self.kind,
            status: self.status,
            connection_version: self.connection_version,
            instance_id: self.instance_id.map(|v| v.to_hex()),
            target_action_id: self.target_action_id.map(|v| v.to_hex()),
            name: self.name,
            arguments_sha256: self.arguments_sha256,
            error_code: self.error_code,
            result_kind: self.result_kind,
            result_size: self.result_size,
            result_sha256: self.result_sha256,
            created_at: time(self.created_at)?,
            finished_at: self.finished_at.map(time).transpose()?,
            cancel_requested: self.cancel_requested,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub actor_id: ObjectId,
    pub owner_id: ObjectId,
    pub connection_id: ObjectId,
    pub sealed: SealedBox,
    pub expires_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultView {
    pub content: String,
    pub size: i64,
    pub sha256: String,
    pub kind: String,
}
