use super::{models::ApplicationMeta, service};
use crate::{
    common::{
        error::AppError,
        observability,
        transaction::{Failure, fail},
    },
    modules::{
        auth::{AuthContext, repository},
        open_access::{
            context::OpenContext,
            scopes::Scope,
            sessions::{self, PrincipalKind, Record},
        },
        rbac::shared::{self, Actor},
    },
};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
#[derive(Clone)]
pub enum Authority {
    Administrator(AuthContext),
    Application(Box<OpenContext>),
}
impl Authority {
    pub fn administrator(context: &AuthContext) -> Result<Self, AppError> {
        context.require_session()?;
        Ok(Self::Administrator(context.clone()))
    }
    pub fn application(context: &OpenContext) -> Result<Self, AppError> {
        context.require(PrincipalKind::Application, Scope::UsersWrite)?;
        Ok(Self::Application(Box::new(context.clone())))
    }
    pub async fn authorize(
        &self,
        database: &Database,
        session: &mut ClientSession,
        application: ObjectId,
    ) -> mongodb::error::Result<Authorized> {
        match self {
            Self::Administrator(context) => {
                let sid = context
                    .require_session()
                    .map_err(|_| fail(Failure::Forbidden))?;
                let actor = Actor::new(context).map_err(|_| fail(Failure::Unauthorized))?;
                shared::lock(database, "rbac", session).await?;
                let permissions = repository::interactive_permissions(
                    database,
                    session,
                    actor.id,
                    sid,
                    &[
                        "applications.read",
                        "applicationUsers.read",
                        "applicationUsers.write",
                    ],
                )
                .await?;
                let app = service::lock_app(database, session, application).await?;
                let grant = [Scope::ProfileRead, Scope::ProfileWrite]
                    .into_iter()
                    .filter(|scope| {
                        permissions.contains("*") || permissions.contains(scope.admin_permission())
                    })
                    .collect();
                Ok(Authorized {
                    application: app,
                    grant,
                    identity: Identity::Administrator(actor),
                })
            }
            Self::Application(context) => {
                if context.application_id() != application {
                    return Err(fail(Failure::Forbidden));
                }
                let (record, app, grant) =
                    sessions::resolve(database, session, &context.claims).await?;
                if record.principal_kind != PrincipalKind::Application
                    || !grant.contains(&Scope::UsersWrite)
                {
                    return Err(fail(Failure::Forbidden));
                }
                Ok(Authorized {
                    application: app,
                    grant,
                    identity: Identity::Application(record),
                })
            }
        }
    }
}
enum Identity {
    Administrator(Actor),
    Application(Record),
}
pub struct Authorized {
    pub application: ApplicationMeta,
    grant: Vec<Scope>,
    identity: Identity,
}
#[derive(Clone, Copy)]
pub enum Action {
    Create,
    Update,
    Delete,
    ResetPassword,
}
impl Action {
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Create => ("applicationUsers.create", "open.users.create"),
            Self::Update => ("applicationUsers.update", "open.users.update"),
            Self::Delete => ("applicationUsers.delete", "open.users.delete"),
            Self::ResetPassword => ("applicationUsers.resetPassword", "open.users.resetPassword"),
        }
    }
}
impl Authorized {
    pub fn check_grant(&self, scopes: &[Scope]) -> mongodb::error::Result<()> {
        if scopes.iter().all(|scope| {
            self.grant.contains(scope) && self.application.allowed_scopes.contains(scope)
        }) {
            return Ok(());
        }
        Err(fail(match self.identity {
            Identity::Administrator(_) => Failure::Validation("用户 Scope 超出当前授权或应用集合"),
            Identity::Application(_) => Failure::Forbidden,
        }))
    }
    pub async fn audit(
        &self,
        database: &Database,
        session: &mut ClientSession,
        action: Action,
        target: ObjectId,
    ) -> mongodb::error::Result<()> {
        let (admin, open) = action.names();
        match &self.identity {
            Identity::Administrator(actor) => {
                shared::audit(database, actor, admin, target, session).await
            }
            Identity::Application(record) => {
                database.collection::<Document>("audit_events").insert_one(observability::event(doc!{"category":"application","action":open,"actorId":null,"applicationId":record.application_id,"principalKind":"application","userId":null,"sessionId":record.id,"targetId":target,"outcome":"success","createdAt":DateTime::now()})).session(session).await?;
                Ok(())
            }
        }
    }
}
