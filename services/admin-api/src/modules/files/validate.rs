use super::storage::BlobSpec;
use crate::common::{error::AppError, validation};
pub fn version(value: i64) -> Result<(), AppError> {
    if !(1..i64::MAX).contains(&value) {
        return Err(AppError::Validation("版本号无效".into()));
    }
    Ok(())
}
pub fn category(name: &str, code: &str, description: &str) -> Result<(), AppError> {
    validation::text(name, "分类名称", 80)?;
    if code.is_empty()
        || code.len() > 64
        || !code.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        || !code
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
    {
        return Err(AppError::Validation("分类标识无效".into()));
    }
    if description.chars().count() > 1000 {
        return Err(AppError::Validation("分类说明最多1000字符".into()));
    }
    Ok(())
}
pub fn filename(name: &str) -> Result<(), AppError> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.trim() != name
        || name.is_empty()
        || name.len() > 255
        || name.ends_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || name == "."
        || name == ".."
        || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9')
    {
        return Err(AppError::Validation(
            "文件名包含不支持的字符或保留名称".into(),
        ));
    }
    Ok(())
}
pub fn spec(size: u64, hash: &str) -> Result<BlobSpec, AppError> {
    BlobSpec::new(size, hash).map_err(|_| AppError::Validation("文件大小或SHA-256无效".into()))
}
pub fn prefix(value: &str) -> String {
    let mut result = "^".to_owned();
    for c in value.chars() {
        if ".+*?^${}()|[]\\".contains(c) {
            result.push('\\')
        }
        result.push(c)
    }
    result
}
