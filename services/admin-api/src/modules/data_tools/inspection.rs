use super::{
    formats,
    models::{InspectInput, Inspection},
};
use crate::common::error::AppError;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
pub fn inspect(input: InspectInput) -> Result<Inspection, AppError> {
    if input.content_base64.len() > formats::MAX_FILE_BYTES.div_ceil(3) * 4 {
        return Err(AppError::PayloadTooLarge);
    }
    let bytes = STANDARD
        .decode(&input.content_base64)
        .map_err(|_| AppError::Validation("文件编码无效".into()))?;
    let table = formats::decode(input.format, &bytes)?;
    Ok(Inspection {
        resource: input.resource,
        format: input.format,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        headers: table.headers,
        row_count: table.rows.len(),
    })
}
