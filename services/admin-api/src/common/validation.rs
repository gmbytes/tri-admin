use super::error::AppError;
use mongodb::bson::oid::ObjectId;

pub fn object_id(raw: &str) -> Result<ObjectId, AppError> {
    ObjectId::parse_str(raw).map_err(|_| AppError::Validation("资源 ID 格式无效".into()))
}

pub fn username(value: &str) -> Result<(), AppError> {
    if !(3..=64).contains(&value.chars().count())
        || !value
            .chars()
            .all(|c| c.is_alphanumeric() || "_-.".contains(c))
    {
        return Err(AppError::Validation(
            "用户名需为 3–64 个字母、数字、下划线、点或短横线".into(),
        ));
    }
    Ok(())
}

pub fn password(value: &str) -> Result<(), AppError> {
    if value.chars().count() < 12 || value.len() > 1024 {
        return Err(AppError::Validation(
            "密码需至少 12 个字符且不超过 1024 字节".into(),
        ));
    }
    Ok(())
}

pub fn text(value: &str, label: &str, max: usize) -> Result<(), AppError> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(AppError::Validation(format!(
            "{label}不能为空且最多 {max} 个字符"
        )));
    }
    Ok(())
}
