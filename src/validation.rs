use std::path::Path;

use crate::error::GroveError;

/// Validate an identifier (repo name or task-id): non-empty, [a-zA-Z0-9._-]+,
/// and not a relative path component.
///
/// The charset alone is not enough. An identifier is joined onto `tasks_dir`
/// and the result is armed for `remove_dir_all` (task creation journals its own
/// undo; `close` deletes the task dir). `.` is in the charset, so `".."` passed
/// every check and `tasks_dir.join("..")` resolves — lexically — to the PARENT
/// of the tasks dir, i.e. `$HOME`. A failed `grove init ..` then unwound its
/// journal straight through the home directory. So reject any value that is not
/// a single normal path component (`.`, `..`, or anything a separator sneaks in).
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
    if !is_single_normal_component(value) {
        return Err(GroveError::General(format!(
            "invalid {label} '{value}': must not be a path component like '.' or '..'"
        )));
    }
    Ok(())
}

/// True only when `value` is exactly one ordinary path component — no `.`, no
/// `..`, no separators, no root. This is what makes `base.join(value)` provably
/// stay directly under `base`.
fn is_single_normal_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

/// True when `path` resolves to a location inside `base` (and not `base`
/// itself). Both sides are canonicalized, so it holds even when `base` is a
/// symlink, and it fails closed if either path cannot be resolved.
///
/// A last-line guard for recursive deletes whose target did not come through
/// [`validate_identifier`] — e.g. a task path loaded from a legacy state file.
pub fn is_within(path: &Path, base: &Path) -> bool {
    let (Ok(path), Ok(base)) = (path.canonicalize(), base.canonicalize()) else {
        return false;
    };
    path != base && path.starts_with(&base)
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

    /// Regression: `.` is in the charset, so `.`/`..` slipped through and let
    /// `tasks_dir.join(id)` escape the tasks dir — `grove init ..` armed
    /// `remove_dir_all` on `$HOME`. These must be rejected as identifiers even
    /// though every character is individually allowed.
    #[test]
    fn path_traversal_identifiers_are_rejected() {
        assert!(validate_identifier("..", "task-id").is_err());
        assert!(validate_identifier(".", "task-id").is_err());
        // A dot-containing name that is still a single normal component is fine.
        assert!(validate_identifier("v1.2.3", "task-id").is_ok());
        assert!(validate_identifier("...", "task-id").is_ok());
    }

    #[test]
    fn is_within_rejects_the_base_itself_and_escapes() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("task");
        std::fs::create_dir(&inside).unwrap();

        assert!(is_within(&inside, root.path()));
        // The base is not "within" itself — guards a delete of the tasks dir.
        assert!(!is_within(root.path(), root.path()));
        // A parent escape does not resolve inside.
        assert!(!is_within(root.path().parent().unwrap(), root.path()));
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
