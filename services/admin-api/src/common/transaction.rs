use super::error::AppError;
use futures_util::future::BoxFuture;
use mongodb::{
    ClientSession, Database,
    error::{Error, ErrorKind, WriteFailure},
};

#[derive(Debug, Clone)]
pub enum Failure {
    Unauthorized,
    NotFound,
    Forbidden,
    NotReady,
    Conflict(&'static str),
    Validation(&'static str),
    PayloadTooLarge,
    SecretStoreUnavailable,
    RateLimited,
}
pub fn fail(reason: Failure) -> Error {
    Error::custom(reason)
}

pub fn map_error(error: Error) -> AppError {
    if let Some(reason) = error.get_custom::<Failure>() {
        return match reason {
            Failure::Unauthorized => AppError::Unauthorized,
            Failure::NotFound => AppError::NotFound,
            Failure::Forbidden => AppError::Forbidden,
            Failure::NotReady => AppError::NotReady,
            Failure::Conflict(message) => AppError::Conflict((*message).into()),
            Failure::Validation(message) => AppError::Validation((*message).into()),
            Failure::PayloadTooLarge => AppError::PayloadTooLarge,
            Failure::SecretStoreUnavailable => AppError::SecretStoreUnavailable,
            Failure::RateLimited => AppError::RateLimited,
        };
    }
    if matches!(&*error.kind,ErrorKind::Write(WriteFailure::WriteError(write)) if write.code==11000)
        || matches!(&*error.kind,ErrorKind::Command(command) if command.code==11000)
    {
        return AppError::Conflict("唯一字段已存在".into());
    }
    AppError::Database(error)
}

pub async fn run<C: Send, R: Send, F>(
    database: &Database,
    context: C,
    callback: F,
) -> Result<R, AppError>
where
    F: Send
        + for<'a> FnMut(&'a mut ClientSession, &'a mut C) -> BoxFuture<'a, mongodb::error::Result<R>>,
{
    let mut session = database.client().start_session().await?;
    session
        .start_transaction()
        .and_run(context, callback)
        .await
        .map_err(map_error)
}

pub async fn run_snapshot<C: Send, R: Send, F>(
    database: &Database,
    context: C,
    callback: F,
) -> Result<R, AppError>
where
    F: Send
        + for<'a> FnMut(&'a mut ClientSession, &'a mut C) -> BoxFuture<'a, mongodb::error::Result<R>>,
{
    let mut session = database.client().start_session().await?;
    session
        .start_transaction()
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .and_run(context, callback)
        .await
        .map_err(map_error)
}
