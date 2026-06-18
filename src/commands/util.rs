//! Small shared helpers for command implementations.

/// Find a registered repo by name in an already-loaded list.
///
/// init/close/sync legitimately `list_repos()` once and iterate; this dedups
/// the inline `.iter().find(|r| r.name == name)` scans where a missing repo is
/// genuinely an error. Sites that tolerate "not found" keep their own
/// `Option`-returning `.find`.
pub fn resolve_repo<'a>(
    repos: &'a [crate::db::RepoEntry],
    name: &str,
) -> Result<&'a crate::db::RepoEntry, crate::error::GroveError> {
    repos
        .iter()
        .find(|r| r.name == name)
        .ok_or_else(|| crate::error::GroveError::RepoNotRegistered(name.to_string()))
}
