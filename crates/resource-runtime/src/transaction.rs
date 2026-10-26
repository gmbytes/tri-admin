use crate::Error;
use futures_util::future::BoxFuture;
use mongodb::{ClientSession, Database};
pub async fn run<C: Send, R: Send, F>(
    database: &Database,
    context: C,
    snapshot: bool,
    callback: F,
) -> Result<R, Error>
where
    F: Send
        + for<'a> FnMut(&'a mut ClientSession, &'a mut C) -> BoxFuture<'a, mongodb::error::Result<R>>,
{
    let mut session = database.client().start_session().await?;
    if snapshot {
        session
            .start_transaction()
            .read_concern(mongodb::options::ReadConcern::snapshot())
            .and_run(context, callback)
            .await
            .map_err(Into::into)
    } else {
        session
            .start_transaction()
            .and_run(context, callback)
            .await
            .map_err(Into::into)
    }
}
