use crate::error::GroveError;

/// Validate an identifier (repo name or task-id): non-empty, [a-zA-Z0-9._-]+
pub fn validate_identifier(value: &str, label: &str) -> Result<(), GroveError> {
    if value.is_empty() {
        return Err(GroveError::General(format!("{label} cannot be empty")));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(GroveError::General(format!(
            "invalid {label} '{value}': must match [a-zA-Z0-9._-]+"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_identifier_valid() {
        assert!(validate_identifier("my-repo", "repo name").is_ok());
        assert!(validate_identifier("my.repo", "repo name").is_ok());
        assert!(validate_identifier("my_repo", "repo name").is_ok());
        assert!(validate_identifier("MyRepo123", "repo name").is_ok());
        assert!(validate_identifier("a", "repo name").is_ok());
        assert!(validate_identifier("TASK-1", "task-id").is_ok());
        assert!(validate_identifier("my.task", "task-id").is_ok());
        assert!(validate_identifier("ABC-123", "task-id").is_ok());
    }

    #[test]
    fn test_validate_identifier_invalid() {
        assert!(validate_identifier("", "repo name").is_err());
        assert!(validate_identifier("my/repo", "repo name").is_err());
        assert!(validate_identifier("my repo", "repo name").is_err());
        assert!(validate_identifier("my@repo", "repo name").is_err());
        assert!(validate_identifier("", "task-id").is_err());
        assert!(validate_identifier("my/task", "task-id").is_err());
        assert!(validate_identifier("my task", "task-id").is_err());
        assert!(validate_identifier("my@task", "task-id").is_err());
    }

    #[test]
    fn test_validate_identifier_error_message() {
        let err = validate_identifier("", "repo name").unwrap_err();
        assert!(err.to_string().contains("repo name cannot be empty"));

        let err = validate_identifier("bad/name", "task-id").unwrap_err();
        assert!(err.to_string().contains("invalid task-id"));
        assert!(err.to_string().contains("bad/name"));
    }
}
