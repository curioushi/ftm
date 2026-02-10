//! Path utilities for cross-platform relative path handling.
//! Normalizes path separators to forward slash for index keys and glob matching.

/// Normalize a relative path string to use forward slashes.
/// Used for index keys and glob pattern matching so behavior is consistent on Windows.
#[must_use]
pub fn normalize_rel_path(s: &str) -> String {
    s.replace('\\', "/")
}
