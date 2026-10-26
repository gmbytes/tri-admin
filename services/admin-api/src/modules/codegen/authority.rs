use crate::modules::{
    auth::AuthContext,
    rbac::{
        owned::{self, Resource},
        shared::Actor,
    },
};
use mongodb::{ClientSession, Database};
pub use owned::owns;
pub async fn fresh(
    database: &Database,
    session: &mut ClientSession,
    context: &AuthContext,
    write: bool,
) -> mongodb::error::Result<Actor> {
    owned::fresh(
        database,
        session,
        context,
        Resource {
            schema: "codegen",
            version: crate::storage::codegen_schema::VERSION,
            read: "codegen.read",
            write: "codegen.write",
        },
        write,
    )
    .await
}
