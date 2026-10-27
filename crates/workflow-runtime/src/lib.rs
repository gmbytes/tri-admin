//! Pure bounded plans, analysis results and private transcript export.
mod analysis;
mod markdown;
mod plan;
pub use analysis::{Analysis, Field, FieldKind, Module};
pub use markdown::{Artifact, Entry, Outcome, Transcript};
pub use plan::{Expected, Inputs, Mode, Plan, Step};
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Workflow definition is invalid")]
    Plan,
    #[error("Workflow input is invalid")]
    Input,
    #[error("Workflow dependency is not available")]
    MissingDependency,
    #[error("AI analysis is invalid")]
    Analysis,
    #[error("Workflow data exceeds its limit")]
    Limit,
}
fn label(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= maximum
        && !value.chars().any(char::is_control)
}
fn key(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}
