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

/// Validate a git ref name (`--branch`, `--base`): non-empty and accepted by
/// `git check-ref-format --branch`.
///
/// A branch legitimately contains `/` — `user/ser-1234-thing` is the house style
/// — so it cannot go through [`validate_identifier`], and WIDENING that one to
/// admit `/` is not an option: it is the single gate on the task-id and on
/// `--dir`, both of which become path segments that `close` hands to
/// `remove_dir_all`. So ref names get their own, separate rule set.
///
/// Mirrors git's own check in-process (no shell-out): every slash-separated
/// component must be non-empty and must neither begin with `.` nor end with
/// `.lock`; the whole name may not contain `..`, `@{`, ASCII control characters,
/// space, or any of `~ ^ : ? * [ \`, may not end with `.`, may not begin with
/// `-` (it would parse as a git flag), and may not be `@`. The empty-component
/// and leading-`.` rules are also what stop a ref name from walking out of
/// `refs/heads/`.
pub fn validate_ref_name(value: &str, label: &str) -> Result<(), GroveError> {
    if value.is_empty() {
        return Err(GroveError::General(format!("{label} cannot be empty")));
    }
    let reject = |why: &str| -> Result<(), GroveError> {
        Err(GroveError::General(format!(
            "invalid {label} '{value}': {why}"
        )))
    };

    if value.starts_with('-') {
        return reject("must not begin with '-'");
    }
    if value == "@" {
        return reject("must not be '@'");
    }
    if value.contains("..") {
        return reject("must not contain '..'");
    }
    if value.contains("@{") {
        return reject("must not contain '@{'");
    }
    if value.ends_with('.') {
        return reject("must not end with '.'");
    }
    if value
        .chars()
        .any(|c| c.is_ascii_control() || " ~^:?*[\\".contains(c))
    {
        return reject("must not contain whitespace, control characters, or any of ~^:?*[\\");
    }
    for component in value.split('/') {
        if component.is_empty() {
            return reject("must not begin or end with '/' or contain '//'");
        }
        if component.starts_with('.') {
            return reject("no '/'-separated part may begin with '.'");
        }
        if component.ends_with(".lock") {
            return reject("no '/'-separated part may end with '.lock'");
        }
    }
    Ok(())
}

/// Validate a commit-ish (`--detach`): non-empty, no leading `-`, no whitespace
/// or control characters.
///
/// Deliberately looser than [`validate_ref_name`] — a commit-ish is a SHA, tag,
/// or revision expression (`HEAD~1`, `origin/main^`), so the `~^:` git forbids
/// in a ref NAME are legal here. This is only the argument-injection guard; git
/// itself rejects anything that does not resolve.
pub fn validate_commitish(value: &str, label: &str) -> Result<(), GroveError> {
    if value.is_empty() {
        return Err(GroveError::General(format!("{label} cannot be empty")));
    }
    if value.starts_with('-') {
        return Err(GroveError::General(format!(
            "invalid {label} '{value}': must not begin with '-'"
        )));
    }
    if value
        .chars()
        .any(|c| c.is_ascii_control() || c.is_whitespace())
    {
        return Err(GroveError::General(format!(
            "invalid {label} '{value}': must not contain whitespace or control characters"
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

    /// REF-slashed-branch: the v0.9.0 regression. `add --branch feat/x` was run
    /// through `validate_identifier` and rejected, while `init --branch feat/x`
    /// accepted it. Ref names are slashed by nature and must pass.
    #[test]
    fn ref_names_allow_slashes() {
        assert!(validate_ref_name("feat/slashed-name", "branch").is_ok());
        assert!(validate_ref_name("kishan/ser-6070-vibe-screening-node", "branch").is_ok());
        assert!(validate_ref_name("release/1.2", "base").is_ok());
        assert!(validate_ref_name("main", "branch").is_ok());
        assert!(validate_ref_name("a/b/c", "branch").is_ok());
        assert!(validate_ref_name("v1.2.3", "branch").is_ok());
    }

    /// REF-check-ref-format: what git itself refuses, plus a leading `-` (which
    /// would parse as a git flag). Mirrored in-process, no shell-out.
    #[test]
    fn ref_names_reject_what_git_rejects() {
        for bad in [
            "",          // empty
            "/leading",  // empty leading component
            "trailing/", // empty trailing component
            "a//b",      // empty inner component
            "..",        // parent traversal
            "feat/../x", // traversal mid-name
            "-dash",     // parses as a flag
            "@",         // git reserves the bare '@'
            "a@{0}",     // reflog syntax
            "has space",
            "tail.",     // trailing dot
            ".hidden",   // component beginning with '.'
            "feat/.hid", // inner component beginning with '.'
            "feat.lock", // .lock suffix
            "a/b.lock",  // .lock on an inner component
            "a~b",
            "a^b",
            "a:b",
            "a?b",
            "a*b",
            "a[b",
            "a\\b", // forbidden chars
            "a\tb", // control char
        ] {
            assert!(
                validate_ref_name(bad, "branch").is_err(),
                "ref name {bad:?} must be rejected"
            );
        }
    }

    /// SAFETY: `validate_ref_name` exists so `validate_identifier` never has to
    /// loosen. This asserts the traversal guard on identifiers is untouched —
    /// the task-id and `--dir` still become path segments armed for
    /// `remove_dir_all`, so `/` must remain fatal there.
    #[test]
    fn identifier_guard_did_not_loosen_when_ref_names_landed() {
        for bad in ["..", ".", "a/b", "/abs", "../evil", "feat/x"] {
            assert!(
                validate_identifier(bad, "task-id").is_err(),
                "identifier {bad:?} must still be rejected"
            );
        }
    }

    #[test]
    fn commitish_allows_revisions_but_not_flags() {
        assert!(validate_commitish("a1b2c3d", "commit").is_ok());
        assert!(validate_commitish("HEAD~1", "commit").is_ok());
        assert!(validate_commitish("origin/main^", "commit").is_ok());

        assert!(validate_commitish("", "commit").is_err());
        assert!(validate_commitish("--upload-pack=evil", "commit").is_err());
        assert!(validate_commitish("a b", "commit").is_err());
        assert!(validate_commitish("a\nb", "commit").is_err());
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
